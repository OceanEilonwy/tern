//! The one core of Tern: everything the app does, with no toolkit in it. A platform draws the views
//! published here and sends back what the person did.

// async_trait marks the boxed futures it returns #[must_use], which clippy on stable Rust 1.99 calls doubled.
#![allow(clippy::double_must_use)]

#[cfg(feature = "uniffi")]
uniffi::setup_scaffolding!();

mod app;
pub mod cleanup;
mod doc;
pub mod format;
pub mod logs;
pub mod paths;
pub mod rules;
pub mod semantic;
pub mod settings;
pub mod view;

use std::panic::{catch_unwind, AssertUnwindSafe};
use std::sync::{Arc, OnceLock};

use tokio::sync::mpsc::{unbounded_channel, UnboundedSender};

use app::{App, Msg};
use semantic::Meaning;
use tern_cache::Cache;
use tern_mail::{Backend, FolderKind};
use tern_sync::fetcher::Fetcher;
use paths::Paths;
pub use view::*;

/// The name of the secret holding the saved session.
pub const SESSION_SECRET: &str = "session";
/// What the name of the secret holding an account's cache key starts with, before the address.
pub const CACHE_KEY_SECRET: &str = "cache-key:";

/// Secrets kept by the platform: the session, and the key of the encrypted mail cache.
#[cfg_attr(feature = "uniffi", uniffi::export(with_foreign))]
pub trait SecretStore: Send + Sync {
	/// `None` only when there is no such secret. A store that cannot tell says so with an error:
	/// taking that for "none" would have a new cache key replace the one the cache needs.
	fn get(&self, name: String) -> Result<Option<Vec<u8>>, SecretError>;
	fn put(&self, name: String, value: Vec<u8>) -> Result<(), SecretError>;
	fn remove(&self, name: String);
}

/// The platform's store of secrets could not do what was asked, for now at least.
#[derive(thiserror::Error, Debug)]
#[cfg_attr(feature = "uniffi", derive(uniffi::Error))]
pub enum SecretError {
	#[error("the secret store is unavailable")]
	Unavailable,
}

#[cfg(feature = "uniffi")]
impl From<uniffi::UnexpectedUniFFICallbackError> for SecretError {
	fn from(_: uniffi::UnexpectedUniFFICallbackError) -> Self {
		SecretError::Unavailable
	}
}

/// Where what is published goes.
#[cfg_attr(feature = "uniffi", uniffi::export(with_foreign))]
pub trait EngineListener: Send + Sync {
	fn on_update(&self, update: Update);
	fn on_effect(&self, effect: Effect);
	/// Something for the system's log. Comes from any thread.
	fn on_log(&self, message: String);
}

/// Where the app keeps things on this device.
#[derive(Clone, Debug)]
#[cfg_attr(feature = "uniffi", derive(uniffi::Record))]
pub struct EngineConfig {
	pub config_dir: String,
	pub cache_dir: String,
	/// Where the platform keeps the certificates it trusts, `:` between directories.
	pub cert_dirs: String,
}

/// Android stops background work at 10 minutes; a check gives up before that and tries again later.
const BACKGROUND_LIMIT: std::time::Duration = std::time::Duration::from_secs(8 * 60);

/// The Tokio runtime everything here runs on, for a platform's own work too.
pub fn runtime() -> &'static tokio::runtime::Runtime {
	static RT: OnceLock<tokio::runtime::Runtime> = OnceLock::new();
	RT.get_or_init(|| tokio::runtime::Builder::new_multi_thread().worker_threads(3).thread_name("tern-io").enable_all().build().expect("start the Tokio runtime"))
}

/// The first of `dirs`, `:` between them, that can be read.
fn first_dir(dirs: &str) -> Option<&str> {
	dirs.split(':').find(|dir| !dir.is_empty() && std::fs::read_dir(dir).is_ok())
}

#[cfg_attr(feature = "uniffi", derive(uniffi::Object))]
pub struct Engine {
	tx: UnboundedSender<Msg>,
	settings: Arc<std::sync::Mutex<settings::Settings>>,
}

/// The parts of a session a platform's screens use directly, until they draw from views instead.
#[derive(Clone)]
pub struct SessionParts {
	pub backend: Arc<dyn Backend>,
	pub cache: Arc<Cache>,
	pub fetcher: Fetcher,
}

/// What a platform still working on the session's parts directly needs to hear. Comes from any thread.
pub trait Host: Send + Sync {
	/// The session changed: a new account, its backend swapped once connected, or `None` at sign-out.
	fn session(&self, parts: Option<SessionParts>);
}

/// How a platform differs. What Android needs is the default.
pub struct Options {
	/// What the app calls the device it is on, as in "the mail on this phone".
	pub device: &'static str,
	/// Shared when the platform uses the language model itself.
	pub meaning: Option<Arc<Meaning>>,
	/// An account already open, with no saved session: the demo at start-up.
	pub signed_in: Option<Arc<dyn Backend>>,
	pub host: Option<Arc<dyn Host>>,
	/// How a failed list says to try again.
	pub retry_hint: &'static str,
	/// The list and the conversation are on screen together, so when the open conversation leaves
	/// the list its neighbour opens. On a phone it just goes back to the list.
	pub follow_neighbour: bool,
	/// A lone draft goes straight to a composer, instead of opening as a conversation first.
	pub lone_draft_composes: bool,
}

impl Default for Options {
	fn default() -> Options {
		Options { device: "phone", meaning: None, signed_in: None, host: None, retry_hint: "Pull down to try again.", follow_neighbour: false, lone_draft_composes: true }
	}
}

// Apart because a constructor's own attribute cannot depend on the feature.
#[cfg(feature = "uniffi")]
#[uniffi::export]
impl Engine {
	#[uniffi::constructor]
	pub fn new(config: EngineConfig, secrets: Arc<dyn SecretStore>, listener: Arc<dyn EngineListener>) -> Arc<Engine> {
		Engine::open(config, secrets, listener)
	}
}

#[cfg_attr(feature = "uniffi", uniffi::export)]
impl Engine {
	/// What the person did. Returns at once; what follows is published.
	pub fn send(&self, action: Action) {
		let _ = self.tx.send(Msg::Action(action));
	}

	/// Checks for mail now, for the system's background work. Blocks until it has run, so it is
	/// not for the main thread.
	pub fn background_check(&self) -> BackgroundResult {
		let (reply, answer) = std::sync::mpsc::channel();
		let none = BackgroundResult { notices: Vec::new(), summary: None, retry: true };
		if self.tx.send(Msg::Background(reply)).is_err() {
			return none;
		}
		answer.recv_timeout(BACKGROUND_LIMIT).unwrap_or(none)
	}
}

impl Drop for Engine {
	/// Nobody can ask it for anything now, so its account's work ends, and the account stays signed in.
	fn drop(&mut self) {
		let _ = self.tx.send(Msg::Stop);
	}
}

impl Engine {
	pub fn open(config: EngineConfig, secrets: Arc<dyn SecretStore>, listener: Arc<dyn EngineListener>) -> Arc<Engine> {
		// The system's log, for what is wrong and has nowhere on screen to go.
		let log_to = listener.clone();
		logs::init(move |message| log_to.on_log(message));
		// The TLS library takes its roots from here and no other way, and only one directory:
		// the first this version of Android has. Set before anything can read it.
		if let Some(dir) = first_dir(&config.cert_dirs) {
			std::env::set_var("SSL_CERT_DIR", dir);
		}
		Engine::open_with(Paths::new(&config.config_dir, &config.cache_dir), Options::default(), secrets, listener)
	}
}

/// One thing happening. A bug in one action must not leave the app dead with its window still up.
fn step(app: &mut App, msg: Msg) {
	let _ = catch_unwind(AssertUnwindSafe(|| match msg {
		Msg::Action(action) => app.handle(action),
		Msg::Done(generation, apply) => {
			if generation == app.generation() {
				apply(app);
			}
		},
		Msg::Background(reply) => app.background_check(reply),
		Msg::SyncNow => app.sync_now(),
		Msg::SyncKinds(kinds) => app.sync_kinds(&kinds),
		Msg::Stop => {},
	}));
}

impl Engine {
	pub fn open_with(paths: Paths, options: Options, secrets: Arc<dyn SecretStore>, listener: Arc<dyn EngineListener>) -> Arc<Engine> {
		let (tx, mut rx) = unbounded_channel();
		let mut app = App::new(tx.clone(), listener, secrets, runtime().handle().clone(), paths, options);
		let settings = app.settings_handle();
		runtime().spawn(async move {
			app.start();
			while let Some(msg) = rx.recv().await {
				if matches!(msg, Msg::Stop) {
					return app.stop();
				}
				step(&mut app, msg);
				// Everything already here at once is drawn as one change.
				while let Ok(msg) = rx.try_recv() {
					if matches!(msg, Msg::Stop) {
						return app.stop();
					}
					step(&mut app, msg);
				}
				app.publish();
			}
		});
		Arc::new(Engine { tx, settings })
	}

	/// The settings as they are now.
	pub fn settings(&self) -> settings::Settings {
		*self.settings.lock().unwrap_or_else(|p| p.into_inner())
	}

	/// Starts the sync's next round now.
	pub fn sync_now(&self) {
		let _ = self.tx.send(Msg::SyncNow);
	}

	/// Mail in folders of these kinds changed behind the core's back, so it brings them up to date.
	pub fn sync_kinds(&self, kinds: &[FolderKind]) {
		let _ = self.tx.send(Msg::SyncKinds(kinds.to_vec()));
	}
}

#[cfg(test)]
mod tests;

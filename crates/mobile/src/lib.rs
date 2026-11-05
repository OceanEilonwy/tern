//! Tern for Android: everything the app does. The Kotlin side is a thin wrapper that draws the
//! views published here and sends back what the person did.

// async_trait marks the boxed futures it returns #[must_use], which clippy on stable Rust 1.99 calls doubled.
#![allow(clippy::double_must_use)]

uniffi::setup_scaffolding!();

mod app;
mod doc;
pub mod view;

use std::panic::{catch_unwind, AssertUnwindSafe};
use std::sync::{Arc, OnceLock};

use tokio::sync::mpsc::{unbounded_channel, UnboundedSender};

use app::{App, Msg};
pub use view::*;

/// Secrets kept by the platform: the session, and the key of the encrypted mail cache.
#[uniffi::export(with_foreign)]
pub trait SecretStore: Send + Sync {
	fn get(&self, name: String) -> Option<Vec<u8>>;
	fn put(&self, name: String, value: Vec<u8>);
	fn remove(&self, name: String);
}

/// Where what is published goes.
#[uniffi::export(with_foreign)]
pub trait EngineListener: Send + Sync {
	fn on_update(&self, update: Update);
	fn on_effect(&self, effect: Effect);
}

/// Where the app keeps things on this device.
#[derive(uniffi::Record, Clone, Debug)]
pub struct EngineConfig {
	pub config_dir: String,
	pub cache_dir: String,
	/// Where the platform keeps the certificates it trusts, `:` between directories.
	pub cert_dirs: String,
}

/// Android stops background work at 10 minutes; a check gives up before that and tries again later.
const BACKGROUND_LIMIT: std::time::Duration = std::time::Duration::from_secs(8 * 60);

fn runtime() -> &'static tokio::runtime::Runtime {
	static RT: OnceLock<tokio::runtime::Runtime> = OnceLock::new();
	RT.get_or_init(|| tokio::runtime::Builder::new_multi_thread().worker_threads(3).thread_name("tern-io").enable_all().build().expect("start the Tokio runtime"))
}

/// The first of `dirs`, `:` between them, that can be read.
fn first_dir(dirs: &str) -> Option<&str> {
	dirs.split(':').find(|dir| !dir.is_empty() && std::fs::read_dir(dir).is_ok())
}

#[derive(uniffi::Object)]
pub struct Engine {
	tx: UnboundedSender<Msg>,
}

#[uniffi::export]
impl Engine {
	#[uniffi::constructor]
	pub fn new(config: EngineConfig, secrets: Arc<dyn SecretStore>, listener: Arc<dyn EngineListener>) -> Arc<Engine> {
		// The shared crates find their directories and the TLS roots through these.
		std::env::set_var("XDG_CONFIG_HOME", &config.config_dir);
		std::env::set_var("XDG_CACHE_HOME", &config.cache_dir);
		std::env::set_var("HOME", &config.cache_dir);
		// The loader takes one directory, so it gets the first this version of Android has.
		if let Some(dir) = first_dir(&config.cert_dirs) {
			std::env::set_var("SSL_CERT_DIR", dir);
		}
		Engine::start(config.cache_dir.into(), secrets, listener)
	}

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
	}));
}

impl Engine {
	fn start(cache_dir: std::path::PathBuf, secrets: Arc<dyn SecretStore>, listener: Arc<dyn EngineListener>) -> Arc<Engine> {
		let (tx, mut rx) = unbounded_channel();
		let mut app = App::new(tx.clone(), listener, secrets, runtime().handle().clone(), cache_dir);
		runtime().spawn(async move {
			app.start();
			while let Some(msg) = rx.recv().await {
				step(&mut app, msg);
				// Everything already here at once is drawn as one change.
				while let Ok(msg) = rx.try_recv() {
					step(&mut app, msg);
				}
				app.publish();
			}
		});
		Arc::new(Engine { tx })
	}
}

#[cfg(test)]
mod tests;

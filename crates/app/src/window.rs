//! The main window: the sign-in page or the mailbox, and the app-wide actions.

use std::cell::{Cell, RefCell};
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::Arc;

use adw::prelude::*;
use gtk::gio;
use tern_cache::{sync::Level, Cache};
use tern_core::{demo::DemoBackend, Backend, Error, FolderKind};

use crate::activity::Activity;
use crate::fetcher::{Fetcher, Tuning};
use crate::settings::Settings;
use crate::{background, format, keyring, login::Login, mailbox::Mailbox, runtime, screenshot};

type ChangedHook = Box<dyn Fn(&[FolderKind])>;

/// What every part of the window shares: the window, its toasts, the signed-in account and its cache.
#[derive(Clone)]
pub struct Ctx {
	pub window: adw::ApplicationWindow,
	pub toasts: adw::ToastOverlay,
	backend: Rc<RefCell<Option<Arc<dyn Backend>>>>,
	cache: Rc<RefCell<Option<Arc<Cache>>>>,
	fetcher: Rc<RefCell<Option<Fetcher>>>,
	settings: Rc<Cell<Settings>>,
	/// Wakes the background sync.
	wake: Arc<tokio::sync::Notify>,
	changed: Rc<RefCell<Option<ChangedHook>>>,
	/// What Tern is doing now, in a word.
	pub activity: Activity,
}

impl Ctx {
	pub fn backend(&self) -> Option<Arc<dyn Backend>> {
		self.backend.borrow().clone()
	}

	pub fn cache(&self) -> Option<Arc<Cache>> {
		self.cache.borrow().clone()
	}

	/// Fetches mail bodies for the account signed in now.
	pub fn fetcher(&self) -> Option<Fetcher> {
		self.fetcher.borrow().clone()
	}

	pub fn settings(&self) -> Settings {
		self.settings.get()
	}

	pub fn set_settings(&self, settings: Settings) {
		self.settings.set(settings);
		settings.save();
		self.sync_now();
	}

	/// Changes how the window is laid out; `save` once the change is done, not on every step of a drag.
	pub fn set_layout(&self, settings: Settings, save: bool) {
		self.settings.set(settings);
		if save {
			settings.save();
		}
	}

	/// Starts the background sync's next round now.
	pub fn sync_now(&self) {
		self.wake.notify_one();
	}

	pub fn wake(&self) -> Arc<tokio::sync::Notify> {
		self.wake.clone()
	}

	pub fn toast(&self, text: &str) {
		let toast = adw::Toast::new(text);
		toast.set_timeout(4);
		self.toasts.add_toast(toast);
	}

	pub fn error(&self, err: &Error) {
		self.toast(&err.to_string());
	}

	/// Called by a composer once its mail is sent.
	pub fn sent(&self, subject: &str) {
		self.toast(&format::sent_toast(subject));
		self.changed(&[FolderKind::Sent, FolderKind::Drafts]);
	}

	/// Mail in folders of these kinds changed on the server, as after saving a draft.
	pub fn changed(&self, kinds: &[FolderKind]) {
		if let Some(f) = self.changed.borrow().as_ref() {
			f(kinds);
		}
	}

	pub fn connect_changed(&self, f: impl Fn(&[FolderKind]) + 'static) {
		*self.changed.borrow_mut() = Some(Box::new(f));
	}
}

/// Where an account's mail cache lives.
fn cache_path(email: &str) -> PathBuf {
	let base = std::env::var_os("XDG_CACHE_HOME")
		.map(PathBuf::from)
		.or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".cache")))
		.unwrap_or_else(std::env::temp_dir);
	let name: String = email.chars().map(|c| if c.is_ascii_alphanumeric() || c == '.' || c == '@' { c } else { '_' }).collect();
	base.join("tern").join("mail").join(format!("{name}.db"))
}

pub struct Window {
	pub ctx: Ctx,
	stack: gtk::Stack,
	pub login: Login,
	pub mailbox: Rc<Mailbox>,
}

impl Window {
	pub fn new(app: &adw::Application) -> Rc<Window> {
		let window = adw::ApplicationWindow::builder().application(app).title("Tern").default_width(1280).default_height(800).build();
		window.set_size_request(360, 294);
		let toasts = adw::ToastOverlay::new();
		let ctx = Ctx {
			window: window.clone(),
			toasts: toasts.clone(),
			backend: Rc::default(),
			cache: Rc::default(),
			fetcher: Rc::default(),
			settings: Rc::new(Cell::new(Settings::load())),
			wake: Arc::default(),
			changed: Rc::default(),
			activity: Activity::default(),
		};

		let stack = gtk::Stack::builder().transition_type(gtk::StackTransitionType::Crossfade).build();
		let starting = adw::Spinner::builder().width_request(32).height_request(32).halign(gtk::Align::Center).valign(gtk::Align::Center).build();
		let starting_page = adw::ToolbarView::new();
		starting_page.add_top_bar(&adw::HeaderBar::builder().show_title(false).build());
		starting_page.set_content(Some(&starting));
		stack.add_named(&starting_page, Some("starting"));

		let login = Login::new();
		stack.add_named(&login.root, Some("login"));
		let mailbox = Mailbox::new(ctx.clone());
		stack.add_named(&mailbox.root, Some("mailbox"));
		toasts.set_child(Some(&stack));
		window.set_content(Some(&toasts));
		for bp in mailbox.breakpoints() {
			window.add_breakpoint(bp);
		}

		let this = Rc::new(Window { ctx, stack, login, mailbox });
		// The GTK window owns this struct until it is destroyed; callbacks and actions hold only weak references.
		let owner = RefCell::new(Some(this.clone()));
		window.connect_destroy(move |_| drop(owner.take()));
		this.add_actions(app);
		{
			let weak = Rc::downgrade(&this);
			this.login.connect_signed_in(move |backend, saved| {
				if let Some(this) = weak.upgrade() {
					// Only a real account keeps its mail on disk.
					let persistent = saved.is_some();
					this.signed_in(backend, saved, persistent);
				}
			});
		}
		if let Some((w, h)) = screenshot::size() {
			window.set_default_size(w, h);
			// On a headless display maximizing fills the screen; narrow scenes keep their size.
			if w > 600 {
				window.maximize();
			}
			// Page changes are crossfades, which never advance when broadway draws no frames.
			if let Some(settings) = gtk::Settings::default() {
				settings.set_gtk_enable_animations(false);
			}
		}
		window.present();
		this.clone().start();
		this
	}

	fn add_actions(self: &Rc<Self>, app: &adw::Application) {
		let weak = Rc::downgrade(self);
		let sign_out = gio::SimpleAction::new("sign-out", None);
		sign_out.connect_activate(move |_, _| {
			if let Some(this) = weak.upgrade() {
				this.sign_out();
			}
		});
		app.add_action(&sign_out);

		let window = self.ctx.window.clone();
		let about = gio::SimpleAction::new("about", None);
		about.connect_activate(move |_, _| about_dialog().present(Some(&window)));
		app.add_action(&about);

		let ctx = self.ctx.clone();
		let preferences = gio::SimpleAction::new("preferences", None);
		preferences.connect_activate(move |_, _| preferences_dialog(&ctx).present(Some(&ctx.window)));
		app.add_action(&preferences);

		let window = self.ctx.window.clone();
		let shortcuts = gio::SimpleAction::new("shortcuts", None);
		shortcuts.connect_activate(move |_, _| shortcuts_dialog().present(Some(&window)));
		app.add_action(&shortcuts);
	}

	/// Opens the saved session if there is one, else shows the sign-in page.
	fn start(self: Rc<Self>) {
		if std::env::var_os("TERN_DEMO").is_some() {
			let backend: Arc<dyn Backend> = if screenshot::scene().is_some() { screenshot::backend() } else { Arc::new(DemoBackend::new()) };
			self.signed_in(backend, None, false);
			screenshot::drive(self);
			return;
		}
		if screenshot::scene().is_some() {
			self.show_login(None);
			screenshot::drive(self);
			return;
		}
		runtime::spawn_local(async move {
			let saved = runtime::run(keyring::load()).await;
			let Some(saved) = saved else { return self.show_login(None) };
			let session = saved.session.clone();
			match runtime::run(async move { tern_tuta::resume(&session).await }).await {
				Ok(backend) => self.signed_in(backend, None, true),
				Err(err) => {
					if err != Error::SessionExpired {
						self.ctx.error(&err);
					}
					self.show_login(Some(&saved.email));
				}
			}
		});
	}

	fn show_login(&self, email: Option<&str>) {
		self.login.reset(email);
		self.stack.set_visible_child_name("login");
		self.ctx.window.set_title(Some("Tern"));
		self.login.focus();
	}

	/// `saved` is the session to remember, with the address it belongs to. A `persistent` account
	/// keeps its mail in an encrypted cache on disk; the demo's lives in memory.
	fn signed_in(self: &Rc<Self>, backend: Arc<dyn Backend>, saved: Option<(String, String)>, persistent: bool) {
		tern_tuta::trace("opening the mailbox");
		*self.ctx.backend.borrow_mut() = Some(backend.clone());
		if let Some((email, session)) = saved {
			runtime::spawn_local(async move { runtime::run(async move { keyring::store(&email, &session).await }).await });
		}
		self.stack.set_visible_child_name("mailbox");
		let email = backend.account().address;
		let this = self.clone();
		runtime::spawn_local(async move {
			let cache = if persistent {
				let key = runtime::run({
					let email = email.clone();
					async move { keyring::cache_key(&email).await }
				})
				.await;
				let path = cache_path(&email);
				match key {
					Some(key) => runtime::run(async move { Cache::open(&path, &key).ok() }).await,
					None => None,
				}
			} else {
				None
			};
			// Without a keyring nothing is kept on disk.
			let cache = Arc::new(cache.unwrap_or_else(Cache::in_memory));
			if !this.ctx.backend().is_some_and(|now| Arc::ptr_eq(&now, &backend)) {
				return;
			}
			*this.ctx.fetcher.borrow_mut() = Some(Fetcher::new(backend.clone(), cache.clone(), runtime::handle(), Tuning::DEFAULT));
			*this.ctx.cache.borrow_mut() = Some(cache);
			this.mailbox.start();
			background::start(&this.ctx, Rc::downgrade(&this.mailbox));
			tern_tuta::trace("mailbox shown");
		});
	}

	/// Ends the session and forgets the account's mail on this computer.
	fn sign_out(self: &Rc<Self>) {
		let Some(backend) = self.ctx.backend.borrow_mut().take() else { return };
		let cache = self.ctx.cache.borrow_mut().take();
		if let Some(fetcher) = self.ctx.fetcher.borrow_mut().take() {
			fetcher.stop();
		}
		let email = backend.account().address;
		self.mailbox.clear();
		self.show_login(Some(&email));
		runtime::spawn_local(async move {
			runtime::run(async move {
				let _ = backend.sign_out().await;
				keyring::forget().await;
				drop(cache);
				let path = cache_path(&email);
				for suffix in ["", "-wal", "-shm"] {
					let _ = std::fs::remove_file(format!("{}{suffix}", path.display()));
				}
				keyring::forget_cache_key(&email).await;
			})
			.await
		});
	}
}

fn preferences_dialog(ctx: &Ctx) -> adw::PreferencesDialog {
	let dialog = adw::PreferencesDialog::new();
	let page = adw::PreferencesPage::new();
	let group = adw::PreferencesGroup::builder()
		.title("Offline Mail")
		.description("Tern keeps recent mail on this computer, encrypted, so it shows at once and can be read offline. It checks for new mail every few minutes.")
		.build();
	let labels: Vec<String> = Level::ALL
		.iter()
		.map(|l| format!("{} — {} per folder, {} opened ahead", level_label(*l), l.headers(), l.bodies()))
		.collect();
	let model = gtk::StringList::new(&labels.iter().map(String::as_str).collect::<Vec<_>>());
	let row = adw::ComboRow::builder().title("Keep Ready").model(&model).build();
	row.set_selected(Level::ALL.iter().position(|l| *l == ctx.settings().offline).unwrap_or(1) as u32);
	let ctx = ctx.clone();
	row.connect_selected_notify(move |row| {
		if let Some(level) = Level::ALL.get(row.selected() as usize) {
			ctx.set_settings(Settings { offline: *level, ..ctx.settings() });
		}
	});
	group.add(&row);
	page.add(&group);
	dialog.add(&page);
	dialog
}

fn level_label(level: Level) -> &'static str {
	match level {
		Level::Low => "Low",
		Level::Medium => "Medium",
		Level::High => "High",
	}
}

fn about_dialog() -> adw::AboutDialog {
	adw::AboutDialog::builder()
		.application_name("Tern")
		.application_icon(crate::APP_ID)
		.version(env!("CARGO_PKG_VERSION"))
		.developer_name("Henry Hollingworth")
		.license_type(gtk::License::Gpl30Only)
		.comments("Unofficial client for Tuta mail. Not affiliated with Tuta GmbH.")
		.build()
}

fn shortcuts_dialog() -> adw::ShortcutsDialog {
	let dialog = adw::ShortcutsDialog::new();
	let sections: [(&str, &[(&str, &str)]); 4] = [
		(
			"Moving Around",
			&[
				("Next Conversation", "<Alt>Down"),
				("Previous Conversation", "<Alt>Up"),
				("Read Conversation", "Return"),
				("Back to the List", "Escape"),
				("Open a Message in the Conversation", "space"),
				("Go to Folder 1…9", "<Control>1"),
				("Folders as Icons, or Show Folders", "F9"),
			],
		),
		(
			"Mail",
			&[
				("New Message", "<Control>n"),
				("Reply", "<Control>r"),
				("Reply All", "<Control><Shift>r"),
				("Forward", "<Control>l"),
				("Mark as Unread", "<Control>u"),
				("Move Conversation to Trash", "Delete"),
				("Check for New Mail", "F5"),
			],
		),
		(
			"Writing",
			&[
				("Send", "<Control>Return"),
				("Attach Files", "<Control><Shift>a"),
				("Close and Keep as Draft", "<Control>w Escape"),
				("Accept a Suggested Address", "Tab Return"),
			],
		),
		("General", &[("Preferences", "<Control>comma"), ("Keyboard Shortcuts", "<Control>question"), ("Quit", "<Control>q")]),
	];
	for (title, items) in sections {
		let section = adw::ShortcutsSection::new(Some(title));
		for (name, accel) in items {
			section.add(adw::ShortcutsItem::new(name, accel));
		}
		dialog.add(section);
	}
	dialog
}

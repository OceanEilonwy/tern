//! The main window: the sign-in page or the mailbox, and the app-wide actions.

use std::cell::RefCell;
use std::rc::Rc;
use std::sync::Arc;

use adw::prelude::*;
use gtk::gio;
use tern_core::{demo::DemoBackend, Backend, Error};

use crate::{keyring, login::Login, mailbox::Mailbox, runtime, screenshot};

type Callback = Box<dyn Fn()>;

/// What every part of the window shares: the window, its toasts and the signed-in account.
#[derive(Clone)]
pub struct Ctx {
	pub window: adw::ApplicationWindow,
	pub toasts: adw::ToastOverlay,
	backend: Rc<RefCell<Option<Arc<dyn Backend>>>>,
	sent: Rc<RefCell<Option<Callback>>>,
}

impl Ctx {
	pub fn backend(&self) -> Option<Arc<dyn Backend>> {
		self.backend.borrow().clone()
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
	pub fn sent(&self) {
		self.toast("Sent");
		if let Some(f) = self.sent.borrow().as_ref() {
			f();
		}
	}

	pub fn connect_sent(&self, f: impl Fn() + 'static) {
		*self.sent.borrow_mut() = Some(Box::new(f));
	}
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
		let ctx = Ctx { window: window.clone(), toasts: toasts.clone(), backend: Rc::default(), sent: Rc::default() };

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
		this.add_actions(app);
		{
			let weak = Rc::downgrade(&this);
			this.login.connect_signed_in(move |backend, saved| {
				if let Some(this) = weak.upgrade() {
					this.signed_in(backend, saved);
				}
			});
		}
		if let Some((w, h)) = screenshot::size() {
			window.set_default_size(w, h);
			window.maximize();
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

		let window = self.ctx.window.clone();
		let shortcuts = gio::SimpleAction::new("shortcuts", None);
		shortcuts.connect_activate(move |_, _| shortcuts_dialog().present(Some(&window)));
		app.add_action(&shortcuts);
	}

	/// Opens the saved session if there is one, else shows the sign-in page.
	fn start(self: Rc<Self>) {
		if std::env::var_os("TERN_DEMO").is_some() {
			let backend: Arc<dyn Backend> = if screenshot::scene().is_some() { screenshot::backend() } else { Arc::new(DemoBackend::new()) };
			self.signed_in(backend, None);
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
				Ok(backend) => self.signed_in(backend, None),
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

	/// `saved` is the session to remember, with the address it belongs to.
	fn signed_in(&self, backend: Arc<dyn Backend>, saved: Option<(String, String)>) {
		*self.ctx.backend.borrow_mut() = Some(backend.clone());
		if let Some((email, session)) = saved {
			runtime::spawn_local(async move { runtime::run(async move { keyring::store(&email, &session).await }).await });
		}
		self.mailbox.start();
		self.stack.set_visible_child_name("mailbox");
	}

	fn sign_out(self: &Rc<Self>) {
		let Some(backend) = self.ctx.backend.borrow_mut().take() else { return };
		let email = backend.account().address;
		self.mailbox.clear();
		self.show_login(Some(&email));
		runtime::spawn_local(async move {
			runtime::run(async move {
				let _ = backend.sign_out().await;
				keyring::forget().await;
			})
			.await
		});
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
	let sections: [(&str, &[(&str, &str)]); 3] = [
		(
			"Mail",
			&[
				("New Message", "<Control>n"),
				("Reply", "<Control>r"),
				("Reply All", "<Control><Shift>r"),
				("Forward", "<Control>l"),
				("Move to Trash", "Delete"),
				("Refresh", "F5"),
			],
		),
		("Composer", &[("Send", "<Control>Return"), ("Close", "<Control>w Escape")]),
		("General", &[("Keyboard Shortcuts", "<Control>question"), ("Quit", "<Control>q")]),
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

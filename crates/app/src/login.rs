//! The sign-in page.

use std::cell::RefCell;
use std::rc::Rc;
use std::sync::Arc;

use adw::prelude::*;
use tern_core::{demo::DemoBackend, demo::DEMO_LOGIN, Backend};

use crate::runtime;

type SignedIn = Box<dyn Fn(Arc<dyn Backend>, Option<(String, String)>)>;

#[derive(Clone)]
pub struct Login {
	pub root: adw::ToolbarView,
	email: adw::EntryRow,
	password: adw::PasswordEntryRow,
	button: gtk::Button,
	error: gtk::Label,
	on_signed_in: Rc<RefCell<Option<SignedIn>>>,
}

impl Login {
	pub fn new() -> Login {
		let email = adw::EntryRow::builder().title("Email Address").input_purpose(gtk::InputPurpose::Email).build();
		let password = adw::PasswordEntryRow::builder().title("Password").build();
		let fields = gtk::ListBox::builder().selection_mode(gtk::SelectionMode::None).css_classes(["boxed-list"]).build();
		fields.append(&email);
		fields.append(&password);

		let error = gtk::Label::builder().wrap(true).justify(gtk::Justification::Center).css_classes(["error"]).visible(false).build();
		let button = gtk::Button::builder()
			.label("Sign In")
			.halign(gtk::Align::Center)
			.css_classes(["pill", "suggested-action"])
			.sensitive(false)
			.build();
		let hint = gtk::Label::builder()
			.label("Type “demo” to try Tern without an account")
			.css_classes(["dim-label", "caption"])
			.wrap(true)
			.justify(gtk::Justification::Center)
			.build();

		let column = gtk::Box::new(gtk::Orientation::Vertical, 24);
		column.append(&fields);
		column.append(&error);
		column.append(&button);
		column.append(&hint);
		let clamp = adw::Clamp::builder().maximum_size(360).child(&column).build();

		let status = adw::StatusPage::builder()
			.icon_name(crate::APP_ID)
			.title("Welcome to Tern")
			.description("Sign in with your Tuta account")
			.child(&clamp)
			.css_classes(["icon-dropshadow"])
			.build();
		let scroll = gtk::ScrolledWindow::builder().hscrollbar_policy(gtk::PolicyType::Never).child(&status).build();

		let menu = gtk::gio::Menu::new();
		menu.append(Some("_Keyboard Shortcuts"), Some("app.shortcuts"));
		menu.append(Some("_About Tern"), Some("app.about"));
		let menu_button = gtk::MenuButton::builder().icon_name("open-menu-symbolic").menu_model(&menu).tooltip_text("Main Menu").primary(true).build();
		let header = adw::HeaderBar::builder().show_title(false).build();
		header.pack_end(&menu_button);
		let root = adw::ToolbarView::new();
		root.add_top_bar(&header);
		root.set_content(Some(&scroll));

		let login = Login { root, email, password, button, error, on_signed_in: Rc::default() };
		let l = login.clone();
		login.email.connect_changed(move |_| l.update_button());
		let l = login.clone();
		login.password.connect_changed(move |_| l.update_button());
		let l = login.clone();
		login.email.connect_entry_activated(move |_| {
			if l.is_demo() {
				l.submit();
			} else {
				l.password.grab_focus();
			}
		});
		let l = login.clone();
		login.password.connect_entry_activated(move |_| l.submit());
		let l = login.clone();
		login.button.connect_clicked(move |_| l.submit());
		login
	}

	pub fn connect_signed_in(&self, f: impl Fn(Arc<dyn Backend>, Option<(String, String)>) + 'static) {
		*self.on_signed_in.borrow_mut() = Some(Box::new(f));
	}

	/// Types `email` and presses Sign In, for the screenshot scenes.
	pub fn sign_in_as(&self, email: &str) {
		self.email.set_text(email);
		self.submit();
	}

	fn is_demo(&self) -> bool {
		self.email.text().trim().eq_ignore_ascii_case(DEMO_LOGIN)
	}

	fn update_button(&self) {
		let ready = self.is_demo() || (!self.email.text().trim().is_empty() && !self.password.text().is_empty());
		self.button.set_sensitive(ready && self.button.child().and_downcast::<adw::Spinner>().is_none());
	}

	pub fn reset(&self, email: Option<&str>) {
		self.email.set_text(email.unwrap_or(""));
		self.password.set_text("");
		self.error.set_visible(false);
		self.set_busy(false);
	}

	pub fn focus(&self) {
		if self.email.text().is_empty() {
			self.email.grab_focus();
		} else {
			self.password.grab_focus();
		}
	}

	fn set_busy(&self, busy: bool) {
		if busy {
			self.button.set_child(Some(&adw::Spinner::new()));
		} else {
			self.button.set_label("Sign In");
		}
		self.email.set_sensitive(!busy);
		self.password.set_sensitive(!busy);
		self.update_button();
	}

	fn submit(&self) {
		if !self.button.is_sensitive() {
			return;
		}
		let email = self.email.text().trim().to_string();
		let password = self.password.text().to_string();
		self.error.set_visible(false);
		if self.is_demo() {
			return self.finish(Arc::new(DemoBackend::new()), None);
		}
		self.set_busy(true);
		let this = self.clone();
		runtime::spawn_local(async move {
			let e = email.clone();
			let result = runtime::run(async move { tern_tuta::login(&e, &password).await }).await;
			tern_tuta::trace(if result.is_ok() { "sign-in finished" } else { "sign-in failed" });
			this.set_busy(false);
			match result {
				Ok((backend, session)) => this.finish(backend, Some((email, session))),
				Err(err) => {
					this.error.set_label(&err.to_string());
					this.error.set_visible(true);
					this.password.grab_focus();
				}
			}
		});
	}

	fn finish(&self, backend: Arc<dyn Backend>, saved: Option<(String, String)>) {
		self.password.set_text("");
		if let Some(f) = self.on_signed_in.borrow().as_ref() {
			f(backend, saved);
		}
	}
}

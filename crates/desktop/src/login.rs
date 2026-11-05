//! The sign-in page.

use std::cell::RefCell;
use std::rc::Rc;
use std::sync::Arc;

use adw::prelude::*;
use tern_app::view::{Action, SignInView};
use tern_app::Engine;
use tern_demo::DEMO_LOGIN;

#[derive(Clone)]
pub struct Login {
	pub root: adw::ToolbarView,
	email: adw::EntryRow,
	password: adw::PasswordEntryRow,
	button: gtk::Button,
	error: gtk::Label,
	engine: Arc<Engine>,
	/// What the core last said, so typing is not overwritten by what it has not changed.
	shown: Rc<RefCell<Option<SignInView>>>,
}

impl Login {
	pub fn new(engine: Arc<Engine>) -> Login {
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

		let login = Login { root, email, password, button, error, engine, shown: Rc::default() };
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

	pub fn clear_password(&self) {
		self.password.set_text("");
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
		self.error.set_visible(false);
		self.engine.send(Action::SignIn { email: self.email.text().trim().to_string(), password: self.password.text().to_string() });
	}

	/// Draws what the core says about signing in.
	pub fn update(&self, view: SignInView) {
		let before = self.shown.borrow_mut().replace(view.clone());
		if before.as_ref().map(|b| &b.email) != Some(&view.email) {
			self.email.set_text(&view.email);
		}
		if before.as_ref().map(|b| b.busy) != Some(view.busy) {
			self.set_busy(view.busy);
		}
		match &view.error {
			Some(error) => {
				self.error.set_label(error);
				self.error.set_visible(true);
				if before.as_ref().and_then(|b| b.error.as_ref()) != Some(error) {
					self.password.grab_focus();
				}
			},
			None => self.error.set_visible(false),
		}
	}
}

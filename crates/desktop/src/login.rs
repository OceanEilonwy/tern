//! The sign-in page.

use std::sync::Arc;

use adw::prelude::*;
use relm4::prelude::*;
use tern_app::view::{Action, SignInView};
use tern_app::Engine;
use tern_demo::DEMO_LOGIN;

pub struct Login {
	engine: Arc<Engine>,
	view: SignInView,
	/// The core has given another address, rather than following what is typed.
	email_set: bool,
	email: String,
	password: String,
}

#[derive(Debug)]
pub enum Input {
	View(SignInView),
	Email(String),
	Password(String),
	/// Enter in the address field.
	EmailDone,
	Submit,
	/// The page has gone or come back: nothing typed stays.
	ClearPassword,
	Focus,
}

impl Login {
	fn is_demo(&self) -> bool {
		self.email.trim().eq_ignore_ascii_case(DEMO_LOGIN)
	}

	fn ready(&self) -> bool {
		!self.view.busy && (self.is_demo() || (!self.email.trim().is_empty() && !self.password.is_empty()))
	}
}

#[relm4::component(pub)]
impl Component for Login {
	type Init = Arc<Engine>;
	type Input = Input;
	type Output = ();
	type CommandOutput = ();

	view! {
		adw::ToolbarView {
			add_top_bar = &adw::HeaderBar {
				set_show_title: false,
				pack_end = &gtk::MenuButton {
					set_icon_name: "open-menu-symbolic",
					set_menu_model: Some(&menu),
					set_tooltip_text: Some("Main Menu"),
					set_primary: true,
				},
			},
			#[wrap(Some)]
			set_content = &gtk::ScrolledWindow {
				set_hscrollbar_policy: gtk::PolicyType::Never,
				#[wrap(Some)]
				set_child = &adw::StatusPage {
					set_icon_name: Some(crate::APP_ID),
					set_title: "Welcome to Tern",
					set_description: Some("Sign in with your Tuta account"),
					add_css_class: "icon-dropshadow",
					#[wrap(Some)]
					set_child = &adw::Clamp {
						set_maximum_size: 360,
						#[wrap(Some)]
						set_child = &gtk::Box {
							set_orientation: gtk::Orientation::Vertical,
							set_spacing: 24,
							gtk::ListBox {
								set_selection_mode: gtk::SelectionMode::None,
								add_css_class: "boxed-list",
								#[name = "email"]
								append = &adw::EntryRow {
									set_title: "Email Address",
									set_input_purpose: gtk::InputPurpose::Email,
									#[watch]
									set_sensitive: !model.view.busy,
									#[track = "model.email_set"]
									#[block_signal(email_typed)]
									set_text: &model.view.email,
									connect_changed[sender] => move |row| sender.input(Input::Email(row.text().to_string())) @email_typed,
									connect_entry_activated => Input::EmailDone,
								},
								#[name = "password"]
								append = &adw::PasswordEntryRow {
									set_title: "Password",
									#[watch]
									set_sensitive: !model.view.busy,
									connect_changed[sender] => move |row| sender.input(Input::Password(row.text().to_string())),
									connect_entry_activated => Input::Submit,
								},
							},
							gtk::Label {
								set_wrap: true,
								set_justify: gtk::Justification::Center,
								add_css_class: "error",
								#[watch]
								set_visible: model.view.error.is_some(),
								#[watch]
								set_label: model.view.error.as_deref().unwrap_or_default(),
							},
							gtk::Button {
								set_halign: gtk::Align::Center,
								set_css_classes: &["pill", "suggested-action"],
								#[watch]
								set_sensitive: model.ready(),
								connect_clicked => Input::Submit,
								#[wrap(Some)]
								set_child = &gtk::Box {
									set_halign: gtk::Align::Center,
									adw::Spinner {
										#[watch]
										set_visible: model.view.busy,
									},
									gtk::Label {
										set_label: "Sign In",
										#[watch]
										set_visible: !model.view.busy,
									},
								},
							},
							gtk::Label {
								set_label: "Type “demo” to try Tern without an account",
								set_css_classes: &["dim-label", "caption"],
								set_wrap: true,
								set_justify: gtk::Justification::Center,
							},
						},
					},
				},
			},
		}
	}

	fn init(engine: Self::Init, root: Self::Root, sender: ComponentSender<Self>) -> ComponentParts<Self> {
		let menu = gtk::gio::Menu::new();
		menu.append(Some("_Keyboard Shortcuts"), Some("app.shortcuts"));
		menu.append(Some("_About Tern"), Some("app.about"));
		let model = Login { engine, view: SignInView { email: String::new(), busy: false, error: None }, email_set: false, email: String::new(), password: String::new() };
		let widgets = view_output!();
		ComponentParts { model, widgets }
	}

	fn update_with_view(&mut self, widgets: &mut Self::Widgets, input: Input, sender: ComponentSender<Self>, _: &Self::Root) {
		self.email_set = false;
		match input {
			Input::View(view) => {
				self.email_set = view.email != self.view.email;
				if self.email_set {
					self.email = view.email.clone();
				}
				let new_error = view.error.is_some() && view.error != self.view.error;
				self.view = view;
				if new_error {
					widgets.password.grab_focus();
				}
			},
			Input::Email(email) => self.email = email,
			Input::Password(password) => self.password = password,
			Input::EmailDone if self.is_demo() => sender.input(Input::Submit),
			Input::EmailDone => {
				widgets.password.grab_focus();
			},
			Input::Submit => {
				if self.ready() {
					self.view.error = None;
					self.engine.send(Action::SignIn { email: self.email.trim().to_string(), password: self.password.clone() });
				}
			},
			Input::ClearPassword => widgets.password.set_text(""),
			Input::Focus => {
				if self.email.is_empty() {
					widgets.email.grab_focus();
				} else {
					widgets.password.grab_focus();
				}
			},
		}
		self.update_view(widgets, sender);
	}
}

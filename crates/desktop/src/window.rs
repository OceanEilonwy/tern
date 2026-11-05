//! The main window: the sign-in page or the mailbox, and the app-wide actions.

use std::cell::{Cell, RefCell};
use std::collections::BTreeMap;
use std::rc::Rc;
use std::sync::Arc;

use adw::prelude::*;
use gtk::{gio, glib};
use relm4::prelude::*;
use tern_app::view::{Action, ConnectionKind, ComposerView, Effect, RuleEditorView, RulesView, Screen, SettingsView, SheetView, ShellView, SnackView, Update};
use tern_app::{Engine, Options, SecretStore};
use tern_cache::Cache;
use tern_compose::Fields;
use tern_demo::DemoBackend;
use tern_mail::Backend;

use crate::activity::{Activity, Busy, Word};
use crate::bridge::{Bridge, Incoming};
use crate::composer::Composer;
use crate::keyring::{InMemory, Keyring};
use crate::layout::Layout;
use crate::cleanup::Cleanup;
use crate::preferences::Preferences;
use crate::rule_editor::Rules;
use crate::{attachments, cleanup, labels, reader, login::{self, Login}, mailbox::{self, Mailbox}, rule_editor, screenshot, spam};

type OfferHook = Box<dyn Fn(Option<&str>)>;
/// The composer last asked to throw its mail away, and the dialog asking.
type Discarding = (Option<(u64, adw::Window)>, Option<adw::AlertDialog>);

/// What every part of the window shares: the window, its toasts, the signed-in account and its cache.
#[derive(Clone)]
pub struct Ctx {
	pub window: adw::ApplicationWindow,
	pub toasts: adw::ToastOverlay,
	pub engine: Arc<Engine>,
	/// Preferences, when open: what Clean Up opens over, and where it says it is done.
	preferences: Rc<RefCell<Option<Rc<Controller<Preferences>>>>>,
	/// Clean Up, while open, with its tick for all mail.
	pub cleanup: Rc<RefCell<Option<Rc<Controller<Cleanup>>>>>,
	/// The last message the core gave, so it is said once.
	said: Rc<Cell<u64>>,
	/// What the core last said about the settings.
	settings_view: Rc<RefCell<Option<SettingsView>>>,
	rules_view: Rc<RefCell<Option<RulesView>>>,
	layout: Rc<Cell<Layout>>,
	/// For the screenshot scenes: files that would have gone to another app.
	pub handed_over: Rc<RefCell<Vec<String>>>,
	/// The labels dialog, while open.
	pub label_picker: Rc<RefCell<Option<Rc<Controller<labels::Labels>>>>>,
	/// Move to Spam, while open.
	pub picker: Rc<RefCell<Option<Rc<Controller<spam::Picker>>>>>,
	/// The composers on screen, by the core's id.
	pub composers: Rc<RefCell<BTreeMap<u64, Rc<Composer>>>>,
	/// The composer last asked to throw its mail away, and the dialog asking, once it is up.
	discarding: Rc<RefCell<Discarding>>,
	offer: Rc<RefCell<Option<OfferHook>>>,
	/// What Tern is doing now, in a word.
	pub activity: Activity,
}

impl Ctx {
	/// For the screenshot scenes, which look behind the views.
	pub fn backend(&self) -> Option<Arc<dyn Backend>> {
		self.engine.session_parts().map(|p| p.backend)
	}

	/// For the screenshot scenes, which look behind the views.
	pub fn cache(&self) -> Option<Arc<Cache>> {
		self.engine.session_parts().map(|p| p.cache)
	}

	pub fn settings_view(&self) -> Option<SettingsView> {
		self.settings_view.borrow().clone()
	}

	pub fn rules_view(&self) -> Option<RulesView> {
		self.rules_view.borrow().clone()
	}

	fn show_rules(&self, view: RulesView) {
		*self.rules_view.borrow_mut() = Some(view.clone());
		if let Some(preferences) = self.preferences.borrow().as_ref() {
			preferences.model().rules.emit(rule_editor::Input::View(view));
		}
	}

	fn show_rule_editor(&self, view: Option<RuleEditorView>) {
		if let Some(preferences) = self.preferences.borrow().as_ref() {
			preferences.model().rules.emit(rule_editor::Input::Editor(view.map(Box::new)));
		}
	}

	/// Preferences has closed: unless another has opened since, there is none.
	pub fn forget_preferences(&self, dialog: &adw::PreferencesDialog) {
		let mut open = self.preferences.borrow_mut();
		if open.as_ref().is_some_and(|p| p.widget() == dialog) {
			open.take();
		}
	}

	/// A message from the core, wherever the person is looking.
	fn say(&self, snack: &SnackView) {
		if self.said.replace(snack.id) == snack.id {
			return;
		}
		match self.open_preferences() {
			Some(preferences) => preferences.add_toast(adw::Toast::new(&snack.text)),
			None => self.toast(&snack.text),
		}
		self.engine.send(Action::SnackGone { id: snack.id });
	}

	fn open_preferences(&self) -> Option<adw::PreferencesDialog> {
		self.preferences.borrow().as_ref().map(|p| p.widget().clone()).filter(|d| d.is_mapped())
	}

	/// Clean Up opens when the core offers it, and goes when the core has finished with it.
	fn show_sheet(&self, view: Option<SheetView>) {
		match view {
			Some(SheetView::Spam { revision, line, problem, rows, count, keep_rule, confirm, choices }) => {
				let offer = spam::Offer { revision, line, problem, rows, count, keep_rule, confirm, choices };
				let open = self.picker.borrow().clone();
				match open {
					Some(picker) => picker.emit(offer),
					None => {
						let picker = spam::Picker::builder().launch((self.engine.clone(), offer)).detach();
						picker.widget().present(Some(&self.window));
						*self.picker.borrow_mut() = Some(Rc::new(picker));
					},
				}
			},
			Some(SheetView::Discard) => self.ask_discard(),
			Some(SheetView::Labels { subject, choices }) => {
				let picker = self.label_picker.borrow().clone().unwrap_or_else(|| {
					let picker = labels::Labels::builder().launch(self.engine.clone()).detach();
					picker.widget().present(Some(&self.window));
					Rc::new(picker)
				});
				picker.emit(labels::Offer { subject, choices });
				*self.label_picker.borrow_mut() = Some(picker);
			},
			Some(SheetView::Cleanup { total, choices }) => {
				let controller = match self.open_preferences() {
					Some(preferences) => cleanup::open(&self.engine, total, choices, &preferences),
					None => cleanup::open(&self.engine, total, choices, &self.window),
				};
				*self.cleanup.borrow_mut() = Some(controller);
			},
			_ => {
				if let Some(dialog) = self.cleanup.borrow_mut().take() {
					dialog.widget().force_close();
				}
				if let Some(picker) = self.picker.borrow_mut().take() {
					picker.widget().force_close();
				}
				if let Some(dialog) = self.discarding.borrow_mut().1.take() {
					dialog.force_close();
				}
				if let Some(picker) = self.label_picker.borrow_mut().take() {
					picker.widget().force_close();
				}
			},
		}
	}

	fn show_settings(&self, view: SettingsView) {
		*self.settings_view.borrow_mut() = Some(view.clone());
		if let Some(preferences) = self.preferences.borrow().as_ref() {
			preferences.emit(view);
		}
	}

	pub fn layout(&self) -> Layout {
		self.layout.get()
	}

	/// Changes how the window is laid out; `save` once the change is done, not on every step of a drag.
	pub fn set_layout(&self, layout: Layout, save: bool) {
		self.layout.set(layout);
		if save {
			layout.save(crate::paths().config_dir());
		}
	}

	pub fn toast(&self, text: &str) {
		let toast = adw::Toast::new(text);
		toast.set_timeout(4);
		self.toasts.add_toast(toast);
	}

	/// For the scenes: draws the composers the core has, opening and closing windows to match.
	fn show_composers(&self, views: Vec<ComposerView>) {
		let gone: Vec<Rc<Composer>> = self.composers.borrow().values().filter(|c| !views.iter().any(|v| v.id == c.id)).cloned().collect();
		for composer in gone {
			self.composers.borrow_mut().remove(&composer.id);
			composer.finish();
		}
		// Closed earlier and kept for these: there is nothing left to keep it for.
		if views.is_empty() && !self.window.is_visible() {
			self.window.close();
		}
		for view in &views {
			let known = self.composers.borrow().get(&view.id).cloned();
			match known {
				Some(composer) => composer.show(view),
				None => {
					let composer = Rc::new(Composer::open(self, view));
					self.composers.borrow_mut().insert(view.id, composer);
				},
			}
		}
	}

	/// The composer whose mail may be about to be thrown away, if the core asks.
	pub fn ask_discard_from(&self, id: u64, window: &adw::Window) {
		self.discarding.borrow_mut().0 = Some((id, window.clone()));
	}

	/// For the screenshot scenes: the dialog asking whether to throw a mail away.
	pub fn discard_dialog(&self) -> Option<adw::AlertDialog> {
		self.discarding.borrow().1.clone()
	}

	/// Asks before throwing a composer's mail away, and its saved draft with it.
	fn ask_discard(&self) {
		let Some((id, window)) = self.discarding.borrow().0.clone() else { return };
		if self.discarding.borrow().1.is_some() {
			return;
		}
		let dialog = adw::AlertDialog::new(Some("Discard Message?"), Some("The message and its draft will be deleted."));
		dialog.add_responses(&[("cancel", "_Cancel"), ("discard", "_Discard")]);
		dialog.set_response_appearance("discard", adw::ResponseAppearance::Destructive);
		dialog.set_default_response(Some("cancel"));
		dialog.set_close_response("cancel");
		let engine = self.engine.clone();
		dialog.connect_response(None, move |_, response| {
			engine.send(Action::ComposeFocus { id });
			engine.send(if response == "discard" { Action::ComposeDiscardConfirmed } else { Action::CloseSheet });
		});
		dialog.present(Some(&window));
		self.discarding.borrow_mut().1 = Some(dialog);
	}

	/// Offers, in the sidebar, to clean up the size Tern takes on disk; None takes the offer back.
	pub fn offer_cleanup(&self, size: Option<&str>) {
		if let Some(f) = self.offer.borrow().as_ref() {
			f(size);
		}
	}

	pub fn connect_offer_cleanup(&self, f: impl Fn(Option<&str>) + 'static) {
		*self.offer.borrow_mut() = Some(Box::new(f));
	}
}


/// For the view macro: a widget made ahead, because the model has to know it, laid out in place.
pub fn same<T: Clone>(widget: &T) -> T {
	widget.clone()
}

/// The main window: what comes from the core goes out from here to whichever screen it is about.
pub struct Shell {
	ctx: Ctx,
	login: Rc<Controller<Login>>,
	mailbox: Rc<Controller<Mailbox>>,
	/// Which of starting, login and mailbox is on show.
	page: &'static str,
	/// The account open now, if any.
	address: String,
	/// What the core is doing, for the activity word.
	working: Option<(Word, Busy)>,
	/// The sign-in page has just come up, and takes the focus once it knows what to show.
	focus_login: bool,
}

#[relm4::component(pub)]
impl SimpleComponent for Shell {
	type Init = adw::Application;
	type Input = Incoming;
	type Output = ();

	view! {
		adw::ApplicationWindow {
			set_application: Some(&app),
			set_title: Some("Tern"),
			set_default_width: 1280,
			set_default_height: 800,
			set_size_request: (360, 294),
			// Closing the window while composers are open only hides it: they go on with this core, and
			// asking for the window again brings it back.
			connect_close_request[composers = model.ctx.composers.clone()] => move |window| {
				if composers.borrow().is_empty() {
					return glib::Propagation::Proceed;
				}
				window.set_visible(false);
				glib::Propagation::Stop
			},
			#[wrap(Some)]
			set_content = &same(&toasts) -> adw::ToastOverlay {
				#[wrap(Some)]
				set_child = &gtk::Stack {
					set_transition_type: gtk::StackTransitionType::Crossfade,
					#[watch(skip_init)]
					set_visible_child_name: model.page,
					add_named[Some("starting")] = &adw::ToolbarView {
						add_top_bar = &adw::HeaderBar {
							set_show_title: false,
						},
						#[wrap(Some)]
						set_content = &adw::Spinner {
							set_width_request: 32,
							set_height_request: 32,
							set_halign: gtk::Align::Center,
							set_valign: gtk::Align::Center,
						},
					},
					add_named: (model.login.widget(), Some("login")),
					add_named: (model.mailbox.widget(), Some("mailbox")),
				},
			},
		}
	}

	fn init(app: adw::Application, root: Self::Root, sender: ComponentSender<Self>) -> ComponentParts<Self> {
		let toasts = adw::ToastOverlay::new();
		// Before the core can start, and rewrite the settings' file it may still be in.
		let layout = Layout::load(crate::paths().config_dir());
		let engine = start_engine(sender.input_sender().clone());
		let ctx = Ctx {
			window: root.clone(),
			toasts: toasts.clone(),
			engine: engine.clone(),
			preferences: Rc::default(),
			cleanup: Rc::default(),
			said: Rc::default(),
			settings_view: Rc::default(),
			rules_view: Rc::default(),
			handed_over: Rc::default(),
			label_picker: Rc::default(),
			layout: Rc::new(Cell::new(layout)),
			picker: Rc::default(),
			composers: Rc::default(),
			discarding: Rc::default(),
			offer: Rc::default(),
			activity: Activity::default(),
		};
		let login = Rc::new(Login::builder().launch(engine).detach());
		let mailbox = Rc::new(Mailbox::builder().launch(ctx.clone()).detach());
		let model = Shell { ctx, login, mailbox, page: "starting", address: String::new(), working: None, focus_login: false };
		let widgets = view_output!();
		for breakpoint in crate::mailbox::breakpoints(&model.mailbox) {
			root.add_breakpoint(breakpoint);
		}
		ComponentParts { model, widgets }
	}

	fn update(&mut self, what: Incoming, _: ComponentSender<Self>) {
		match what {
			Incoming::Update(update) => match *update {
				Update::Shell { view } => self.show_shell(view),
				Update::SignIn { view } => {
					self.login.emit(login::Input::View(view));
					if std::mem::take(&mut self.focus_login) {
						self.login.emit(login::Input::Focus);
					}
				},
				Update::Settings { view } => self.ctx.show_settings(view),
				Update::Sheet { view } => self.ctx.show_sheet(view),
				Update::Composers { view } => self.ctx.show_composers(view.open),
				Update::List { view } => self.mailbox.emit(mailbox::Input::List(Box::new(view))),
				Update::Search { view } => self.mailbox.emit(mailbox::Input::Search(Box::new(view))),
				Update::Conversation { view } => self.mailbox.model().reader.emit(reader::Input::Show(view.map(Box::new))),
				Update::Rules { view } => self.ctx.show_rules(view),
				Update::RuleEditor { view } => self.ctx.show_rule_editor(view),
				_ => {},
			},
			Incoming::Effect(effect) => match effect {
				Effect::OpenFile { path, .. } => attachments::open_file(&self.ctx, &path),
				Effect::SaveFile { path, name, .. } => attachments::save_file(&self.ctx, &path, &name),
				Effect::OpenUrl { url } => crate::render::open_uri(&self.ctx.window, &url),
				Effect::ClearNotifications => {},
			},
		}
	}
}

impl Shell {
	fn show_shell(&mut self, view: ShellView) {
		self.account_changed(&view.address);
		self.mailbox.emit(mailbox::Input::Searching(view.stack.contains(&Screen::Search)));
		match view.stack.last() {
			Some(Screen::Starting) | None => {},
			Some(Screen::SignIn) => {
				if self.page != "login" {
					self.login.emit(login::Input::ClearPassword);
					self.page = "login";
					self.ctx.window.set_title(Some("Tern"));
					self.focus_login = true;
				}
			},
			Some(_) => {
				if self.page != "mailbox" {
					self.login.emit(login::Input::ClearPassword);
					self.page = "mailbox";
				}
				// There is nothing here to sign in again from, so go and do it, as ever.
				if view.status.kind == ConnectionKind::SignedOut {
					self.ctx.engine.send(Action::SignInAgain);
				}
			},
		}
		match view.status.kind {
			ConnectionKind::Working => {
				let word = Word::from_label(&view.status.line).unwrap_or(Word::Fetching);
				if self.working.as_ref().map(|(w, _)| *w) != Some(word) {
					self.working = Some((word, self.ctx.activity.start(word)));
				}
			},
			kind => {
				self.working = None;
				self.ctx.activity.set_problem(matches!(kind, ConnectionKind::Offline | ConnectionKind::Busy).then_some(view.status.line));
			},
		}
		self.ctx.offer_cleanup(view.cleanup_offer.as_deref());
		if let Some(snack) = &view.snack {
			self.ctx.say(snack);
		}
	}

	/// An account opened or closed.
	fn account_changed(&mut self, address: &str) {
		if self.address == address {
			return;
		}
		self.address = address.to_string();
		if address.is_empty() {
			self.mailbox.emit(mailbox::Input::Clear);
		} else {
			self.mailbox.emit(mailbox::Input::Start(address.to_string()));
			log::debug!("mailbox shown");
		}
	}
}

/// The main window, for the app and the scenes: the component and what they reach into.
pub struct Window {
	pub ctx: Ctx,
	pub login: Rc<Controller<Login>>,
	pub mailbox: Rc<Controller<Mailbox>>,
	shell: RefCell<Option<Controller<Shell>>>,
}

impl Window {
	pub fn new(app: &adw::Application) -> Rc<Window> {
		let shell = Shell::builder().launch(app.clone()).detach();
		let (ctx, login, mailbox) = {
			let model = shell.model();
			(model.ctx.clone(), model.login.clone(), model.mailbox.clone())
		};
		let window = ctx.window.clone();
		let this = Rc::new(Window { ctx, login, mailbox, shell: RefCell::new(Some(shell)) });
		// The GTK window owns this struct until it is destroyed; callbacks and actions hold only weak references.
		let owner = RefCell::new(Some(this.clone()));
		window.connect_destroy(move |_| {
			if let Some(this) = owner.take() {
				// Not the component while its window is going: once it has gone.
				glib::idle_add_local_once(move || drop(this.shell.take()));
			}
		});
		this.add_actions(app);
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
		if screenshot::scene().is_some() {
			screenshot::drive(this.clone());
		}
		this
	}

	fn add_actions(self: &Rc<Self>, app: &adw::Application) {
		let engine = self.ctx.engine.clone();
		let sign_out = gio::SimpleAction::new("sign-out", None);
		sign_out.connect_activate(move |_, _| engine.send(Action::SignOut));
		app.add_action(&sign_out);

		let window = self.ctx.window.clone();
		let about = gio::SimpleAction::new("about", None);
		about.connect_activate(move |_, _| about_dialog().present(Some(&window)));
		app.add_action(&about);

		let ctx = self.ctx.clone();
		let preferences = gio::SimpleAction::new("preferences", None);
		preferences.connect_activate(move |_, _| preferences_dialog(&ctx).0.present(Some(&ctx.window)));
		app.add_action(&preferences);

		let window = self.ctx.window.clone();
		let shortcuts = gio::SimpleAction::new("shortcuts", None);
		shortcuts.connect_activate(move |_, _| shortcuts_dialog().present(Some(&window)));
		app.add_action(&shortcuts);
	}

	/// Writes a mail with these fields: now if an account is open, else as soon as one is, with the
	/// window up for signing in.
	pub fn compose(&self, fields: Fields) {
		self.ctx.engine.compose(fields);
		self.ctx.window.present();
	}
}

/// The core, for this desktop. The scenes keep to themselves, with no real keyring.
fn start_engine(incoming: relm4::Sender<Incoming>) -> Arc<Engine> {
	let bridge = Arc::new(Bridge(incoming));
	let in_scene = screenshot::scene().is_some();
	let signed_in: Option<Arc<dyn Backend>> = std::env::var_os("TERN_DEMO").map(|_| if in_scene { screenshot::backend() } else { Arc::new(DemoBackend::new()) });
	let secrets: Arc<dyn SecretStore> = if in_scene { Arc::new(InMemory::default()) } else { Arc::new(Keyring) };
	let options = Options { device: "computer", meaning: Some(crate::meaning().clone()), signed_in, retry_hint: "Press F5 to try again.", follow_neighbour: true, lone_draft_composes: false };
	Engine::open_with(crate::paths().clone(), options, secrets, bridge)
}

/// Preferences, with its Rules section.
pub fn preferences_dialog(ctx: &Ctx) -> (adw::PreferencesDialog, Rc<Controller<Rules>>) {
	let preferences = Preferences::builder().launch(ctx.clone()).detach();
	let (dialog, rules) = (preferences.widget().clone(), preferences.model().rules.clone());
	*ctx.preferences.borrow_mut() = Some(Rc::new(preferences));
	(dialog, rules)
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
				("Move to Spam", "<Control>j"),
				("Labels", "<Control><Shift>l"),
				("Conversation Menu", "<Shift>F10"),
				("Check for New Mail", "F5"),
				("Search", "<Control>f"),
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

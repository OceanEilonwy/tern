//! The main window: the sign-in page or the mailbox, and the app-wide actions.

use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::sync::Arc;

use adw::prelude::*;
use gtk::{gio, glib};
use tern_app::view::{Action, ChoiceView, ConnectionKind, Effect, RuleEditorView, RulesView, Screen, SettingsView, SheetView, ShellView, SnackView, Update};
use tern_app::{Engine, EngineListener, Host, Options, SecretStore, SessionParts};
use tern_cache::Cache;
use tern_compose::Fields;
use tern_demo::DemoBackend;
use tern_mail::{Backend, Error, FolderKind};
use tern_sync::fetcher::Fetcher;
use tokio::sync::mpsc::{unbounded_channel, UnboundedReceiver, UnboundedSender};

use crate::activity::{Activity, Busy, Word};
use crate::composer::Composer;
use crate::keyring::{InMemory, Keyring};
use crate::layout::Layout;
use crate::{attachments, cleanup, default_app, login::Login, mailbox::Mailbox, rule_editor, runtime, screenshot, spam};

type OfferHook = Box<dyn Fn(Option<&str>)>;
type SettingsHook = Box<dyn Fn(&SettingsView)>;
type RulesHook = Box<dyn Fn(&RulesView)>;
type RuleEditorHook = Box<dyn Fn(Option<&RuleEditorView>)>;

/// What every part of the window shares: the window, its toasts, the signed-in account and its cache.
#[derive(Clone)]
pub struct Ctx {
	pub window: adw::ApplicationWindow,
	pub toasts: adw::ToastOverlay,
	backend: Rc<RefCell<Option<Arc<dyn Backend>>>>,
	cache: Rc<RefCell<Option<Arc<Cache>>>>,
	fetcher: Rc<RefCell<Option<Fetcher>>>,
	pub engine: Arc<Engine>,
	/// Preferences, when open: what Clean Up opens over, and where it says it is done.
	preferences: Rc<RefCell<Option<glib::WeakRef<adw::PreferencesDialog>>>>,
	/// Clean Up, while open, with its tick for all mail.
	pub cleanup: Rc<RefCell<Option<(adw::AlertDialog, gtk::CheckButton)>>>,
	/// The last message the core gave, so it is said once.
	said: Rc<Cell<u64>>,
	/// What the core last said about the settings.
	settings_view: Rc<RefCell<Option<SettingsView>>>,
	on_settings: Rc<RefCell<Option<SettingsHook>>>,
	on_rules: Rc<RefCell<Option<RulesHook>>>,
	on_rule_editor: Rc<RefCell<Option<RuleEditorHook>>>,
	rules_view: Rc<RefCell<Option<RulesView>>>,
	rule_editor_view: Rc<RefCell<Option<RuleEditorView>>>,
	layout: Rc<Cell<Layout>>,
	/// Move to Spam, while open.
	pub picker: Rc<RefCell<Option<Rc<spam::Picker>>>>,
	offer: Rc<RefCell<Option<OfferHook>>>,
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

	pub fn settings_view(&self) -> Option<SettingsView> {
		self.settings_view.borrow().clone()
	}

	/// Calls `f` whenever the settings on show change, instead of whoever asked last.
	pub fn connect_settings(&self, f: impl Fn(&SettingsView) + 'static) {
		*self.on_settings.borrow_mut() = Some(Box::new(f));
	}

	/// Calls `f` with the rules, now and whenever they change, instead of whoever asked last.
	pub fn connect_rules(&self, f: impl Fn(&RulesView) + 'static) {
		if let Some(view) = self.rules_view.borrow().as_ref() {
			f(view);
		}
		*self.on_rules.borrow_mut() = Some(Box::new(f));
	}

	/// Calls `f` whenever the rule being edited changes, instead of whoever asked last.
	pub fn connect_rule_editor(&self, f: impl Fn(Option<&RuleEditorView>) + 'static) {
		*self.on_rule_editor.borrow_mut() = Some(Box::new(f));
	}

	fn show_rules(&self, view: RulesView) {
		if let Some(f) = self.on_rules.borrow().as_ref() {
			f(&view);
		}
		*self.rules_view.borrow_mut() = Some(view);
	}

	fn show_rule_editor(&self, view: Option<RuleEditorView>) {
		if let Some(f) = self.on_rule_editor.borrow().as_ref() {
			f(view.as_ref());
		}
		*self.rule_editor_view.borrow_mut() = view;
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
		self.preferences.borrow().as_ref().and_then(|d| d.upgrade()).filter(|d| d.is_mapped())
	}

	/// Clean Up opens when the core offers it, and goes when the core has finished with it.
	fn show_sheet(&self, view: Option<SheetView>) {
		match view {
			Some(SheetView::Spam { revision, line, problem, rows, count, keep_rule, confirm, choices }) => {
				let picker = self.picker.borrow().clone().unwrap_or_else(|| spam::open(self));
				picker.show(spam::Offer { revision, line, problem, rows, count, keep_rule, confirm, choices });
				*self.picker.borrow_mut() = Some(picker);
			},
			Some(SheetView::Cleanup { total, choices }) => {
				let (dialog, all) = cleanup::dialog(self, &total, &choices);
				match self.open_preferences() {
					Some(preferences) => dialog.present(Some(&preferences)),
					None => dialog.present(Some(&self.window)),
				}
				*self.cleanup.borrow_mut() = Some((dialog, all));
			},
			_ => {
				if let Some((dialog, _)) = self.cleanup.borrow_mut().take() {
					dialog.force_close();
				}
				if let Some(picker) = self.picker.borrow_mut().take() {
					picker.dialog.force_close();
				}
			},
		}
	}

	fn show_settings(&self, view: SettingsView) {
		if let Some(f) = self.on_settings.borrow().as_ref() {
			f(&view);
		}
		*self.settings_view.borrow_mut() = Some(view);
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

	pub fn error(&self, err: &Error) {
		self.toast(&err.to_string());
	}

	/// Called by a composer once its mail is sent.
	pub fn sent(&self, subject: &str) {
		self.toast(&tern_app::format::sent_toast(subject));
		self.changed(&[FolderKind::Sent, FolderKind::Drafts]);
	}

	/// Mail in folders of these kinds changed on the server, as after saving a draft.
	pub fn changed(&self, kinds: &[FolderKind]) {
		self.engine.sync_kinds(kinds);
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


pub struct Window {
	pub ctx: Ctx,
	stack: gtk::Stack,
	pub login: Login,
	pub mailbox: Rc<Mailbox>,
	/// Mail asked for before there was an account to send it from, to write once signed in.
	waiting: RefCell<Vec<Fields>>,
	/// What the core is doing, for the activity word.
	working: RefCell<Option<(Word, Busy)>>,
	/// The sign-in page has just come up, and takes the focus once it knows what to show.
	focus_login: Cell<bool>,
}

impl Window {
	pub fn new(app: &adw::Application) -> Rc<Window> {
		let window = adw::ApplicationWindow::builder().application(app).title("Tern").default_width(1280).default_height(800).build();
		window.set_size_request(360, 294);
		let toasts = adw::ToastOverlay::new();
		// Before the core can start, and rewrite the settings' file it may still be in.
		let layout = Layout::load(crate::paths().config_dir());
		let (engine, incoming) = start_engine();
		let ctx = Ctx {
			window: window.clone(),
			toasts: toasts.clone(),
			engine: engine.clone(),
			preferences: Rc::default(),
			cleanup: Rc::default(),
			said: Rc::default(),
			settings_view: Rc::default(),
			on_settings: Rc::default(),
			on_rules: Rc::default(),
			on_rule_editor: Rc::default(),
			rules_view: Rc::default(),
			rule_editor_view: Rc::default(),
			backend: Rc::default(),
			cache: Rc::default(),
			fetcher: Rc::default(),
			layout: Rc::new(Cell::new(layout)),
			picker: Rc::default(),
			offer: Rc::default(),
			activity: Activity::default(),
		};

		let stack = gtk::Stack::builder().transition_type(gtk::StackTransitionType::Crossfade).build();
		let starting = adw::Spinner::builder().width_request(32).height_request(32).halign(gtk::Align::Center).valign(gtk::Align::Center).build();
		let starting_page = adw::ToolbarView::new();
		starting_page.add_top_bar(&adw::HeaderBar::builder().show_title(false).build());
		starting_page.set_content(Some(&starting));
		stack.add_named(&starting_page, Some("starting"));

		let login = Login::new(engine.clone());
		stack.add_named(&login.root, Some("login"));
		let mailbox = Mailbox::new(ctx.clone());
		stack.add_named(&mailbox.root, Some("mailbox"));
		toasts.set_child(Some(&stack));
		window.set_content(Some(&toasts));
		for bp in mailbox.breakpoints() {
			window.add_breakpoint(bp);
		}

		let this = Rc::new(Window { ctx, stack, login, mailbox, waiting: RefCell::default(), working: RefCell::default(), focus_login: Cell::new(false) });
		// The GTK window owns this struct until it is destroyed; callbacks and actions hold only weak references.
		let owner = RefCell::new(Some(this.clone()));
		window.connect_destroy(move |_| drop(owner.take()));
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
		// Holds only a weak reference: the window's life is its own.
		let weak = Rc::downgrade(&this);
		runtime::spawn_local(async move {
			let mut incoming = incoming;
			while let Some(what) = incoming.recv().await {
				let Some(this) = weak.upgrade() else { return };
				this.take(what);
			}
		});
		if screenshot::scene().is_some() {
			screenshot::drive(this.clone());
		}
		this
	}

	fn add_actions(self: &Rc<Self>, app: &adw::Application) {
		let weak = Rc::downgrade(self);
		let sign_out = gio::SimpleAction::new("sign-out", None);
		sign_out.connect_activate(move |_, _| {
			if let Some(this) = weak.upgrade() {
				this.ctx.engine.send(Action::SignOut);
			}
		});
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
		if self.ctx.cache().is_some() {
			Composer::open(&self.ctx, fields, None);
		} else {
			self.waiting.borrow_mut().push(fields);
			self.ctx.window.present();
		}
	}

	/// Takes what the core says, on the GTK thread.
	fn take(self: &Rc<Self>, what: Incoming) {
		match what {
			Incoming::Update(update) => match *update {
				Update::Shell { view } => self.show_shell(view),
				Update::SignIn { view } => {
					self.login.update(view);
					if self.focus_login.take() {
						self.login.focus();
					}
				},
				Update::Settings { view } => self.ctx.show_settings(view),
				Update::Sheet { view } => self.ctx.show_sheet(view),
				Update::List { view } => self.mailbox.show_list(view),
				Update::Search { view } => self.mailbox.show_search(view),
				Update::Conversation { view } => self.mailbox.reader.show(view.as_ref()),
				Update::Rules { view } => self.ctx.show_rules(view),
				Update::RuleEditor { view } => self.ctx.show_rule_editor(view),
				_ => {},
			},
			Incoming::Session(parts) => self.session_changed(parts),
			Incoming::Effect(effect) => match effect {
				Effect::OpenFile { path, .. } => attachments::open_file(&self.ctx, &path),
				Effect::SaveFile { path, name, .. } => attachments::save_file(&self.ctx, &path, &name),
				Effect::OpenUrl { url } => crate::render::open_uri(&self.ctx.window, &url),
				Effect::ClearNotifications => {},
			},
		}
	}

	fn show_shell(self: &Rc<Self>, view: ShellView) {
		self.mailbox.set_searching(view.stack.contains(&Screen::Search));
		match view.stack.last() {
			Some(Screen::Starting) | None => {},
			Some(Screen::SignIn) => {
				if self.stack.visible_child_name().as_deref() != Some("login") {
					self.login.clear_password();
					self.stack.set_visible_child_name("login");
					self.ctx.window.set_title(Some("Tern"));
					self.focus_login.set(true);
				}
			},
			Some(_) => {
				if self.stack.visible_child_name().as_deref() != Some("mailbox") {
					self.login.clear_password();
					self.stack.set_visible_child_name("mailbox");
				}
				// There is nothing here to sign in again from, so go and do it, as ever.
				if view.status.kind == ConnectionKind::SignedOut {
					self.ctx.engine.send(Action::SignInAgain);
				}
			},
		}
		let mut working = self.working.borrow_mut();
		match view.status.kind {
			ConnectionKind::Working => {
				let word = Word::from_label(&view.status.line).unwrap_or(Word::Fetching);
				if working.as_ref().map(|(w, _)| *w) != Some(word) {
					*working = Some((word, self.ctx.activity.start(word)));
				}
			},
			kind => {
				working.take();
				self.ctx.activity.set_problem(matches!(kind, ConnectionKind::Offline | ConnectionKind::Busy).then_some(view.status.line));
			},
		}
		self.ctx.offer_cleanup(view.cleanup_offer.as_deref());
		if let Some(snack) = &view.snack {
			self.ctx.say(snack);
		}
	}

	fn session_changed(self: &Rc<Self>, parts: Option<SessionParts>) {
		let Some(parts) = parts else {
			*self.ctx.backend.borrow_mut() = None;
			*self.ctx.cache.borrow_mut() = None;
			*self.ctx.fetcher.borrow_mut() = None;
			self.mailbox.clear();
			return;
		};
		// The same account again once it has connected, with the same cache and a real backend.
		let opening = self.ctx.cache.borrow().is_none();
		*self.ctx.backend.borrow_mut() = Some(parts.backend);
		*self.ctx.fetcher.borrow_mut() = Some(parts.fetcher);
		*self.ctx.cache.borrow_mut() = Some(parts.cache);
		if opening {
			self.mailbox.start();
			for fields in self.waiting.take() {
				Composer::open(&self.ctx, fields, None);
			}
			log::debug!("mailbox shown");
		}
	}
}

/// What comes from the core, in order, for the GTK thread.
enum Incoming {
	Update(Box<Update>),
	Effect(Effect),
	Session(Option<SessionParts>),
}

struct Bridge(UnboundedSender<Incoming>);

impl EngineListener for Bridge {
	fn on_update(&self, update: Update) {
		let _ = self.0.send(Incoming::Update(Box::new(update)));
	}
	fn on_effect(&self, effect: Effect) {
		let _ = self.0.send(Incoming::Effect(effect));
	}
	fn on_log(&self, _: String) {}
}

impl Host for Bridge {
	fn session(&self, parts: Option<SessionParts>) {
		let _ = self.0.send(Incoming::Session(parts));
	}
}

/// The core, for this desktop. The scenes keep to themselves, with no real keyring.
fn start_engine() -> (Arc<Engine>, UnboundedReceiver<Incoming>) {
	let (tx, incoming) = unbounded_channel();
	let bridge = Arc::new(Bridge(tx));
	let listener = bridge.clone();
	let in_scene = screenshot::scene().is_some();
	let signed_in: Option<Arc<dyn Backend>> = std::env::var_os("TERN_DEMO").map(|_| if in_scene { screenshot::backend() } else { Arc::new(DemoBackend::new()) });
	let secrets: Arc<dyn SecretStore> = if in_scene { Arc::new(InMemory::default()) } else { Arc::new(Keyring) };
	let options = Options { device: "computer", meaning: Some(crate::meaning().clone()), signed_in, host: Some(bridge), retry_hint: "Press F5 to try again.", follow_neighbour: true, lone_draft_composes: false };
	(Engine::open_with(crate::paths().clone(), options, secrets, listener), incoming)
}

/// Preferences, with its Rules section.
pub fn preferences_dialog(ctx: &Ctx) -> (adw::PreferencesDialog, Rc<rule_editor::Rules>) {
	// Tall enough for a rule's page without scrolling.
	let dialog = adw::PreferencesDialog::builder().content_height(680).build();
	let page = adw::PreferencesPage::new();
	let (search, meaning) = search_group(ctx);
	let rules = rule_editor::Rules::new(ctx, &dialog);
	let group = adw::PreferencesGroup::builder()
		.title("Offline Mail")
		.description("Tern keeps recent mail on this computer, encrypted, so it shows at once and can be read offline. It checks for new mail every few minutes.")
		.build();
	let view = ctx.settings_view().unwrap_or_else(|| panic!("the core publishes settings at once"));
	group.add(&choice_row(ctx, "Keep Ready", None, &view.keep_ready, |value| Action::SetKeepReady { value }));
	let forget = adw::SwitchRow::builder()
		.title("Forget Mail on Sign Out")
		.subtitle("Deletes the mail kept on this computer when you sign out. Signing in again downloads it afresh.")
		.active(view.forget_on_sign_out)
		.build();
	forget.connect_active_notify({
		let ctx = ctx.clone();
		move |row| ctx.engine.send(Action::SetForget { on: row.is_active() })
	});
	group.add(&forget);
	group.add(&choice_row(ctx, "Offer Cleanup Past", Some("The sidebar offers to clean up once mail here takes more than this."), &view.cleanup_limits, |value| Action::SetCleanupLimit { value }));
	let storage = storage_row(ctx, &view.storage_line);
	group.add(&storage);
	// The download's progress, the switch going back off if it fails, and the size as it changes.
	let (meaning, storage) = (meaning.downgrade(), storage.downgrade());
	ctx.connect_settings(move |view| {
		if let Some(row) = meaning.upgrade() {
			row.set_subtitle(&view.semantic_line);
			if row.is_active() != view.semantic {
				row.set_active(view.semantic);
			}
		}
		if let Some(row) = storage.upgrade() {
			row.set_subtitle(&view.storage_line);
		}
	});
	*ctx.preferences.borrow_mut() = Some(dialog.downgrade());
	page.add(&group);
	page.add(&search);
	page.add(&default_app::group());
	page.add(&rules.group);
	dialog.add(&page);
	// The dialog keeps its rules section alive while open.
	let keep = RefCell::new(Some(rules.clone()));
	let engine = ctx.engine.clone();
	dialog.connect_closed(move |_| {
		keep.borrow_mut().take();
		engine.send(Action::CloseSettings);
	});
	(dialog, rules)
}

/// One of the choices the core offers.
fn choice_row(ctx: &Ctx, title: &str, subtitle: Option<&str>, choices: &[ChoiceView], action: fn(String) -> Action) -> adw::ComboRow {
	let names: Vec<&str> = choices.iter().map(|c| c.label.as_str()).collect();
	let row = adw::ComboRow::builder().title(title).model(&gtk::StringList::new(&names)).build();
	if let Some(subtitle) = subtitle {
		row.set_subtitle(subtitle);
	}
	row.set_selected(choices.iter().position(|c| c.selected).unwrap_or(0) as u32);
	let (ctx, values): (_, Vec<String>) = (ctx.clone(), choices.iter().map(|c| c.value.clone()).collect());
	row.connect_selected_notify(move |row| {
		if let Some(value) = values.get(row.selected() as usize) {
			ctx.engine.send(action(value.clone()));
		}
	});
	row
}

/// How much Tern takes on disk, with Clean Up always to hand whatever the size.
fn storage_row(ctx: &Ctx, line: &str) -> adw::ActionRow {
	let button = gtk::Button::builder().label("Clean Up…").valign(gtk::Align::Center).build();
	let row = adw::ActionRow::builder().title("Storage").subtitle(line).visible(ctx.cache().is_some()).build();
	row.add_suffix(&button);
	let engine = ctx.engine.clone();
	button.connect_clicked(move |_| engine.send(Action::OpenCleanup));
	row
}

/// Search by meaning is off until asked for, as it downloads a model once.
fn search_group(ctx: &Ctx) -> (adw::PreferencesGroup, adw::SwitchRow) {
	let group = adw::PreferencesGroup::builder().title("Search").description("Search looks through the mail kept on this computer.").build();
	let view = ctx.settings_view().unwrap_or_else(|| panic!("the core publishes settings at once"));
	let row = adw::SwitchRow::builder().title("Search by Meaning").subtitle(view.semantic_line).active(view.semantic).build();
	let engine = ctx.engine.clone();
	row.connect_active_notify(move |row| engine.send(Action::SetSemantic { on: row.is_active() }));
	group.add(&row);
	(group, row)
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

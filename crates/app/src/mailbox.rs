//! The signed-in view: folders, the conversation list and the reading pane.
//!
//! Everything shown comes from the cache, so switching folders is instant; the folder on show is
//! brought up to date in the background while a small spinner in its header turns.

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use adw::prelude::*;
use gtk::{gdk, gio, glib};
use tern_cache::{sync, ThreadMail, ThreadRow};
use tern_core::{Folder, FolderId, FolderKind, MailId, MailSummary};

use crate::compose_model::{Answer, Fields};
use crate::reader::Reader;
use crate::{composer, format, runtime, window::Ctx};

/// Conversations taken from the cache at a time.
const ROWS: usize = 100;

type MailAction = fn(&Rc<Mailbox>);

pub struct Mailbox {
	ctx: Ctx,
	pub root: gtk::Widget,
	outer: adw::OverlaySplitView,
	inner: adw::NavigationSplitView,
	account: adw::WindowTitle,
	folder_list: gtk::ListBox,
	folders: RefCell<Vec<(Folder, gtk::Label)>>,
	current: RefCell<Option<Folder>>,
	list_title: adw::WindowTitle,
	list_stack: gtk::Stack,
	empty: adw::StatusPage,
	syncing: adw::Spinner,
	pub store: gio::ListStore,
	selection: gtk::SingleSelection,
	list_view: gtk::ListView,
	/// How many conversations the list asks the cache for; grows when scrolled to the end.
	shown: Cell<usize>,
	fetching_older: Cell<bool>,
	/// Folders being brought up to date now.
	in_flight: RefCell<Vec<FolderId>>,
	/// Set while the list is swapped under an open conversation, so the reader stays.
	hold: Cell<bool>,
	/// The last selection came from the keyboard: on a narrow window it does not leave the list.
	by_keyboard: Cell<bool>,
	pub reader: Rc<Reader>,
}

pub fn folder_icon(kind: FolderKind) -> &'static str {
	match kind {
		FolderKind::Inbox => "tern-inbox-symbolic",
		FolderKind::Drafts => "document-edit-symbolic",
		FolderKind::Sent => "mail-send-symbolic",
		FolderKind::Archive => "tern-archive-symbolic",
		FolderKind::Spam => "mail-mark-junk-symbolic",
		FolderKind::Trash => "user-trash-symbolic",
		FolderKind::Custom => "folder-symbolic",
	}
}

fn row_of(obj: &glib::Object) -> ThreadRow {
	obj.downcast_ref::<glib::BoxedAnyObject>().unwrap().borrow::<ThreadRow>().clone()
}

/// One line of the conversation list.
fn row_widget() -> gtk::Box {
	let dot = gtk::Image::builder().icon_name("media-record-symbolic").pixel_size(8).css_classes(["accent"]).valign(gtk::Align::Center).build();
	dot.set_tooltip_text(Some("Unread"));
	let sender = gtk::Label::builder().xalign(0.0).hexpand(true).ellipsize(gtk::pango::EllipsizeMode::End).build();
	let count = gtk::Label::builder().css_classes(["caption", "numeric", "thread-count"]).valign(gtk::Align::Center).build();
	let clip = gtk::Image::builder().icon_name("mail-attachment-symbolic").css_classes(["dim-label"]).build();
	let date = gtk::Label::builder().css_classes(["caption", "dim-label", "numeric"]).build();
	let top = gtk::Box::new(gtk::Orientation::Horizontal, 6);
	top.append(&dot);
	top.append(&sender);
	top.append(&count);
	top.append(&clip);
	top.append(&date);
	let subject = gtk::Label::builder().xalign(0.0).ellipsize(gtk::pango::EllipsizeMode::End).build();
	let row = gtk::Box::builder().orientation(gtk::Orientation::Vertical).spacing(3).margin_top(9).margin_bottom(9).margin_start(6).margin_end(6).build();
	row.append(&top);
	row.append(&subject);
	row
}

fn bind_row(row: &gtk::Box, t: &ThreadRow, now: i64, tz: &glib::TimeZone) {
	let m = &t.latest;
	let top = row.first_child().and_downcast::<gtk::Box>().unwrap();
	let subject = top.next_sibling().and_downcast::<gtk::Label>().unwrap();
	let dot = top.first_child().unwrap();
	let sender = dot.next_sibling().and_downcast::<gtk::Label>().unwrap();
	let count = sender.next_sibling().and_downcast::<gtk::Label>().unwrap();
	let clip = count.next_sibling().unwrap();
	let date = clip.next_sibling().and_downcast::<gtk::Label>().unwrap();
	dot.set_visible(t.unread);
	let who = match (&m.to, m.outgoing) {
		(Some(to), true) => format!("To: {}", to.display()),
		(None, true) => "(No Recipients)".to_string(),
		_ => if m.from.display().is_empty() { "(Unknown Sender)" } else { m.from.display() }.to_string(),
	};
	if m.draft {
		sender.set_markup(&format!("<span foreground=\"#e01b24\">Draft</span>  {}", glib::markup_escape_text(&who)));
	} else {
		sender.set_label(&who);
	}
	if t.unread {
		sender.add_css_class("heading");
	} else {
		sender.remove_css_class("heading");
	}
	count.set_label(&t.count.to_string());
	count.set_visible(t.count > 1);
	count.set_tooltip_text(Some(&format!("{} messages", t.count)));
	clip.set_visible(m.has_attachments);
	date.set_label(&format::short_date(m.received_ms, now, tz));
	subject.set_label(if m.subject.trim().is_empty() { "(No Subject)" } else { m.subject.as_str() });
	if t.unread {
		subject.remove_css_class("dim-label");
	} else {
		subject.add_css_class("dim-label");
	}
}

impl Mailbox {
	pub fn new(ctx: Ctx) -> Rc<Mailbox> {
		// Folders.
		let account = adw::WindowTitle::new("Tern", "");
		let menu = gio::Menu::new();
		let section = gio::Menu::new();
		section.append(Some("_Preferences"), Some("app.preferences"));
		section.append(Some("_Keyboard Shortcuts"), Some("app.shortcuts"));
		section.append(Some("_About Tern"), Some("app.about"));
		menu.append_section(None, &section);
		let section = gio::Menu::new();
		section.append(Some("_Sign Out"), Some("app.sign-out"));
		menu.append_section(None, &section);
		let menu_button = gtk::MenuButton::builder().icon_name("open-menu-symbolic").menu_model(&menu).tooltip_text("Main Menu").primary(true).build();
		let folder_header = adw::HeaderBar::builder().title_widget(&account).build();
		folder_header.pack_end(&menu_button);
		let folder_list = gtk::ListBox::builder().css_classes(["navigation-sidebar"]).build();
		folder_list.update_property(&[gtk::accessible::Property::Label("Folders")]);
		let folder_scroll = gtk::ScrolledWindow::builder().hscrollbar_policy(gtk::PolicyType::Never).child(&folder_list).vexpand(true).build();
		let folder_view = adw::ToolbarView::new();
		folder_view.add_top_bar(&folder_header);
		folder_view.set_content(Some(&folder_scroll));

		// Conversation list.
		let store = gio::ListStore::new::<glib::BoxedAnyObject>();
		let selection = gtk::SingleSelection::builder().model(&store).autoselect(false).can_unselect(true).build();
		let factory = gtk::SignalListItemFactory::new();
		factory.connect_setup(|_, item| {
			let item = item.downcast_ref::<gtk::ListItem>().unwrap();
			item.set_child(Some(&row_widget()));
		});
		let tz = glib::TimeZone::local();
		factory.connect_bind(move |_, item| {
			let item = item.downcast_ref::<gtk::ListItem>().unwrap();
			let (Some(row), Some(obj)) = (item.child().and_downcast::<gtk::Box>(), item.item()) else { return };
			bind_row(&row, &row_of(&obj), format::now_ms(), &tz);
		});
		let list_view = gtk::ListView::builder().model(&selection).factory(&factory).css_classes(["navigation-sidebar"]).build();
		list_view.update_property(&[gtk::accessible::Property::Label("Conversations")]);
		let list_scroll = gtk::ScrolledWindow::builder().hscrollbar_policy(gtk::PolicyType::Never).child(&list_view).vexpand(true).build();
		let empty = adw::StatusPage::builder().icon_name("mail-read-symbolic").title("No Messages").css_classes(["compact"]).build();
		let spinner = adw::Spinner::builder().width_request(32).height_request(32).halign(gtk::Align::Center).valign(gtk::Align::Center).build();
		let list_stack = gtk::Stack::builder().transition_type(gtk::StackTransitionType::Crossfade).transition_duration(100).build();
		list_stack.add_named(&spinner, Some("loading"));
		list_stack.add_named(&empty, Some("empty"));
		list_stack.add_named(&list_scroll, Some("list"));

		let list_title = adw::WindowTitle::new("Inbox", "");
		let list_header = adw::HeaderBar::builder().title_widget(&list_title).build();
		let sidebar_toggle = gtk::ToggleButton::builder().icon_name("sidebar-show-symbolic").tooltip_text("Show Folders").build();
		let compose_content = adw::ButtonContent::builder().icon_name("mail-message-new-symbolic").label("_New").use_underline(true).build();
		let compose = gtk::Button::builder().child(&compose_content).action_name("win.compose").tooltip_text("New Message (Ctrl+N)").css_classes(["suggested-action"]).build();
		let syncing = adw::Spinner::builder().tooltip_text("Checking for new mail").visible(false).build();
		let refresh = gtk::Button::builder().icon_name("view-refresh-symbolic").action_name("win.refresh").tooltip_text("Check for New Mail (F5)").build();
		list_header.pack_start(&sidebar_toggle);
		list_header.pack_start(&compose);
		list_header.pack_end(&refresh);
		list_header.pack_end(&syncing);
		let list_view_tb = adw::ToolbarView::new();
		list_view_tb.add_top_bar(&list_header);
		list_view_tb.set_content(Some(&list_stack));
		let list_page = adw::NavigationPage::builder().title("Inbox").tag("list").child(&list_view_tb).build();

		let reader = Reader::new(ctx.clone());
		let inner = adw::NavigationSplitView::builder()
			.sidebar(&list_page)
			.content(&reader.page)
			.min_sidebar_width(300.0)
			.max_sidebar_width(420.0)
			.sidebar_width_fraction(0.38)
			.build();
		let inner_page = adw::NavigationPage::builder().title("Mail").child(&inner).build();
		let folder_page = adw::NavigationPage::builder().title("Folders").child(&folder_view).build();
		let outer = adw::OverlaySplitView::builder()
			.sidebar(&folder_page)
			.content(&inner_page)
			.min_sidebar_width(200.0)
			.max_sidebar_width(260.0)
			.sidebar_width_fraction(0.2)
			.build();
		outer.bind_property("collapsed", &sidebar_toggle, "visible").sync_create().build();
		outer.bind_property("show-sidebar", &sidebar_toggle, "active").sync_create().bidirectional().build();

		let this = Rc::new(Mailbox {
			ctx,
			root: outer.clone().upcast(),
			outer,
			inner,
			account,
			folder_list,
			folders: RefCell::default(),
			current: RefCell::default(),
			list_title,
			list_stack,
			empty,
			syncing,
			store,
			selection,
			list_view,
			shown: Cell::new(ROWS),
			fetching_older: Cell::new(false),
			in_flight: RefCell::default(),
			hold: Cell::new(false),
			by_keyboard: Cell::new(false),
			reader,
		});
		this.connect_signals(&list_scroll);
		let weak = Rc::downgrade(&this);
		this.reader.connect_read(move |summary| {
			if let Some(this) = weak.upgrade() {
				this.set_unread(summary, false);
			}
		});
		let weak = Rc::downgrade(&this);
		this.reader.connect_current(move || {
			if let Some(this) = weak.upgrade() {
				this.update_actions();
			}
		});
		let weak = Rc::downgrade(&this);
		this.reader.connect_discard(move |summary| {
			if let Some(this) = weak.upgrade() {
				this.trash(vec![summary.id.clone()], "Draft discarded");
			}
		});
		let weak = Rc::downgrade(&this);
		this.ctx.connect_changed(move |kinds| {
			if let Some(this) = weak.upgrade() {
				this.sync_kinds(kinds);
			}
		});
		this.add_actions();
		this
	}

	/// Folders collapse into an overlay first, then list and reader into one column.
	pub fn breakpoints(&self) -> Vec<adw::Breakpoint> {
		let medium = adw::Breakpoint::new(adw::BreakpointCondition::new_length(adw::BreakpointConditionLengthType::MaxWidth, 900.0, adw::LengthUnit::Sp));
		medium.add_setter(&self.outer, "collapsed", Some(&true.to_value()));
		let narrow = adw::Breakpoint::new(adw::BreakpointCondition::new_length(adw::BreakpointConditionLengthType::MaxWidth, 600.0, adw::LengthUnit::Sp));
		narrow.add_setter(&self.outer, "collapsed", Some(&true.to_value()));
		narrow.add_setter(&self.inner, "collapsed", Some(&true.to_value()));
		vec![medium, narrow]
	}

	fn connect_signals(self: &Rc<Self>, list_scroll: &gtk::ScrolledWindow) {
		let weak = Rc::downgrade(self);
		self.folder_list.connect_row_selected(move |_, row| {
			let (Some(this), Some(row)) = (weak.upgrade(), row) else { return };
			let folder = this.folders.borrow().get(row.index() as usize).map(|(f, _)| f.clone());
			if let Some(folder) = folder {
				if this.current.borrow().as_ref().map(|f| &f.id) != Some(&folder.id) {
					this.open_folder(&folder);
				}
				if this.outer.is_collapsed() {
					this.outer.set_show_sidebar(false);
				}
			}
		});
		// Enter on a folder moves on to its conversations.
		let weak = Rc::downgrade(self);
		self.folder_list.connect_row_activated(move |_, _| {
			if let Some(this) = weak.upgrade() {
				this.focus_list();
			}
		});

		let weak = Rc::downgrade(self);
		self.selection.connect_selection_changed(move |sel, _, _| {
			let Some(this) = weak.upgrade() else { return };
			if this.hold.get() {
				return;
			}
			match sel.selected_item() {
				Some(obj) => {
					this.open_thread(&row_of(&obj));
					if !this.by_keyboard.get() {
						this.inner.set_show_content(true);
					}
				},
				None => {
					this.reader.clear();
					this.update_actions();
				},
			}
			this.by_keyboard.set(false);
		});
		// Arrow keys move through the list without leaving it, even on a narrow window.
		let keys = gtk::EventControllerKey::new();
		keys.set_propagation_phase(gtk::PropagationPhase::Capture);
		let weak = Rc::downgrade(self);
		keys.connect_key_pressed(move |_, key, _, _| {
			if let Some(this) = weak.upgrade() {
				if matches!(key, gdk::Key::Up | gdk::Key::Down | gdk::Key::Page_Up | gdk::Key::Page_Down | gdk::Key::Home | gdk::Key::End) {
					this.by_keyboard.set(true);
				}
			}
			glib::Propagation::Proceed
		});
		self.list_view.add_controller(keys);
		// Enter (or a click on the selected row) opens the conversation and moves focus into it.
		let weak = Rc::downgrade(self);
		self.list_view.connect_activate(move |_, _| {
			let Some(this) = weak.upgrade() else { return };
			if this.current.borrow().as_ref().is_some_and(|f| f.kind == FolderKind::Drafts) && this.reader.edit_current_draft().is_some() {
				return;
			}
			this.inner.set_show_content(true);
			this.reader.focus();
		});
		// Going back in the narrow layout drops the selection, so tapping the same mail opens it again.
		let weak = Rc::downgrade(self);
		self.inner.connect_show_content_notify(move |inner| {
			let Some(this) = weak.upgrade() else { return };
			if inner.is_collapsed() && !inner.shows_content() {
				this.hold.set(true);
				this.selection.set_selected(gtk::INVALID_LIST_POSITION);
				this.hold.set(false);
			}
		});
		// Escape in the reader goes back to the list.
		let keys = gtk::EventControllerKey::new();
		let weak = Rc::downgrade(self);
		keys.connect_key_pressed(move |_, key, _, _| {
			let Some(this) = weak.upgrade() else { return glib::Propagation::Proceed };
			if key == gdk::Key::Escape {
				this.inner.set_show_content(false);
				this.focus_list();
				return glib::Propagation::Stop;
			}
			glib::Propagation::Proceed
		});
		self.reader.page.add_controller(keys);

		let weak = Rc::downgrade(self);
		list_scroll.connect_edge_reached(move |_, pos| {
			if pos == gtk::PositionType::Bottom {
				if let Some(this) = weak.upgrade() {
					this.load_more();
				}
			}
		});
	}

	fn add_actions(self: &Rc<Self>) {
		let entries: [(&str, MailAction); 11] = [
			("compose", |m| {
				composer::Composer::open(&m.ctx, Fields::default(), None);
			}),
			("reply", |m| {
				m.reader.answer(Answer::Reply);
			}),
			("reply-all", |m| {
				m.reader.answer(Answer::ReplyAll);
			}),
			("forward", |m| {
				m.reader.answer(Answer::Forward);
			}),
			("trash", |m| m.trash_conversation()),
			("refresh", |m| m.refresh()),
			("mark-unread", |m| m.mark_unread()),
			("open-in-tuta", |m| {
				if let Some(url) = m.reader.web_url() {
					crate::render::open_uri(&m.ctx.window, &url);
				}
			}),
			("next", |m| m.step(1)),
			("previous", |m| m.step(-1)),
			("focus-list", |m| m.focus_list()),
		];
		for (name, f) in entries {
			let action = gio::SimpleAction::new(name, None);
			let weak = Rc::downgrade(self);
			action.connect_activate(move |_, _| {
				if let Some(this) = weak.upgrade() {
					f(&this);
				}
			});
			self.ctx.window.add_action(&action);
		}
		// Ctrl+1…9 open the folders in order.
		let folder = gio::SimpleAction::new("folder", Some(glib::VariantTy::INT32));
		let weak = Rc::downgrade(self);
		folder.connect_activate(move |_, param| {
			let (Some(this), Some(i)) = (weak.upgrade(), param.and_then(|p| p.get::<i32>())) else { return };
			if let Some(row) = this.folder_list.row_at_index(i) {
				this.folder_list.select_row(Some(&row));
				this.focus_list();
			}
		});
		self.ctx.window.add_action(&folder);
		self.set_enabled(false);
	}

	fn action(&self, name: &str) -> gio::SimpleAction {
		self.ctx.window.lookup_action(name).and_downcast::<gio::SimpleAction>().unwrap()
	}

	fn set_enabled(&self, signed_in: bool) {
		for name in ["compose", "refresh", "next", "previous", "focus-list", "folder"] {
			self.action(name).set_enabled(signed_in);
		}
		self.update_actions();
	}

	/// Mail actions work only while a conversation is open.
	fn update_actions(&self) {
		let open = self.reader.current.borrow().is_some() && self.ctx.backend().is_some();
		let answerable = open && self.reader.current.borrow().as_ref().is_some_and(|o| !o.mail.summary.draft);
		for name in ["reply", "reply-all", "forward"] {
			self.action(name).set_enabled(answerable);
		}
		for name in ["trash", "mark-unread", "open-in-tuta"] {
			self.action(name).set_enabled(open || self.reader.thread().is_some());
		}
	}

	/// Shows the cached mailbox at once; the background sync fills in the rest.
	pub fn start(self: &Rc<Self>) {
		let Some(backend) = self.ctx.backend() else { return };
		let account = backend.account();
		self.account.set_subtitle(&account.address);
		self.ctx.window.set_title(Some(&format!("Tern — {}", account.address)));
		self.set_enabled(true);
		let cached = self.ctx.cache().and_then(|c| c.folders().ok()).unwrap_or_default();
		if cached.is_empty() {
			self.list_stack.set_visible_child_name("loading");
		} else {
			self.set_folders(cached, true);
		}
	}

	pub fn clear(&self) {
		self.folder_list.remove_all();
		self.folders.borrow_mut().clear();
		*self.current.borrow_mut() = None;
		self.in_flight.borrow_mut().clear();
		self.syncing.set_visible(false);
		self.store.remove_all();
		self.reader.clear();
		self.set_enabled(false);
	}

	/// The cache changed: refresh folder counts and the list. A change in any folder can touch the
	/// list on show, as conversations span folders; unchanged rows stay as they are.
	pub fn cache_changed(self: &Rc<Self>, _folder: Option<&FolderId>) {
		let Some(cache) = self.ctx.cache() else { return };
		if let Ok(folders) = cache.folders() {
			if !folders.is_empty() {
				self.set_folders(folders, false);
			}
		}
		if self.current.borrow().is_some() {
			self.reload_list();
		}
	}

	/// Shows that the first sync of the folder list failed, when there is nothing cached to show.
	pub fn sync_failed(&self, err: &tern_core::Error) {
		if self.folders.borrow().is_empty() {
			self.list_title.set_title("");
			self.show_status("Could Not Load Folders", Some(&format!("{err}. Press F5 to try again.")));
		}
	}

	fn set_folders(self: &Rc<Self>, folders: Vec<Folder>, select_inbox: bool) {
		let current = self.current.borrow().as_ref().map(|f| f.id.clone());
		let unchanged = {
			let old = self.folders.borrow();
			old.len() == folders.len() && old.iter().zip(&folders).all(|((a, _), b)| a.id == b.id && a.name == b.name)
		};
		if unchanged {
			// Only counts changed: update labels in place, keep the selection.
			let mut old = self.folders.borrow_mut();
			for ((f, label), new) in old.iter_mut().zip(folders) {
				*f = new;
				set_count(label, f.unread);
				if Some(&f.id) == current.as_ref() {
					self.list_title.set_subtitle(&unread_subtitle(f.unread));
				}
			}
			return;
		}
		self.folder_list.remove_all();
		let mut rows = Vec::new();
		for (i, folder) in folders.into_iter().enumerate() {
			let icon = gtk::Image::from_icon_name(folder_icon(folder.kind));
			let name = gtk::Label::builder().label(&folder.name).xalign(0.0).hexpand(true).ellipsize(gtk::pango::EllipsizeMode::End).build();
			let count = gtk::Label::builder().css_classes(["dim-label", "numeric", "caption-heading"]).build();
			set_count(&count, folder.unread);
			let row_box = gtk::Box::builder().spacing(12).margin_start(6).margin_end(6).margin_top(3).margin_bottom(3).build();
			row_box.append(&icon);
			row_box.append(&name);
			row_box.append(&count);
			if i < 9 {
				row_box.set_tooltip_text(Some(&format!("Ctrl+{}", i + 1)));
			}
			self.folder_list.append(&row_box);
			rows.push((folder, count));
		}
		*self.folders.borrow_mut() = rows;
		if self.folders.borrow().is_empty() {
			return self.show_status("No Folders", None);
		}
		let target = {
			let folders = self.folders.borrow();
			let by_current = current.and_then(|c| folders.iter().position(|(f, _)| f.id == c));
			let inbox = folders.iter().position(|(f, _)| f.kind == FolderKind::Inbox).or(Some(0));
			if select_inbox { inbox } else { by_current.or(inbox) }
		};
		if let Some(row) = target.and_then(|i| self.folder_list.row_at_index(i as i32)) {
			self.folder_list.select_row(Some(&row));
		}
	}

	/// Shows a status page in place of the list.
	fn show_status(&self, title: &str, description: Option<&str>) {
		self.empty.set_title(title);
		self.empty.set_description(description);
		self.list_stack.set_visible_child_name("empty");
	}

	fn change_title(&self, folder: &Folder) {
		self.list_title.set_title(&folder.name);
		self.list_title.set_subtitle(&unread_subtitle(folder.unread));
	}

	fn open_folder(self: &Rc<Self>, folder: &Folder) {
		*self.current.borrow_mut() = Some(folder.clone());
		self.change_title(folder);
		self.hold.set(true);
		self.selection.set_selected(gtk::INVALID_LIST_POSITION);
		self.store.remove_all();
		self.hold.set(false);
		self.reader.clear();
		self.update_actions();
		self.inner.set_show_content(false);
		self.shown.set(ROWS);
		self.reload_list();
		self.sync_folder(&folder.id);
	}

	/// Shows the current folder's conversations as the cache has them, keeping the selection.
	fn reload_list(self: &Rc<Self>) {
		let (Some(cache), Some(folder)) = (self.ctx.cache(), self.current.borrow().clone()) else { return };
		let rows = cache.threads(&folder.id, self.shown.get()).unwrap_or_default();
		let selected = self.selection.selected_item().map(|o| row_of(&o).key);
		let position = self.selection.selected();
		let items: Vec<glib::BoxedAnyObject> = rows.iter().cloned().map(glib::BoxedAnyObject::new).collect();
		// Rows that did not change keep their widgets.
		let same = self.store.n_items() as usize == rows.len()
			&& (0..self.store.n_items()).all(|i| self.store.item(i).is_some_and(|o| row_of(&o) == rows[i as usize]));
		if !same {
			self.hold.set(true);
			self.store.splice(0, self.store.n_items(), &items);
			let again = selected.as_ref().and_then(|k| rows.iter().position(|r| &r.key == k));
			self.selection.set_selected(again.map_or(gtk::INVALID_LIST_POSITION, |i| i as u32));
			self.hold.set(false);
			if selected.is_some() && again.is_none() {
				// The open conversation left this folder: show its neighbour.
				let next = position.min(self.store.n_items().saturating_sub(1));
				if self.store.n_items() > 0 && position != gtk::INVALID_LIST_POSITION {
					self.by_keyboard.set(true);
					self.selection.set_selected(next);
				} else {
					self.reader.clear();
				}
			}
		}
		// The open conversation may have new mail.
		if let Some(obj) = self.selection.selected_item() {
			self.open_thread(&row_of(&obj));
		}
		let synced = cache.folder_state(&folder.id).map(|s| s.synced_ms > 0).unwrap_or(false);
		if !rows.is_empty() {
			self.list_stack.set_visible_child_name("list");
		} else if synced || !self.in_flight.borrow().contains(&folder.id) {
			self.show_status("No Messages", None);
		} else {
			self.list_stack.set_visible_child_name("loading");
		}
		self.update_actions();
	}

	/// Brings `folder` up to date now, ahead of the background sync.
	pub fn sync_folder(self: &Rc<Self>, folder: &FolderId) {
		let (Some(backend), Some(cache)) = (self.ctx.backend(), self.ctx.cache()) else { return };
		if self.in_flight.borrow().contains(folder) {
			return;
		}
		self.in_flight.borrow_mut().push(folder.clone());
		self.update_syncing();
		let keep = self.ctx.settings().offline.headers();
		let (this, id) = (self.clone(), folder.clone());
		runtime::spawn_local(async move {
			let i = id.clone();
			let b = backend.clone();
			let result = runtime::run(async move { sync::folder(b.as_ref(), &cache, &i, keep).await }).await;
			this.in_flight.borrow_mut().retain(|f| f != &id);
			this.update_syncing();
			// Signed out, or into another account, meanwhile.
			if !this.ctx.backend().is_some_and(|now| std::sync::Arc::ptr_eq(&now, &backend)) {
				return;
			}
			match result {
				Ok(_) => this.cache_changed(Some(&id)),
				Err(e) => {
					let current = this.current.borrow().as_ref().map(|f| f.id.clone());
					if current.as_ref() == Some(&id) && this.store.n_items() == 0 {
						this.show_status("Could Not Load Messages", Some(&format!("{e}. Press F5 to try again.")));
					} else if current.as_ref() == Some(&id) {
						this.ctx.error(&e);
					}
				},
			}
		});
	}

	fn update_syncing(&self) {
		let current = self.current.borrow().as_ref().map(|f| f.id.clone());
		let busy = current.is_some_and(|c| self.in_flight.borrow().contains(&c)) || self.fetching_older.get();
		self.syncing.set_visible(busy);
	}

	/// Brings the folders of these kinds up to date, as after sending or saving a draft.
	fn sync_kinds(self: &Rc<Self>, kinds: &[FolderKind]) {
		let ids: Vec<FolderId> = self.folders.borrow().iter().filter(|(f, _)| kinds.contains(&f.kind)).map(|(f, _)| f.id.clone()).collect();
		for id in ids {
			self.sync_folder(&id);
		}
	}

	fn load_more(self: &Rc<Self>) {
		let (Some(cache), Some(backend), Some(folder)) = (self.ctx.cache(), self.ctx.backend(), self.current.borrow().clone()) else { return };
		if self.store.n_items() as usize >= self.shown.get() {
			// The cache has more than shown.
			self.shown.set(self.shown.get() + ROWS);
			self.reload_list();
			if self.store.n_items() as usize >= self.shown.get() - ROWS + 1 {
				return;
			}
		}
		if self.fetching_older.get() || cache.folder_state(&folder.id).map(|s| s.complete).unwrap_or(true) {
			return;
		}
		self.fetching_older.set(true);
		self.update_syncing();
		let this = self.clone();
		runtime::spawn_local(async move {
			let id = folder.id.clone();
			let result = runtime::run(async move { sync::older(backend.as_ref(), &cache, &id).await }).await;
			this.fetching_older.set(false);
			this.update_syncing();
			match result {
				Ok(_) => {
					this.shown.set(this.shown.get() + ROWS);
					this.cache_changed(Some(&folder.id));
				},
				Err(e) => this.ctx.error(&e),
			}
		});
	}

	fn refresh(self: &Rc<Self>) {
		self.ctx.sync_now();
		let current = self.current.borrow().as_ref().map(|f| f.id.clone());
		match current {
			Some(id) => self.sync_folder(&id),
			None => self.list_stack.set_visible_child_name("loading"),
		}
	}

	fn open_thread(self: &Rc<Self>, row: &ThreadRow) {
		let (Some(cache), Some(folder)) = (self.ctx.cache(), self.current.borrow().clone()) else { return };
		let mut mails = cache.thread(&row.key, &folder.id).unwrap_or_default();
		if mails.is_empty() {
			mails.push(ThreadMail { summary: row.latest.clone(), folder: folder.kind, folder_id: folder.id.clone() });
		}
		self.reader.show_thread(&row.key, &folder.id, mails, &row.latest.id);
		self.update_actions();
	}

	/// Moves the selection by `delta` conversations, from anywhere in the window.
	fn step(self: &Rc<Self>, delta: i32) {
		let n = self.store.n_items() as i32;
		if n == 0 {
			return;
		}
		let at = self.selection.selected();
		let next = if at == gtk::INVALID_LIST_POSITION { 0 } else { (at as i32 + delta).clamp(0, n - 1) };
		self.by_keyboard.set(true);
		self.selection.set_selected(next as u32);
		self.list_view.scroll_to(next as u32, gtk::ListScrollFlags::FOCUS, None);
	}

	fn focus_list(&self) {
		if self.store.n_items() == 0 {
			return;
		}
		let at = match self.selection.selected() {
			gtk::INVALID_LIST_POSITION => 0,
			at => at,
		};
		self.list_view.scroll_to(at, gtk::ListScrollFlags::FOCUS, None);
	}

	/// Marks a mail read or unread on the server and in the cache.
	fn set_unread(self: &Rc<Self>, summary: &MailSummary, unread: bool) {
		let (Some(backend), Some(cache)) = (self.ctx.backend(), self.ctx.cache()) else { return };
		let _ = cache.set_unread(&summary.id, unread);
		self.cache_changed(None);
		let id = summary.id.clone();
		let ctx = self.ctx.clone();
		runtime::spawn_local(async move {
			let i = id.clone();
			if let Err(e) = runtime::run(async move { backend.set_unread(&i, unread).await }).await {
				let _ = cache.set_unread(&id, !unread);
				ctx.error(&e);
			}
		});
	}

	fn mark_unread(self: &Rc<Self>) {
		let target = self.reader.current.borrow().as_ref().map(|o| o.mail.summary.clone());
		if let Some(summary) = target {
			// Keep it unread: leave the conversation, or opening it marks it read again.
			self.hold.set(true);
			self.selection.set_selected(gtk::INVALID_LIST_POSITION);
			self.hold.set(false);
			self.reader.clear();
			self.set_unread(&summary, true);
			self.focus_list();
		}
	}

	/// Moves the open conversation's mails in this folder to Trash.
	fn trash_conversation(self: &Rc<Self>) {
		let (Some(cache), Some((key, folder))) = (self.ctx.cache(), self.reader.thread()) else { return };
		let ids: Vec<MailId> = cache.thread(&key, &folder).unwrap_or_default().into_iter().filter(|m| m.folder_id == folder).map(|m| m.summary.id).collect();
		let message = if ids.len() > 1 { "Conversation moved to Trash" } else { "Moved to Trash" };
		self.trash(ids, message);
	}

	fn trash(self: &Rc<Self>, ids: Vec<MailId>, message: &'static str) {
		let (Some(backend), Some(cache)) = (self.ctx.backend(), self.ctx.cache()) else { return };
		if ids.is_empty() {
			return;
		}
		let trash = self.folders.borrow().iter().find(|(f, _)| f.kind == FolderKind::Trash).map(|(f, _)| f.id.clone());
		for id in &ids {
			let _ = match &trash {
				Some(t) => cache.move_to(id, t),
				None => cache.remove(id),
			};
		}
		self.cache_changed(None);
		self.focus_list();
		let this = self.clone();
		runtime::spawn_local(async move {
			let result = runtime::run(async move {
				for id in &ids {
					backend.trash(id).await?;
				}
				Ok::<_, tern_core::Error>(())
			})
			.await;
			match result {
				Ok(()) => this.ctx.toast(message),
				Err(e) => {
					this.ctx.error(&e);
					this.refresh();
				},
			}
		});
	}

	/// For the screenshot scenes: opens the first folder of a kind.
	pub fn select_folder(&self, kind: FolderKind) {
		let index = self.folders.borrow().iter().position(|(f, _)| f.kind == kind);
		if let Some(row) = index.and_then(|i| self.folder_list.row_at_index(i as i32)) {
			self.folder_list.select_row(Some(&row));
		}
	}

	/// For the screenshot scenes: selects the first conversation whose subject contains `text`.
	pub fn select_by_subject(&self, text: &str) -> bool {
		let found = (0..self.store.n_items()).find(|&i| self.store.item(i).is_some_and(|o| row_of(&o).latest.subject.contains(text)));
		if let Some(i) = found {
			self.selection.set_selected(i);
		}
		found.is_some()
	}

	/// For the screenshot scenes: the title of the status page shown instead of the list, if one is.
	pub fn status(&self) -> Option<String> {
		(self.list_stack.visible_child_name().as_deref() == Some("empty")).then(|| self.empty.title().to_string())
	}

	/// For the screenshot scenes: a folder is being brought up to date.
	pub fn is_loading(&self) -> bool {
		!self.in_flight.borrow().is_empty() || self.fetching_older.get()
	}
}

fn set_count(label: &gtk::Label, unread: u32) {
	label.set_label(&unread.to_string());
	label.set_visible(unread > 0);
}

fn unread_subtitle(unread: u32) -> String {
	match unread {
		0 => String::new(),
		n => format!("{n} unread"),
	}
}

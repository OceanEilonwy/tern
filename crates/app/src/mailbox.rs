//! The signed-in view: folders, the mail list and the reading pane.

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use adw::prelude::*;
use gtk::{gio, glib};
use tern_core::{Folder, FolderId, FolderKind, MailSummary};

use crate::compose_model::{Answer, Fields};
use crate::html_doc::Doc;
use crate::reader::{Opened, Reader};
use crate::{composer, format, runtime, window::Ctx};

const PAGE: usize = 50;

type MailAction = fn(&Rc<Mailbox>);

pub struct Mailbox {
	ctx: Ctx,
	pub root: gtk::Widget,
	outer: adw::OverlaySplitView,
	inner: adw::NavigationSplitView,
	account: adw::WindowTitle,
	folder_list: gtk::ListBox,
	folders: RefCell<Vec<(Folder, gtk::Label)>>,
	current: RefCell<Option<FolderId>>,
	list_title: adw::WindowTitle,
	list_stack: gtk::Stack,
	empty: adw::StatusPage,
	pub store: gio::ListStore,
	selection: gtk::SingleSelection,
	list_view: gtk::ListView,
	next: RefCell<Option<String>>,
	loading_more: Cell<bool>,
	generation: Cell<u64>,
	/// Set while the list is swapped under an open mail, so the reader stays.
	hold: Cell<bool>,
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

fn summary_of(obj: &glib::Object) -> MailSummary {
	obj.downcast_ref::<glib::BoxedAnyObject>().unwrap().borrow::<MailSummary>().clone()
}

/// One line of the mail list.
fn row_widget() -> gtk::Box {
	let dot = gtk::Image::builder().icon_name("media-record-symbolic").pixel_size(8).css_classes(["accent"]).valign(gtk::Align::Center).build();
	dot.set_tooltip_text(Some("Unread"));
	let sender = gtk::Label::builder().xalign(0.0).hexpand(true).ellipsize(gtk::pango::EllipsizeMode::End).build();
	let clip = gtk::Image::builder().icon_name("mail-attachment-symbolic").css_classes(["dim-label"]).build();
	let date = gtk::Label::builder().css_classes(["caption", "dim-label", "numeric"]).build();
	let top = gtk::Box::new(gtk::Orientation::Horizontal, 6);
	top.append(&dot);
	top.append(&sender);
	top.append(&clip);
	top.append(&date);
	let subject = gtk::Label::builder().xalign(0.0).ellipsize(gtk::pango::EllipsizeMode::End).build();
	let row = gtk::Box::builder().orientation(gtk::Orientation::Vertical).spacing(3).margin_top(9).margin_bottom(9).margin_start(6).margin_end(6).build();
	row.append(&top);
	row.append(&subject);
	row
}

fn bind_row(row: &gtk::Box, m: &MailSummary, now: i64, tz: &glib::TimeZone) {
	let top = row.first_child().and_downcast::<gtk::Box>().unwrap();
	let subject = top.next_sibling().and_downcast::<gtk::Label>().unwrap();
	let dot = top.first_child().unwrap();
	let sender = dot.next_sibling().and_downcast::<gtk::Label>().unwrap();
	let clip = sender.next_sibling().unwrap();
	let date = clip.next_sibling().and_downcast::<gtk::Label>().unwrap();
	dot.set_visible(m.unread);
	sender.set_label(if m.from.display().is_empty() { "(Unknown Sender)" } else { m.from.display() });
	if m.unread {
		sender.add_css_class("heading");
	} else {
		sender.remove_css_class("heading");
	}
	clip.set_visible(m.has_attachments);
	date.set_label(&format::short_date(m.received_ms, now, tz));
	subject.set_label(if m.subject.trim().is_empty() { "(No Subject)" } else { m.subject.as_str() });
	if m.unread {
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
		let folder_scroll = gtk::ScrolledWindow::builder().hscrollbar_policy(gtk::PolicyType::Never).child(&folder_list).vexpand(true).build();
		let folder_view = adw::ToolbarView::new();
		folder_view.add_top_bar(&folder_header);
		folder_view.set_content(Some(&folder_scroll));

		// Mail list.
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
			bind_row(&row, &summary_of(&obj), format::now_ms(), &tz);
		});
		let list_view = gtk::ListView::builder().model(&selection).factory(&factory).css_classes(["navigation-sidebar"]).build();
		list_view.update_property(&[gtk::accessible::Property::Label("Messages")]);
		let list_scroll = gtk::ScrolledWindow::builder().hscrollbar_policy(gtk::PolicyType::Never).child(&list_view).vexpand(true).build();
		let empty = adw::StatusPage::builder().icon_name("mail-read-symbolic").title("No Messages").css_classes(["compact"]).build();
		let spinner = adw::Spinner::builder().width_request(32).height_request(32).halign(gtk::Align::Center).valign(gtk::Align::Center).build();
		let list_stack = gtk::Stack::builder().transition_type(gtk::StackTransitionType::Crossfade).transition_duration(120).build();
		list_stack.add_named(&spinner, Some("loading"));
		list_stack.add_named(&empty, Some("empty"));
		list_stack.add_named(&list_scroll, Some("list"));

		let list_title = adw::WindowTitle::new("Inbox", "");
		let list_header = adw::HeaderBar::builder().title_widget(&list_title).build();
		let sidebar_toggle = gtk::ToggleButton::builder().icon_name("sidebar-show-symbolic").tooltip_text("Show Folders").build();
		let compose = gtk::Button::builder().icon_name("mail-message-new-symbolic").action_name("win.compose").tooltip_text("New Message").build();
		let refresh = gtk::Button::builder().icon_name("view-refresh-symbolic").action_name("win.refresh").tooltip_text("Refresh").build();
		list_header.pack_start(&sidebar_toggle);
		list_header.pack_start(&compose);
		list_header.pack_end(&refresh);
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
			store,
			selection,
			list_view,
			next: RefCell::default(),
			loading_more: Cell::new(false),
			generation: Cell::new(0),
			hold: Cell::new(false),
			reader,
		});
		this.connect_signals(&list_scroll);
		// A sent mail shows up in Sent, and its counts change.
		let weak = Rc::downgrade(&this);
		this.ctx.connect_sent(move || {
			let Some(this) = weak.upgrade() else { return };
			let current = this.current.borrow().clone();
			let in_sent = this.folders.borrow().iter().any(|(f, _)| Some(&f.id) == current.as_ref() && f.kind == FolderKind::Sent);
			if in_sent {
				this.refresh();
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
			let index = row.index() as usize;
			let folder = this.folders.borrow().get(index).map(|(f, _)| f.clone());
			if let Some(folder) = folder {
				if this.current.borrow().as_ref() != Some(&folder.id) {
					this.open_folder(&folder);
				}
				if this.outer.is_collapsed() {
					this.outer.set_show_sidebar(false);
				}
			}
		});

		let weak = Rc::downgrade(self);
		self.selection.connect_selection_changed(move |sel, _, _| {
			let Some(this) = weak.upgrade() else { return };
			if this.hold.get() {
				return;
			}
			match sel.selected_item() {
				Some(obj) => this.open_mail(summary_of(&obj)),
				None => {
					this.reader.clear();
					this.update_actions();
				}
			}
		});
		// Activating the selected row again (narrow layout) shows it again.
		let weak = Rc::downgrade(self);
		self.list_view.connect_activate(move |_, _| {
			if let Some(this) = weak.upgrade() {
				this.inner.set_show_content(true);
			}
		});
		// Going back in the narrow layout drops the selection, so tapping the same mail opens it again.
		let weak = Rc::downgrade(self);
		self.inner.connect_show_content_notify(move |inner| {
			let Some(this) = weak.upgrade() else { return };
			if inner.is_collapsed() && !inner.shows_content() {
				this.selection.set_selected(gtk::INVALID_LIST_POSITION);
			}
		});

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
		let entries: [(&str, MailAction); 8] = [
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
			("trash", |m| m.trash_selected()),
			("refresh", |m| m.refresh()),
			("mark-unread", |m| m.mark_unread()),
			("open-in-tuta", |m| {
				if let Some(url) = m.reader.web_url() {
					crate::render::open_uri(&m.ctx.window, &url);
				}
			}),
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
		self.set_enabled(false);
	}

	fn action(&self, name: &str) -> gio::SimpleAction {
		self.ctx.window.lookup_action(name).and_downcast::<gio::SimpleAction>().unwrap()
	}

	fn set_enabled(&self, signed_in: bool) {
		for name in ["compose", "refresh"] {
			self.action(name).set_enabled(signed_in);
		}
		self.update_actions();
	}

	/// Mail actions work only while a mail is open.
	fn update_actions(&self) {
		let open = self.reader.current.borrow().is_some() && self.ctx.backend().is_some();
		for name in ["reply", "reply-all", "forward", "trash", "mark-unread", "open-in-tuta"] {
			self.action(name).set_enabled(open);
		}
	}

	pub fn start(self: &Rc<Self>) {
		let Some(backend) = self.ctx.backend() else { return };
		let account = backend.account();
		self.account.set_subtitle(&account.address);
		self.ctx.window.set_title(Some(&format!("Tern — {}", account.address)));
		self.set_enabled(true);
		self.load_folders(true);
	}

	pub fn clear(&self) {
		self.generation.set(self.generation.get() + 1);
		self.folder_list.remove_all();
		self.folders.borrow_mut().clear();
		*self.current.borrow_mut() = None;
		self.store.remove_all();
		self.reader.clear();
		self.set_enabled(false);
	}

	fn load_folders(self: &Rc<Self>, select_inbox: bool) {
		let Some(backend) = self.ctx.backend() else { return };
		let this = self.clone();
		runtime::spawn_local(async move {
			match runtime::run(async move { backend.folders().await }).await {
				Ok(folders) => this.set_folders(folders, select_inbox),
				Err(e) => this.ctx.error(&e),
			}
		});
	}

	fn set_folders(self: &Rc<Self>, folders: Vec<Folder>, select_inbox: bool) {
		let current = self.current.borrow().clone();
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
			}
			return;
		}
		self.folder_list.remove_all();
		let mut rows = Vec::new();
		for folder in folders {
			let icon = gtk::Image::from_icon_name(folder_icon(folder.kind));
			let name = gtk::Label::builder().label(&folder.name).xalign(0.0).hexpand(true).ellipsize(gtk::pango::EllipsizeMode::End).build();
			let count = gtk::Label::builder().css_classes(["dim-label", "numeric", "caption-heading"]).build();
			set_count(&count, folder.unread);
			let row_box = gtk::Box::builder().spacing(12).margin_start(6).margin_end(6).margin_top(3).margin_bottom(3).build();
			row_box.append(&icon);
			row_box.append(&name);
			row_box.append(&count);
			self.folder_list.append(&row_box);
			rows.push((folder, count));
		}
		*self.folders.borrow_mut() = rows;
		let target = {
			let folders = self.folders.borrow();
			let by_current = current.and_then(|c| folders.iter().position(|(f, _)| f.id == c));
			let inbox = folders.iter().position(|(f, _)| f.kind == FolderKind::Inbox).or(if folders.is_empty() { None } else { Some(0) });
			if select_inbox { inbox } else { by_current.or(inbox) }
		};
		if let Some(row) = target.and_then(|i| self.folder_list.row_at_index(i as i32)) {
			self.folder_list.select_row(Some(&row));
		}
	}

	fn change_unread(&self, delta: i32) {
		let current = self.current.borrow().clone();
		let mut folders = self.folders.borrow_mut();
		if let Some((f, label)) = folders.iter_mut().find(|(f, _)| Some(&f.id) == current.as_ref()) {
			f.unread = (f.unread as i32 + delta).max(0) as u32;
			set_count(label, f.unread);
			self.list_title.set_subtitle(&unread_subtitle(f.unread));
		}
	}

	fn open_folder(self: &Rc<Self>, folder: &Folder) {
		*self.current.borrow_mut() = Some(folder.id.clone());
		self.list_title.set_title(&folder.name);
		self.list_title.set_subtitle(&unread_subtitle(folder.unread));
		self.selection.set_selected(gtk::INVALID_LIST_POSITION);
		self.reader.clear();
		self.update_actions();
		self.inner.set_show_content(false);
		self.store.remove_all();
		self.list_stack.set_visible_child_name("loading");
		self.load_page(None);
	}

	/// Loads a page of the current folder; `next` None starts over.
	fn load_page(self: &Rc<Self>, next: Option<String>) {
		let (Some(backend), Some(folder)) = (self.ctx.backend(), self.current.borrow().clone()) else { return };
		let restart = next.is_none();
		let generation = if restart {
			self.generation.set(self.generation.get() + 1);
			self.generation.get()
		} else {
			self.generation.get()
		};
		self.loading_more.set(true);
		let this = self.clone();
		runtime::spawn_local(async move {
			let result = runtime::run(async move { backend.list(&folder, next, PAGE).await }).await;
			if this.generation.get() != generation {
				return;
			}
			this.loading_more.set(false);
			match result {
				Ok(page) => {
					let items: Vec<glib::BoxedAnyObject> = page.mails.into_iter().map(glib::BoxedAnyObject::new).collect();
					if restart {
						this.store.splice(0, this.store.n_items(), &items);
					} else {
						this.store.extend_from_slice(&items);
					}
					*this.next.borrow_mut() = page.next;
					this.empty.set_title("No Messages");
					this.empty.set_description(None);
					this.list_stack.set_visible_child_name(if this.store.n_items() == 0 { "empty" } else { "list" });
				}
				Err(e) => {
					if this.store.n_items() == 0 {
						this.empty.set_title("Could Not Load Messages");
						this.empty.set_description(Some(&e.to_string()));
						this.list_stack.set_visible_child_name("empty");
					} else {
						this.ctx.error(&e);
					}
				}
			}
		});
	}

	fn load_more(self: &Rc<Self>) {
		if self.loading_more.get() {
			return;
		}
		let next = self.next.borrow().clone();
		if let Some(next) = next {
			self.load_page(Some(next));
		}
	}

	/// Reloads folder counts and the list, keeping the open mail selected if it is still there.
	fn refresh(self: &Rc<Self>) {
		if self.current.borrow().is_none() {
			return self.load_folders(true);
		}
		self.load_folders(false);
		let Some(backend) = self.ctx.backend() else { return };
		let Some(folder) = self.current.borrow().clone() else { return };
		self.generation.set(self.generation.get() + 1);
		let generation = self.generation.get();
		let open = self.reader.current.borrow().as_ref().map(|o| o.mail.summary.id.clone());
		let this = self.clone();
		runtime::spawn_local(async move {
			let result = runtime::run(async move { backend.list(&folder, None, PAGE).await }).await;
			if this.generation.get() != generation {
				return;
			}
			match result {
				Ok(page) => {
					let selected = page.mails.iter().position(|m| Some(&m.id) == open.as_ref());
					let items: Vec<glib::BoxedAnyObject> = page.mails.into_iter().map(glib::BoxedAnyObject::new).collect();
					// The reader keeps what it shows while the list is swapped underneath.
					this.hold.set(true);
					this.store.splice(0, this.store.n_items(), &items);
					this.selection.set_selected(selected.map_or(gtk::INVALID_LIST_POSITION, |i| i as u32));
					this.hold.set(false);
					*this.next.borrow_mut() = page.next;
					this.list_stack.set_visible_child_name(if this.store.n_items() == 0 { "empty" } else { "list" });
				}
				Err(e) => this.ctx.error(&e),
			}
		});
	}

	fn open_mail(self: &Rc<Self>, summary: MailSummary) {
		if self.reader.current.borrow().as_ref().is_some_and(|o| o.mail.summary.id == summary.id) {
			self.inner.set_show_content(true);
			return;
		}
		let Some(backend) = self.ctx.backend() else { return };
		let generation = self.reader.loading();
		self.inner.set_show_content(true);
		let this = self.clone();
		let id = summary.id.clone();
		runtime::spawn_local(async move {
			let b = backend.clone();
			let result = runtime::run(async move {
				let mail = b.open(&id).await?;
				let doc = Doc::parse(&mail.body_html);
				Ok::<_, tern_core::Error>(Opened { mail, doc })
			})
			.await;
			if !this.reader.is_current(generation) {
				return;
			}
			match result {
				Ok(opened) => {
					this.reader.show(opened);
					this.update_actions();
					if summary.unread {
						this.set_unread(&summary, false);
					}
				}
				Err(e) => {
					this.reader.clear();
					this.update_actions();
					this.ctx.error(&e);
				}
			}
		});
	}

	fn position_of(&self, id: &tern_core::MailId) -> Option<u32> {
		(0..self.store.n_items()).find(|&i| self.store.item(i).is_some_and(|o| &summary_of(&o).id == id))
	}

	/// Marks a mail read or unread on the server and in the list.
	fn set_unread(self: &Rc<Self>, summary: &MailSummary, unread: bool) {
		let Some(backend) = self.ctx.backend() else { return };
		if let Some(pos) = self.position_of(&summary.id) {
			// A new object, so the list view binds the row again; the selection stays put.
			let mut m = summary_of(&self.store.item(pos).unwrap());
			m.unread = unread;
			let selected = self.selection.selected();
			self.hold.set(true);
			self.store.splice(pos, 1, &[glib::BoxedAnyObject::new(m)]);
			self.selection.set_selected(selected);
			self.hold.set(false);
		}
		self.change_unread(if unread { 1 } else { -1 });
		let id = summary.id.clone();
		let ctx = self.ctx.clone();
		runtime::spawn_local(async move {
			if let Err(e) = runtime::run(async move { backend.set_unread(&id, unread).await }).await {
				ctx.error(&e);
			}
		});
	}

	fn mark_unread(self: &Rc<Self>) {
		let Some(summary) = self.reader.current.borrow().as_ref().map(|o| o.mail.summary.clone()) else { return };
		let listed = self.position_of(&summary.id).and_then(|p| self.store.item(p)).map(|o| summary_of(&o));
		if listed.is_some_and(|m| !m.unread) {
			self.set_unread(&summary, true);
		}
	}

	fn trash_selected(self: &Rc<Self>) {
		let Some(backend) = self.ctx.backend() else { return };
		let Some(summary) = self.reader.current.borrow().as_ref().map(|o| o.mail.summary.clone()) else { return };
		let pos = self.position_of(&summary.id);
		if let Some(pos) = pos {
			let unread = summary_of(&self.store.item(pos).unwrap()).unread;
			self.store.remove(pos);
			if unread {
				self.change_unread(-1);
			}
		}
		self.reader.clear();
		self.selection.set_selected(gtk::INVALID_LIST_POSITION);
		self.update_actions();
		if self.inner.is_collapsed() {
			self.inner.set_show_content(false);
		}
		if self.store.n_items() == 0 {
			self.list_stack.set_visible_child_name("empty");
		}
		let this = self.clone();
		runtime::spawn_local(async move {
			let id = summary.id.clone();
			match runtime::run(async move { backend.trash(&id).await }).await {
				Ok(()) => {
					this.ctx.toast("Moved to Trash");
					this.load_folders(false);
				}
				Err(e) => {
					this.ctx.error(&e);
					this.refresh();
				}
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

	/// For the screenshot scenes: selects the first mail whose subject contains `text`.
	pub fn select_by_subject(&self, text: &str) -> bool {
		let found = (0..self.store.n_items()).find(|&i| self.store.item(i).is_some_and(|o| summary_of(&o).subject.contains(text)));
		if let Some(i) = found {
			self.selection.set_selected(i);
		}
		found.is_some()
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

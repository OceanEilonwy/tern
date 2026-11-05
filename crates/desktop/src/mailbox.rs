//! The signed-in view: folders, the conversation list and the reading pane.
//!
//! Everything shown comes from the cache, so switching folders is instant; the folder on show is
//! brought up to date in the background while a small spinner in its header turns.

use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::rc::Rc;

use adw::prelude::*;
use gtk::{gdk, gio, glib};
use tern_app::view::{Action, AnswerKind, FolderIcon, FolderView, ListState, ListView, RowView, SearchView};
use tern_mail::FolderKind;

use crate::layout::Layout;
use crate::reader::Reader;
use crate::{grip, search, window::Ctx};

/// The folder sidebar's width bounds, in pixels, and its width as a rail of icons. Dragged narrower
/// than `RAIL_SNAP` it becomes the rail.
const FOLDERS: (u32, u32) = (180, 400);
pub const RAIL: u32 = 52;
const RAIL_SNAP: i32 = 140;
/// The conversation list's width bounds.
const LIST: (u32, u32) = (280, 720);

type MailAction = fn(&Rc<Mailbox>);
type RowAction = fn(&Rc<Mailbox>, &RowView);

pub struct Mailbox {
	ctx: Ctx,
	pub root: gtk::Widget,
	outer: adw::OverlaySplitView,
	inner: adw::NavigationSplitView,
	account: adw::WindowTitle,
	folder_header: adw::HeaderBar,
	/// The app's icon, centered on the folders' header in place of the window's own when they are a
	/// rail: the window's sits at the header's start, off the rail's middle.
	rail_icon: gtk::Image,
	/// The main menu: on the folders' header, or on the list's while the folders are a rail.
	menus: (gtk::MenuButton, gtk::MenuButton),
	folder_list: gtk::ListBox,
	/// The offer to clean up, at the foot of the folders: shown once Tern takes more room than set.
	broom: Broom,
	folders: RefCell<Vec<FolderRow>>,
	list_title: adw::WindowTitle,
	list_stack: gtk::Stack,
	empty: adw::StatusPage,
	pub store: gio::ListStore,
	/// The row widget showing each conversation on screen, so a changed one is redrawn in place.
	bound: Rc<RefCell<HashMap<glib::Object, gtk::Box>>>,
	selection: gtk::SingleSelection,
	list_view: gtk::ListView,
	/// What the core last said about the folder on show, and about a search.
	list: RefCell<Option<ListView>>,
	found: RefCell<Option<SearchView>>,
	/// Changing what is selected to match the core, which is not the person choosing anything.
	hold: Cell<bool>,
	/// The conversation just chosen, until the core says it is open.
	asked: RefCell<Option<String>>,
	/// The last selection came from the keyboard: on a narrow window it does not leave the list.
	by_keyboard: Cell<bool>,
	search_bar: gtk::SearchBar,
	search_entry: gtk::SearchEntry,
	/// The core has a search on show instead of the folder.
	searching: Cell<bool>,
	/// A search was asked for, which the core may not have heard yet.
	search_open: Cell<bool>,
	/// The last search the core put in the bar itself, as for a rule's line.
	search_revision: Cell<u64>,
	/// Putting the core's text in the bar, which is not the person typing.
	writing_search: Cell<bool>,
	/// The conversation whose menu was opened last, which the menu's actions act on.
	menu_row: RefCell<Option<RowView>>,
	pub reader: Rc<Reader>,
}

/// The sidebar's offer to clean up: a broom with a dot, "Clean Up" and how much Tern takes; on the
/// rail, the broom alone.
struct Broom {
	button: gtk::Button,
	text: gtk::Box,
	size: gtk::Label,
}

impl Broom {
	fn new() -> Broom {
		let icon = gtk::Image::builder().icon_name("tern-broom-symbolic").build();
		let dot = gtk::Box::builder().css_classes(["cleanup-dot"]).halign(gtk::Align::End).valign(gtk::Align::Start).build();
		let mark = gtk::Overlay::builder().child(&icon).valign(gtk::Align::Center).build();
		mark.add_overlay(&dot);
		let size = gtk::Label::builder().xalign(0.0).css_classes(["caption", "dim-label", "numeric"]).build();
		let text = gtk::Box::builder().orientation(gtk::Orientation::Vertical).build();
		text.append(&gtk::Label::builder().label("Clean Up").xalign(0.0).build());
		text.append(&size);
		let content = gtk::Box::builder().spacing(12).build();
		content.append(&mark);
		content.append(&text);
		let button = gtk::Button::builder().child(&content).action_name("win.clean-up").css_classes(["flat", "cleanup"]).visible(false).build();
		button.update_property(&[gtk::accessible::Property::Label("Clean Up")]);
		Broom { button, text, size }
	}

	fn offer(&self, size: Option<&str>) {
		self.button.set_visible(size.is_some());
		if let Some(size) = size {
			let size = format!("{size} on this computer");
			self.size.set_label(&size);
			self.button.set_tooltip_text(Some(&format!("Clean Up — {size}")));
		}
	}

	fn set_rail(&self, rail: bool) {
		self.text.set_visible(!rail);
		self.button.set_halign(if rail { gtk::Align::Center } else { gtk::Align::Fill });
	}
}

/// A folder in the sidebar, with its unread count beside its name, or on its icon on the rail.
struct FolderRow {
	folder: FolderView,
	/// Its Ctrl+number shortcut, if it has one.
	shortcut: Option<usize>,
	row: gtk::Box,
	icon: gtk::Image,
	name: gtk::Label,
	count: gtk::Label,
	badge: gtk::Label,
}

impl FolderRow {
	fn new(folder: FolderView, shortcut: Option<usize>, rail: bool) -> FolderRow {
		let icon = gtk::Image::from_icon_name(folder_icon(folder.icon));
		let badge = gtk::Label::builder().css_classes(["numeric", "rail-badge"]).halign(gtk::Align::End).valign(gtk::Align::Start).build();
		let marked = gtk::Overlay::builder().child(&icon).build();
		marked.add_overlay(&badge);
		let name = gtk::Label::builder().label(&folder.name).xalign(0.0).hexpand(true).ellipsize(gtk::pango::EllipsizeMode::End).build();
		let count = gtk::Label::builder().css_classes(["dim-label", "numeric", "caption-heading"]).build();
		let row = gtk::Box::builder().spacing(12).build();
		row.append(&marked);
		row.append(&name);
		row.append(&count);
		let this = FolderRow { folder, shortcut, row, icon, name, count, badge };
		this.set_rail(rail);
		this
	}

	/// As a line (icon, name, count) or, on the rail, the icon alone with the count on its corner.
	fn set_rail(&self, rail: bool) {
		self.name.set_visible(!rail);
		self.row.set_halign(if rail { gtk::Align::Center } else { gtk::Align::Fill });
		self.row.set_margin_start(if rail { 0 } else { 6 });
		self.row.set_margin_end(if rail { 0 } else { 6 });
		// On the rail, room around the icon for the count to sit on its corner.
		let (v, h) = if rail { (7, 6) } else { (3, 0) };
		self.icon.set_margin_top(v);
		self.icon.set_margin_bottom(v);
		self.icon.set_margin_start(h);
		self.icon.set_margin_end(h);
		self.set_unread(self.folder.unread);
	}

	fn set_unread(&self, unread: u32) {
		let rail = !self.name.is_visible();
		self.count.set_label(&unread.to_string());
		self.count.set_visible(unread > 0 && !rail);
		self.badge.set_label(&if unread > 99 { "99+".to_string() } else { unread.to_string() });
		self.badge.set_visible(unread > 0 && rail);
		let mut tip = self.folder.name.clone();
		if rail && unread > 0 {
			tip = format!("{tip}, {}", unread_subtitle(unread));
		}
		if let Some(n) = self.shortcut {
			tip = format!("{tip} (Ctrl+{n})");
		}
		self.row.set_tooltip_text(Some(&tip));
	}
}

fn folder_icon(icon: FolderIcon) -> &'static str {
	match icon {
		FolderIcon::Inbox => "tern-inbox-symbolic",
		FolderIcon::Drafts => "document-edit-symbolic",
		FolderIcon::Sent => "tern-sent-symbolic",
		FolderIcon::Archive => "tern-archive-symbolic",
		FolderIcon::Spam => "mail-mark-junk-symbolic",
		FolderIcon::Trash => "user-trash-symbolic",
		FolderIcon::Folder => "folder-symbolic",
	}
}

fn row_of(obj: &glib::Object) -> RowView {
	obj.downcast_ref::<glib::BoxedAnyObject>().unwrap().borrow::<RowView>().clone()
}

/// One line of the conversation list.
fn row_widget() -> gtk::Box {
	let dot = gtk::Image::builder().icon_name("media-record-symbolic").pixel_size(8).css_classes(["accent"]).valign(gtk::Align::Center).build();
	dot.set_tooltip_text(Some("Unread"));
	let sender = gtk::Label::builder().xalign(0.0).hexpand(true).ellipsize(gtk::pango::EllipsizeMode::End).build();
	let pin = gtk::Image::builder().icon_name("tern-pin-symbolic").css_classes(["dim-label"]).build();
	pin.set_tooltip_text(Some("Pinned"));
	let count = gtk::Label::builder().css_classes(["caption", "numeric", "thread-count"]).valign(gtk::Align::Center).build();
	let clip = gtk::Image::builder().icon_name("mail-attachment-symbolic").css_classes(["dim-label"]).build();
	let date = gtk::Label::builder().css_classes(["caption", "dim-label", "numeric"]).build();
	let top = gtk::Box::new(gtk::Orientation::Horizontal, 6);
	top.append(&dot);
	top.append(&sender);
	top.append(&pin);
	top.append(&count);
	top.append(&clip);
	top.append(&date);
	let subject = gtk::Label::builder().xalign(0.0).ellipsize(gtk::pango::EllipsizeMode::End).build();
	let row = gtk::Box::builder().orientation(gtk::Orientation::Vertical).spacing(3).margin_top(9).margin_bottom(9).margin_start(6).margin_end(6).build();
	row.append(&top);
	row.append(&subject);
	row
}

fn bind_row(row: &gtk::Box, t: &RowView) {
	let top = row.first_child().and_downcast::<gtk::Box>().unwrap();
	let subject = top.next_sibling().and_downcast::<gtk::Label>().unwrap();
	let dot = top.first_child().unwrap();
	let sender = dot.next_sibling().and_downcast::<gtk::Label>().unwrap();
	let pin = sender.next_sibling().unwrap();
	let count = pin.next_sibling().and_downcast::<gtk::Label>().unwrap();
	let clip = count.next_sibling().unwrap();
	let date = clip.next_sibling().and_downcast::<gtk::Label>().unwrap();
	dot.set_visible(t.unread);
	pin.set_visible(t.pinned);
	if t.draft {
		sender.set_markup(&format!("<span foreground=\"#e01b24\">Draft</span>  {}", glib::markup_escape_text(&t.who)));
	} else {
		sender.set_label(&t.who);
	}
	if t.unread {
		sender.add_css_class("heading");
	} else {
		sender.remove_css_class("heading");
	}
	count.set_label(&t.count.to_string());
	count.set_visible(t.count > 1);
	count.set_tooltip_text(Some(&format!("{} messages", t.count)));
	clip.set_visible(t.has_attachments);
	date.set_label(&t.date);
	subject.set_label(&t.subject);
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
		let menu_button = || gtk::MenuButton::builder().icon_name("open-menu-symbolic").menu_model(&menu).tooltip_text("Main Menu").build();
		let menus = (menu_button(), menu_button());
		let folder_header = adw::HeaderBar::builder().title_widget(&account).build();
		folder_header.pack_end(&menus.0);
		let folder_list = gtk::ListBox::builder().css_classes(["navigation-sidebar"]).build();
		folder_list.update_property(&[gtk::accessible::Property::Label("Folders")]);
		let folder_scroll = gtk::ScrolledWindow::builder().hscrollbar_policy(gtk::PolicyType::Never).child(&folder_list).vexpand(true).build();
		let folder_view = adw::ToolbarView::new();
		folder_view.add_top_bar(&folder_header);
		folder_view.set_content(Some(&folder_scroll));
		let broom = Broom::new();
		folder_view.add_bottom_bar(&broom.button);

		// Conversation list.
		let store = gio::ListStore::new::<glib::BoxedAnyObject>();
		let selection = gtk::SingleSelection::builder().model(&store).autoselect(false).can_unselect(true).build();
		let factory = gtk::SignalListItemFactory::new();
		factory.connect_setup(|_, item| {
			let item = item.downcast_ref::<gtk::ListItem>().unwrap();
			item.set_child(Some(&row_widget()));
		});
		let bound: Rc<RefCell<HashMap<glib::Object, gtk::Box>>> = Rc::default();
		let b = bound.clone();
		factory.connect_bind(move |_, item| {
			let item = item.downcast_ref::<gtk::ListItem>().unwrap();
			let (Some(row), Some(obj)) = (item.child().and_downcast::<gtk::Box>(), item.item()) else { return };
			bind_row(&row, &row_of(&obj));
			b.borrow_mut().insert(obj, row);
		});
		let b = bound.clone();
		factory.connect_unbind(move |_, item| {
			if let Some(obj) = item.downcast_ref::<gtk::ListItem>().unwrap().item() {
				b.borrow_mut().remove(&obj);
			}
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
		let activity = ctx.activity.indicator();
		let refresh = gtk::Button::builder().icon_name("view-refresh-symbolic").action_name("win.refresh").tooltip_text("Check for New Mail (F5)").build();
		list_header.pack_start(&sidebar_toggle);
		list_header.pack_start(&compose);
		list_header.pack_end(&menus.1);
		list_header.pack_end(&refresh);
		list_header.pack_end(&activity);
		let list_view_tb = adw::ToolbarView::new();
		list_view_tb.add_top_bar(&list_header);
		let (search_button, search_bar, search_entry) = search::widgets();
		list_header.pack_end(&search_button);
		list_view_tb.add_top_bar(&search_bar);
		list_view_tb.set_content(Some(&list_stack));
		let (list_pane, list_grip) = grip::wrap(&list_view_tb);
		let list_page = adw::NavigationPage::builder().title("Inbox").tag("list").child(&list_pane).build();

		let reader = Reader::new(ctx.clone());
		let inner = adw::NavigationSplitView::builder()
			.sidebar(&list_page)
			.content(&reader.page)
			.min_sidebar_width(300.0)
			.max_sidebar_width(420.0)
			.sidebar_width_fraction(0.38)
			.build();
		let inner_page = adw::NavigationPage::builder().title("Mail").child(&inner).build();
		let (folder_pane, folder_grip) = grip::wrap(&folder_view);
		let folder_page = adw::NavigationPage::builder().title("Folders").child(&folder_pane).build();
		let outer = adw::OverlaySplitView::builder()
			.sidebar(&folder_page)
			.content(&inner_page)
			.min_sidebar_width(200.0)
			.max_sidebar_width(260.0)
			.sidebar_width_fraction(0.2)
			.build();
		outer.bind_property("collapsed", &sidebar_toggle, "visible").sync_create().build();
		outer.bind_property("show-sidebar", &sidebar_toggle, "active").sync_create().bidirectional().build();
		outer.bind_property("collapsed", &folder_grip, "visible").invert_boolean().sync_create().build();
		inner.bind_property("collapsed", &list_grip, "visible").invert_boolean().sync_create().build();

		let this = Rc::new(Mailbox {
			ctx,
			root: outer.clone().upcast(),
			outer,
			inner,
			account,
			folder_header,
			rail_icon: gtk::Image::builder().icon_name(crate::APP_ID).pixel_size(16).build(),
			menus,
			folder_list,
			broom,
			folders: RefCell::default(),
			list_title,
			list_stack,
			empty,
			store,
			bound,
			selection,
			list_view,
			list: RefCell::default(),
			found: RefCell::default(),
			hold: Cell::new(false),
			asked: RefCell::default(),
			by_keyboard: Cell::new(false),
			search_bar,
			search_entry,
			searching: Cell::new(false),
			search_open: Cell::new(false),
			search_revision: Cell::new(0),
			writing_search: Cell::new(false),
			menu_row: RefCell::default(),
			reader,
		});
		this.connect_signals(&list_scroll);
		this.connect_grips((&folder_grip, &folder_pane), (&list_grip, &list_pane));
		this.connect_search();
		this.apply_panes();
		let weak = Rc::downgrade(&this);
		this.reader.connect_current(move || {
			if let Some(this) = weak.upgrade() {
				this.update_actions();
			}
		});
		let weak = Rc::downgrade(&this);
		this.ctx.connect_offer_cleanup(move |bytes| {
			if let Some(this) = weak.upgrade() {
				this.broom.offer(bytes);
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

	/// Dragging a pane's edge resizes it; the folders become a rail of icons when dragged narrow,
	/// or on a double click. The widths are kept for next time.
	fn connect_grips(self: &Rc<Self>, folders: (&gtk::Box, &gtk::Overlay), list: (&gtk::Box, &gtk::Overlay)) {
		let save = {
			let ctx = self.ctx.clone();
			move || ctx.set_layout(ctx.layout(), true)
		};
		let weak = Rc::downgrade(self);
		let resize = move |w| {
			if let Some(this) = weak.upgrade() {
				this.drag_folders(w);
			}
		};
		let weak = Rc::downgrade(self);
		let double = move || {
			if let Some(this) = weak.upgrade() {
				this.toggle_rail();
			}
		};
		grip::connect(folders.0, folders.1, resize, save.clone(), double);
		let weak = Rc::downgrade(self);
		let resize = move |w| {
			if let Some(this) = weak.upgrade() {
				this.drag_list(w);
			}
		};
		let weak = Rc::downgrade(self);
		let double = move || {
			if let Some(this) = weak.upgrade() {
				this.ctx.set_layout(Layout { list_width: 0, ..this.ctx.layout() }, true);
				this.apply_panes();
			}
		};
		grip::connect(list.0, list.1, resize, save, double);
		let weak = Rc::downgrade(self);
		self.outer.connect_collapsed_notify(move |_| {
			if let Some(this) = weak.upgrade() {
				this.apply_panes();
			}
		});
		if let Some(settings) = gtk::Settings::default() {
			let weak = Rc::downgrade(self);
			settings.connect_gtk_decoration_layout_notify(move |_| {
				if let Some(this) = weak.upgrade() {
					this.apply_panes();
				}
			});
		}
	}

	/// The folder sidebar's edge is dragged to `w` pixels.
	pub fn drag_folders(&self, w: i32) {
		let (folders_width, folders_rail) = folders_drag(w, self.ctx.layout().folders_width);
		self.ctx.set_layout(Layout { folders_width, folders_rail, ..self.ctx.layout() }, false);
		self.apply_panes();
	}

	/// The conversation list's edge is dragged to `w` pixels.
	pub fn drag_list(&self, w: i32) {
		let list_width = (w.max(0) as u32).clamp(LIST.0, LIST.1);
		self.ctx.set_layout(Layout { list_width, ..self.ctx.layout() }, false);
		self.apply_panes();
	}

	/// For the screenshot scenes: the widths of the folder sidebar and the conversation list.
	pub fn pane_widths(&self) -> (i32, i32) {
		(self.outer.sidebar().map_or(0, |w| w.width()), self.inner.sidebar().map_or(0, |w| w.width()))
	}

	/// F9: folders to a rail of icons and back; on a narrow window, shows or hides them.
	fn toggle_rail(&self) {
		if self.outer.is_collapsed() {
			self.outer.set_show_sidebar(!self.outer.shows_sidebar());
			return;
		}
		let s = self.ctx.layout();
		self.ctx.set_layout(Layout { folders_rail: !s.folders_rail, ..s }, true);
		self.apply_panes();
	}

	/// Sizes the panes as last left. Up to a share of the window: a narrower window narrows them.
	fn apply_panes(&self) {
		let s = self.ctx.layout();
		let rail = self.is_rail();
		self.outer.set_sidebar_width_unit(adw::LengthUnit::Px);
		let (min, max, fraction) = match (rail, s.folders_width) {
			(true, _) => (RAIL, RAIL, 0.0),
			(false, 0) => (200, 260, 0.2),
			(false, w) => (FOLDERS.0.min(w), w, 0.3),
		};
		self.outer.set_min_sidebar_width(min as f64);
		self.outer.set_max_sidebar_width(max as f64);
		self.outer.set_sidebar_width_fraction(fraction);
		let layout = self.folder_header.decoration_layout().map(|l| l.to_string()).or_else(|| gtk::Settings::default().and_then(|s| s.gtk_decoration_layout()).map(|l| l.to_string()));
		let centered = rail && layout.as_deref().is_some_and(starts_with_icon_only);
		self.folder_header.set_show_start_title_buttons(!centered);
		if centered {
			self.folder_header.set_title_widget(Some(&self.rail_icon));
		} else {
			self.folder_header.set_title_widget(Some(&self.account));
		}
		self.folder_header.set_show_title(!rail || centered);
		self.menus.0.set_visible(!rail);
		self.menus.0.set_primary(!rail);
		self.menus.1.set_visible(rail);
		self.menus.1.set_primary(rail);
		if rail {
			self.folder_list.add_css_class("rail");
		} else {
			self.folder_list.remove_css_class("rail");
		}
		self.broom.set_rail(rail);
		for row in self.folders.borrow().iter() {
			row.set_rail(rail);
		}
		self.inner.set_sidebar_width_unit(adw::LengthUnit::Px);
		let (min, max, fraction) = match s.list_width {
			0 => (300, 420, 0.38),
			w => (LIST.0.min(w), w, 0.6),
		};
		self.inner.set_min_sidebar_width(min as f64);
		self.inner.set_max_sidebar_width(max as f64);
		self.inner.set_sidebar_width_fraction(fraction);
	}

	/// For the screenshot scenes: how far the middle of the icon on the folders' header is from
	/// the folders' left edge.
	pub fn header_icon_middle(&self) -> Option<f32> {
		let sidebar = self.outer.sidebar()?;
		let icon: gtk::Widget = if self.rail_icon.is_mapped() {
			self.rail_icon.clone().upcast()
		} else {
			// The window's own icon, which GTK draws as an image in the window controls.
			let mut found = None;
			let mut stack = vec![self.folder_header.clone().upcast::<gtk::Widget>()];
			while let Some(w) = stack.pop() {
				if w.is::<gtk::Image>() && w.is_mapped() && w.ancestor(gtk::WindowControls::static_type()).is_some() {
					found = Some(w.clone());
				}
				let mut child = w.first_child();
				while let Some(c) = child {
					child = c.next_sibling();
					stack.push(c);
				}
			}
			found?
		};
		let bounds = icon.compute_bounds(&sidebar)?;
		Some(bounds.x() + bounds.width() / 2.0)
	}

	/// The folders show as a rail of icons: only beside the list, not over it on a narrow window.
	pub fn is_rail(&self) -> bool {
		self.ctx.layout().folders_rail && !self.outer.is_collapsed()
	}

	fn connect_signals(self: &Rc<Self>, list_scroll: &gtk::ScrolledWindow) {
		let weak = Rc::downgrade(self);
		self.folder_list.connect_row_selected(move |_, row| {
			let (Some(this), Some(row)) = (weak.upgrade(), row) else { return };
			let folder = this.folders.borrow().get(row.index() as usize).map(|r| r.folder.clone());
			if let Some(folder) = folder {
				if !this.hold.get() && !folder.selected {
					this.leave_search();
					this.hold.set(true);
					this.selection.set_selected(gtk::INVALID_LIST_POSITION);
					this.store.remove_all();
					this.hold.set(false);
					this.inner.set_show_content(false);
					this.ctx.engine.send(Action::OpenFolder { id: folder.id });
				}
				if !this.hold.get() && this.outer.is_collapsed() {
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
					let row = row_of(&obj);
					if !row.open {
						*this.asked.borrow_mut() = Some(row.key.clone());
						this.ctx.engine.send(Action::OpenThread { key: row.key, folder_id: row.folder_id });
					}
					if !this.by_keyboard.get() {
						this.inner.set_show_content(true);
					}
				},
				None => this.close_conversation(),
			}
			this.by_keyboard.set(false);
		});
		// Arrow keys move through the list without leaving it, even on a narrow window.
		let keys = gtk::EventControllerKey::new();
		keys.set_propagation_phase(gtk::PropagationPhase::Capture);
		let weak = Rc::downgrade(self);
		keys.connect_key_pressed(move |_, key, _, state| {
			if let Some(this) = weak.upgrade() {
				if matches!(key, gdk::Key::Up | gdk::Key::Down | gdk::Key::Page_Up | gdk::Key::Page_Down | gdk::Key::Home | gdk::Key::End) {
					this.by_keyboard.set(true);
				}
				// The menu key, or Shift+F10, opens the focused conversation's menu.
				let menu_key = key == gdk::Key::Menu || (key == gdk::Key::F10 && state.contains(gdk::ModifierType::SHIFT_MASK));
				if menu_key && this.focused_row().is_some_and(|(i, _)| this.open_row_menu(i, None).is_some()) {
					return glib::Propagation::Stop;
				}
			}
			glib::Propagation::Proceed
		});
		self.list_view.add_controller(keys);
		// A right click, or a long press, opens a conversation's menu where it was pressed.
		let click = gtk::GestureClick::builder().button(gdk::BUTTON_SECONDARY).build();
		let weak = Rc::downgrade(self);
		click.connect_pressed(move |gesture, _, x, y| {
			if let Some(this) = weak.upgrade() {
				if this.row_menu_at(x, y) {
					gesture.set_state(gtk::EventSequenceState::Claimed);
				}
			}
		});
		self.list_view.add_controller(click);
		let press = gtk::GestureLongPress::builder().touch_only(true).build();
		let weak = Rc::downgrade(self);
		press.connect_pressed(move |gesture, x, y| {
			if let Some(this) = weak.upgrade() {
				if this.row_menu_at(x, y) {
					gesture.set_state(gtk::EventSequenceState::Claimed);
				}
			}
		});
		self.list_view.add_controller(press);
		// Enter (or a click on the selected row) opens the conversation and moves focus into it.
		let weak = Rc::downgrade(self);
		self.list_view.connect_activate(move |_, _| {
			let Some(this) = weak.upgrade() else { return };
			if this.reader.edit_current_draft() {
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
				this.close_conversation();
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
					this.ctx.engine.send(Action::LoadMore);
				}
			}
		});
	}

	fn add_actions(self: &Rc<Self>) {
		let entries: [(&str, MailAction); 15] = [
			("compose", |m| m.ctx.engine.send(Action::ComposeNew)),
			("reply", |m| m.answer(AnswerKind::Reply)),
			("reply-all", |m| m.answer(AnswerKind::ReplyAll)),
			("forward", |m| m.answer(AnswerKind::Forward)),
			("trash", |m| {
				m.ctx.engine.send(Action::ConversationTrash);
				m.focus_list();
			}),
			("spam", |m| m.ctx.engine.send(Action::ConversationSpam)),
			("refresh", |m| m.ctx.engine.send(Action::Refresh)),
			("mark-unread", |m| {
				m.ctx.engine.send(Action::ConversationUnread);
				m.focus_list();
			}),
			("open-in-tuta", |m| {
				if let Some(url) = m.reader.web_url() {
					crate::render::open_uri(&m.ctx.window, &url);
				}
			}),
			("next", |m| m.step(1)),
			("previous", |m| m.step(-1)),
			("focus-list", |m| m.focus_list()),
			("toggle-folders", |m| m.toggle_rail()),
			("search", |m| m.start_search()),
			("clean-up", |m| m.ctx.engine.send(Action::OpenCleanup)),
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
		// What a conversation's menu does, to the conversation it was opened on.
		let row_actions: [(&str, RowAction); 5] = [
			("pin", |m, row| m.ctx.engine.send(Action::RowPin { key: row.key.clone() })),
			("read", |m, row| m.ctx.engine.send(Action::RowRead { key: row.key.clone(), folder_id: row.folder_id.clone(), read: true })),
			("unread", |m, row| m.ctx.engine.send(Action::RowRead { key: row.key.clone(), folder_id: row.folder_id.clone(), read: false })),
			("spam", |m, row| m.ctx.engine.send(Action::RowSpam { key: row.key.clone(), folder_id: row.folder_id.clone() })),
			("trash", |m, row| m.ctx.engine.send(Action::RowTrash { key: row.key.clone(), folder_id: row.folder_id.clone() })),
		];
		let group = gio::SimpleActionGroup::new();
		for (name, f) in row_actions {
			let action = gio::SimpleAction::new(name, None);
			let weak = Rc::downgrade(self);
			action.connect_activate(move |_, _| {
				let Some(this) = weak.upgrade() else { return };
				let row = this.menu_row.borrow().clone();
				if let Some(row) = row {
					f(&this, &row);
				}
			});
			group.add_action(&action);
		}
		self.list_view.insert_action_group("row", Some(&group));
		self.set_enabled(false);
	}

	/// Opens the menu of the conversation under `(x, y)` in the list, if there is one there.
	fn row_menu_at(self: &Rc<Self>, x: f64, y: f64) -> bool {
		let Some(picked) = self.list_view.pick(x, y, gtk::PickFlags::DEFAULT) else { return false };
		let found = self.bound.borrow().iter().find(|(_, w)| picked == *w.upcast_ref::<gtk::Widget>() || picked.is_ancestor(*w)).map(|(o, w)| (o.clone(), w.clone()));
		let Some((obj, widget)) = found else { return false };
		let at = self.list_view.compute_point(&widget, &gtk::graphene::Point::new(x as f32, y as f32)).map(|p| (p.x() as f64, p.y() as f64));
		self.show_row_menu(&widget, row_of(&obj), at);
		true
	}

	/// Opens the menu of the conversation at `i` in the list, at `at` in its row or under it. For
	/// the screenshot scenes too, which get the menu back.
	pub fn open_row_menu(self: &Rc<Self>, i: u32, at: Option<(f64, f64)>) -> Option<gtk::PopoverMenu> {
		let obj = self.store.item(i)?;
		let widget = self.bound.borrow().get(&obj).cloned()?;
		Some(self.show_row_menu(&widget, row_of(&obj), at))
	}

	/// For the screenshot scenes: right-clicks the middle of row `i`'s sender.
	pub fn right_click_row(self: &Rc<Self>, i: u32) -> bool {
		let Some(widget) = self.store.item(i).and_then(|obj| self.bound.borrow().get(&obj).cloned()) else { return false };
		let Some(sender) = widget.first_child().and_then(|top| top.first_child()).and_then(|dot| dot.next_sibling()) else { return false };
		let middle = gtk::graphene::Point::new(sender.width() as f32 / 2.0, sender.height() as f32 / 2.0);
		let Some(p) = sender.compute_point(&self.list_view, &middle) else { return false };
		self.row_menu_at(p.x() as f64, p.y() as f64)
	}

	/// For the screenshot scenes: does what the conversation menu's item `name` does.
	pub fn row_action(&self, name: &str) -> Result<(), String> {
		self.list_view.activate_action(&format!("row.{name}"), None).map_err(|e| e.to_string())
	}

	fn show_row_menu(self: &Rc<Self>, widget: &gtk::Box, row: RowView, at: Option<(f64, f64)>) -> gtk::PopoverMenu {
		let kind = self.folders.borrow().iter().find(|r| r.folder.id == row.folder_id).map(|r| r.folder.icon);
		let menu = gio::Menu::new();
		let section = gio::Menu::new();
		section.append(Some(if row.pinned { "Un_pin" } else { "_Pin" }), Some("row.pin"));
		if row.unread {
			section.append(Some("Mark as _Read"), Some("row.read"));
		} else {
			section.append(Some("Mark as _Unread"), Some("row.unread"));
		}
		menu.append_section(None, &section);
		let section = gio::Menu::new();
		if kind != Some(FolderIcon::Spam) {
			section.append(Some("Move to _Spam…"), Some("row.spam"));
		}
		if kind != Some(FolderIcon::Trash) {
			section.append(Some("Move to _Trash"), Some("row.trash"));
		}
		menu.append_section(None, &section);
		*self.menu_row.borrow_mut() = Some(row);
		let popover = gtk::PopoverMenu::builder().menu_model(&menu).has_arrow(false).halign(gtk::Align::Start).build();
		popover.set_parent(widget);
		if let Some((x, y)) = at {
			popover.set_pointing_to(Some(&gdk::Rectangle::new(x as i32, y as i32, 1, 1)));
		}
		popover.connect_closed(|popover| {
			// After the chosen action has run.
			let popover = popover.clone();
			glib::idle_add_local_once(move || popover.unparent());
		});
		popover.popup();
		popover
	}

	fn action(&self, name: &str) -> gio::SimpleAction {
		self.ctx.window.lookup_action(name).and_downcast::<gio::SimpleAction>().unwrap()
	}

	fn set_enabled(&self, signed_in: bool) {
		for name in ["compose", "refresh", "next", "previous", "focus-list", "folder", "search", "clean-up"] {
			self.action(name).set_enabled(signed_in);
		}
		self.update_actions();
	}

	/// Mail actions work only while a conversation is open.
	fn update_actions(&self) {
		let open = self.reader.current().is_some();
		let answerable = self.reader.opened().is_some_and(|c| !c.draft);
		for name in ["reply", "reply-all", "forward"] {
			self.action(name).set_enabled(answerable);
		}
		for name in ["trash", "spam", "mark-unread", "open-in-tuta"] {
			self.action(name).set_enabled(open || self.reader.thread().is_some());
		}
	}

	/// An account has opened.
	pub fn start(self: &Rc<Self>) {
		let Some(backend) = self.ctx.backend() else { return };
		let account = backend.account();
		self.account.set_subtitle(&account.address);
		self.ctx.window.set_title(Some(&format!("Tern — {}", account.address)));
		self.set_enabled(true);
	}

	pub fn clear(&self) {
		self.leave_search();
		self.folder_list.remove_all();
		self.folders.borrow_mut().clear();
		*self.list.borrow_mut() = None;
		*self.asked.borrow_mut() = None;
		self.hold.set(true);
		self.store.remove_all();
		self.hold.set(false);
		self.reader.show(None);
		self.set_enabled(false);
	}

	/// For the screenshot scenes: whether the sidebar offers to clean up.
	pub fn cleanup_offered(&self) -> bool {
		self.broom.button.is_visible()
	}

	/// The conversation was left: back in a narrow layout, or nothing selected.
	fn close_conversation(&self) {
		*self.asked.borrow_mut() = None;
		if self.reader.thread().is_some() {
			self.ctx.engine.send(Action::Back);
		}
	}

	fn answer(&self, kind: AnswerKind) {
		self.reader.answer(kind);
	}

	/// The core says whether a search is on show instead of the folder.
	pub fn set_searching(self: &Rc<Self>, searching: bool) {
		if self.searching.replace(searching) != searching {
			self.show_current();
		}
	}

	/// Draws what the core says about the folder on show.
	pub fn show_list(self: &Rc<Self>, view: ListView) {
		self.show_folders(&view.folders);
		*self.list.borrow_mut() = Some(view);
		if !self.searching.get() {
			self.show_current();
		}
	}

	/// Draws what the core says about the search.
	pub fn show_search(self: &Rc<Self>, view: SearchView) {
		// The core has put words in the bar itself, as for a rule's line.
		if self.search_revision.replace(view.revision) != view.revision && !view.query.is_empty() && self.search_entry.text() != view.query {
			self.writing_search.set(true);
			self.search_open.set(true);
			self.inner.set_show_content(false);
			self.search_bar.set_search_mode(true);
			self.search_entry.set_text(&view.query);
			self.search_entry.grab_focus();
			self.writing_search.set(false);
		}
		*self.found.borrow_mut() = Some(view);
		if self.searching.get() {
			self.show_current();
		}
	}

	/// Puts what is on show, the folder or the search, in the list.
	fn show_current(self: &Rc<Self>) {
		if self.searching.get() {
			let Some(view) = self.found.borrow().clone() else { return };
			self.list_title.set_title(&view.title);
			self.list_title.set_subtitle("");
			self.show_rows(&view.rows);
			if !view.rows.is_empty() {
				self.list_stack.set_visible_child_name("list");
			} else if view.empty {
				self.show_status("No Matches", Some("No matches in mail on this computer"));
			}
		} else {
			let Some(view) = self.list.borrow().clone() else { return };
			self.list_title.set_title(&view.title);
			let unread = view.folders.iter().find(|f| f.selected).map_or(0, |f| f.unread);
			self.list_title.set_subtitle(&unread_subtitle(unread));
			self.show_rows(&view.rows);
			match view.state {
				ListState::Rows => self.list_stack.set_visible_child_name("list"),
				ListState::Loading => self.list_stack.set_visible_child_name("loading"),
				ListState::Empty | ListState::Failed => self.show_status(&view.empty_title, view.empty_detail.as_deref()),
			}
		}
		self.update_actions();
	}

	fn show_folders(self: &Rc<Self>, folders: &[FolderView]) {
		let unchanged = {
			let old = self.folders.borrow();
			old.len() == folders.len() && old.iter().zip(folders).all(|(a, b)| a.folder.id == b.id && a.folder.name == b.name)
		};
		if unchanged {
			// Only counts changed: update labels in place.
			for (row, new) in self.folders.borrow_mut().iter_mut().zip(folders) {
				row.folder = new.clone();
				row.set_unread(new.unread);
			}
		} else {
			self.hold.set(true);
			self.folder_list.remove_all();
			self.hold.set(false);
			let mut rows = Vec::new();
			for (i, folder) in folders.iter().enumerate() {
				let row = FolderRow::new(folder.clone(), (i < 9).then_some(i + 1), self.is_rail());
				self.folder_list.append(&row.row);
				rows.push(row);
			}
			*self.folders.borrow_mut() = rows;
		}
		let on_show = folders.iter().position(|f| f.selected).and_then(|i| self.folder_list.row_at_index(i as i32));
		if self.folder_list.selected_row() != on_show {
			self.hold.set(true);
			self.folder_list.select_row(on_show.as_ref());
			self.hold.set(false);
		}
	}

	/// Shows a status page in place of the list.
	fn show_status(&self, title: &str, description: Option<&str>) {
		self.empty.set_title(title);
		self.empty.set_description(description);
		self.list_stack.set_visible_child_name("empty");
	}

	/// Puts `rows` in the list, changing only the rows that changed, and selects the one open.
	fn show_rows(self: &Rc<Self>, rows: &[RowView]) {
		let old: Vec<RowView> = (0..self.store.n_items()).filter_map(|i| self.store.item(i)).map(|o| row_of(&o)).collect();
		let (head, tail) = unchanged_ends(&old.iter().map(|r| &r.key).collect::<Vec<_>>(), &rows.iter().map(|r| &r.key).collect::<Vec<_>>());
		// Conversations still in place keep their row object, so the keyboard focus and scroll stay
		// on them; only a changed one is redrawn.
		for (i, row) in rows.iter().enumerate().filter(|&(i, _)| i < head || i >= rows.len() - tail) {
			let at = if i < head { i } else { i + old.len() - rows.len() };
			if old[at] == *row {
				continue;
			}
			let Some(obj) = self.store.item(at as u32) else { continue };
			obj.downcast_ref::<glib::BoxedAnyObject>().unwrap().replace(row.clone());
			if let Some(widget) = self.bound.borrow().get(&obj) {
				bind_row(widget, row);
			}
		}
		self.hold.set(true);
		if head + tail < old.len().max(rows.len()) {
			let items: Vec<glib::BoxedAnyObject> = rows[head..rows.len() - tail].iter().cloned().map(glib::BoxedAnyObject::new).collect();
			self.store.splice(head as u32, (old.len() - head - tail) as u32, &items);
		}
		// What was just chosen stays selected until the core has heard, so nothing flickers.
		let open = rows.iter().position(|r| r.open);
		if open.is_some_and(|i| self.asked.borrow().as_deref() == Some(rows[i].key.as_str())) {
			*self.asked.borrow_mut() = None;
		}
		let chosen = self.asked.borrow().as_ref().and_then(|k| rows.iter().position(|r| &r.key == k));
		let selected = chosen.or(open).map_or(gtk::INVALID_LIST_POSITION, |i| i as u32);
		if self.selection.selected() != selected {
			self.selection.set_selected(selected);
		}
		self.hold.set(false);
	}

	fn connect_search(self: &Rc<Self>) {
		let weak = Rc::downgrade(self);
		self.search_entry.connect_search_changed(move |_| {
			if let Some(this) = weak.upgrade() {
				this.search_changed();
			}
		});
		// Enter opens the best match.
		let weak = Rc::downgrade(self);
		self.search_entry.connect_activate(move |_| {
			if let Some(this) = weak.upgrade() {
				if this.searching.get() && this.store.n_items() > 0 {
					this.selection.set_selected(0);
					this.inner.set_show_content(true);
				}
			}
		});
		// Escape, the button or Ctrl+F again close the bar and go back to the folder.
		let weak = Rc::downgrade(self);
		self.search_bar.connect_search_mode_enabled_notify(move |bar| {
			let Some(this) = weak.upgrade() else { return };
			if !bar.is_search_mode() && this.search_open.get() {
				this.leave_search();
				this.ctx.engine.send(Action::CloseSearch);
				this.focus_list();
			}
		});
	}

	/// Ctrl+F: opens the search bar, or closes it when it has the focus.
	fn start_search(&self) {
		if self.search_bar.is_search_mode() && self.search_focused() {
			self.search_bar.set_search_mode(false);
			return;
		}
		self.inner.set_show_content(false);
		self.search_bar.set_search_mode(true);
		self.search_entry.grab_focus();
	}

	/// The keyboard focus is in the search entry (on the text inside it).
	fn search_focused(&self) -> bool {
		gtk::prelude::GtkWindowExt::focus(&self.ctx.window).is_some_and(|f| f == *self.search_entry.upcast_ref::<gtk::Widget>() || f.is_ancestor(&self.search_entry))
	}

	fn search_changed(self: &Rc<Self>) {
		if self.writing_search.get() {
			return;
		}
		let text = self.search_entry.text().to_string();
		if text.trim().is_empty() {
			// Cleared (also on the way to new text): back to the folder, the bar stays open.
			if self.search_open.replace(false) {
				self.ctx.engine.send(Action::CloseSearch);
			}
			return;
		}
		if !self.search_open.replace(true) {
			self.hold.set(true);
			self.selection.set_selected(gtk::INVALID_LIST_POSITION);
			self.store.remove_all();
			self.hold.set(false);
			self.ctx.engine.send(Action::OpenSearch);
		}
		*self.asked.borrow_mut() = None;
		self.ctx.engine.send(Action::SearchText { text });
	}

	/// Clears the bar, without telling the core; whoever calls this does, one way or another.
	fn leave_search(&self) {
		if self.search_open.replace(false) {
			self.writing_search.set(true);
			self.search_entry.set_text("");
			self.search_bar.set_search_mode(false);
			self.writing_search.set(false);
		}
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

	/// For the screenshot scenes: opens the first folder of a kind.
	pub fn select_folder(&self, kind: FolderKind) {
		let icon = match kind {
			FolderKind::Inbox => FolderIcon::Inbox,
			FolderKind::Drafts => FolderIcon::Drafts,
			FolderKind::Sent => FolderIcon::Sent,
			FolderKind::Archive => FolderIcon::Archive,
			FolderKind::Spam => FolderIcon::Spam,
			FolderKind::Trash => FolderIcon::Trash,
			FolderKind::Custom => FolderIcon::Folder,
		};
		let index = self.folders.borrow().iter().position(|r| r.folder.icon == icon);
		if let Some(row) = index.and_then(|i| self.folder_list.row_at_index(i as i32)) {
			self.folder_list.select_row(Some(&row));
		}
	}

	/// For the screenshot scenes: selects the first conversation whose subject contains `text`.
	pub fn select_by_subject(&self, text: &str) -> bool {
		let found = (0..self.store.n_items()).find(|&i| self.store.item(i).is_some_and(|o| row_of(&o).subject.contains(text)));
		if let Some(i) = found {
			self.selection.set_selected(i);
		}
		found.is_some()
	}

	/// For the screenshot scenes: moves the keyboard focus and selection to row `i`, as an arrow key does.
	pub fn arrow_to(&self, i: u32) {
		self.list_view.grab_focus();
		self.by_keyboard.set(true);
		self.list_view.scroll_to(i, gtk::ListScrollFlags::FOCUS | gtk::ListScrollFlags::SELECT, None);
	}

	/// For the screenshot scenes: the row holding the keyboard focus, and whether it is unread.
	pub fn focused_row(&self) -> Option<(u32, bool)> {
		let focus = gtk::prelude::GtkWindowExt::focus(&self.ctx.window)?;
		let bound = self.bound.borrow();
		let (obj, _) = bound.iter().find(|(_, w)| w.is_ancestor(&focus) || **w == focus)?;
		let at = (0..self.store.n_items()).find(|&i| self.store.item(i).as_ref() == Some(obj))?;
		Some((at, row_of(obj).unread))
	}

	/// For the screenshot scenes: the title of the status page shown instead of the list, if one is.
	pub fn status(&self) -> Option<String> {
		(self.list_stack.visible_child_name().as_deref() == Some("empty")).then(|| self.empty.title().to_string())
	}

	/// For the screenshot scenes: types `text` into the search bar, which is open and has the focus.
	pub fn type_search(&self, text: &str) -> Result<(), String> {
		if !self.search_bar.is_search_mode() || !self.search_focused() {
			return Err("the search bar is not open with the focus in it".into());
		}
		self.search_entry.set_text(text);
		Ok(())
	}

	/// For the screenshot scenes: types `more` after the text in the search bar, as typing does.
	pub fn append_search(&self, more: &str) {
		let mut end = self.search_entry.text().chars().count() as i32;
		self.search_entry.insert_text(more, &mut end);
	}

	/// For the screenshot scenes: the list shows search results, and the bar is open.
	pub fn is_searching(&self) -> bool {
		self.searching.get() && self.search_bar.is_search_mode()
	}

	/// For the screenshot scenes: presses Escape in the search bar.
	pub fn escape_search(&self) {
		self.search_entry.emit_by_name::<()>("stop-search", &[]);
	}

	/// For the screenshot scenes: the conversations listed.
	pub fn listed(&self) -> Vec<RowView> {
		(0..self.store.n_items()).filter_map(|i| self.store.item(i)).map(|o| row_of(&o)).collect()
	}

	/// For the screenshot scenes: the list's title and the subjects it shows, top to bottom.
	pub fn shown(&self) -> (String, Vec<String>) {
		(self.list_title.title().to_string(), self.listed().into_iter().map(|r| r.subject).collect())
	}
}

/// What dragging the folder sidebar's edge to `w` pixels makes of it, from `last` (its width when
/// not a rail; 0 for the default): narrow enough, a rail that keeps `last` for when it widens again.
fn folders_drag(w: i32, last: u32) -> (u32, bool) {
	if w < RAIL_SNAP {
		(last, true)
	} else {
		((w as u32).clamp(FOLDERS.0, FOLDERS.1), false)
	}
}

/// How many conversations at the start and at the end of the list stay where they are, by key.
fn unchanged_ends<K: PartialEq>(old: &[K], new: &[K]) -> (usize, usize) {
	let head = old.iter().zip(new).take_while(|(a, b)| a == b).count();
	let tail = old[head..].iter().rev().zip(new[head..].iter().rev()).take_while(|(a, b)| a == b).count();
	(head, tail)
}

/// The start of a decoration layout such as `icon:minimize,close` holds nothing but the window's icon.
fn starts_with_icon_only(layout: &str) -> bool {
	layout.split(':').next().unwrap_or_default().split(',').all(|part| matches!(part.trim(), "" | "icon"))
}

fn unread_subtitle(unread: u32) -> String {
	match unread {
		0 => String::new(),
		n => format!("{n} unread"),
	}
}

#[cfg(test)]
mod tests {
	use super::*;

	#[test]
	fn dragging_the_folders_narrow_makes_a_rail() {
		assert_eq!(folders_drag(250, 0), (250, false));
		assert_eq!(folders_drag(150, 250), (FOLDERS.0, false));
		assert_eq!(folders_drag(900, 250), (FOLDERS.1, false));
		// Narrow enough: a rail, keeping the width for when it is dragged out again.
		assert_eq!(folders_drag(100, 250), (250, true));
		assert_eq!(folders_drag(-20, 0), (0, true));
	}

	#[test]
	fn only_an_icon_alone_at_the_start_is_moved_to_the_middle() {
		assert!(starts_with_icon_only("icon:minimize,maximize,close"));
		assert!(starts_with_icon_only(":close"));
		assert!(!starts_with_icon_only("close,minimize:"), "the buttons stay");
		assert!(!starts_with_icon_only("icon,menu:close"));
		assert!(!starts_with_icon_only("close"), "no colon: all at the start");
	}

	#[test]
	fn rows_that_stay_put_are_found_at_both_ends() {
		// A mail read: every row stays.
		assert_eq!(unchanged_ends(&["a", "b", "c"], &["a", "b", "c"]), (3, 0));
		// New mail on top.
		assert_eq!(unchanged_ends(&["a", "b", "c"], &["n", "a", "b", "c"]), (0, 3));
		// A conversation trashed from the middle.
		assert_eq!(unchanged_ends(&["a", "b", "c"], &["a", "c"]), (1, 1));
		// A reply moves a conversation to the top.
		assert_eq!(unchanged_ends(&["a", "b", "c", "d"], &["c", "a", "b", "d"]), (0, 1));
		assert_eq!(unchanged_ends::<&str>(&[], &["a"]), (0, 0));
	}
}

//! The signed-in view: folders, the conversation list and the reading pane.
//!
//! Everything shown comes from the cache, so switching folders is instant; the folder on show is
//! brought up to date in the background while a small spinner in its header turns.
//!
//! The list itself stays a GTK list view with its rows changed by hand: conversations still in place
//! keep their row, and with it the keyboard focus and the scroll.

use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::rc::Rc;

use adw::prelude::*;
use gtk::{gdk, gio, glib};
use relm4::factory::FactoryVecDeque;
use relm4::prelude::*;
use tern_app::view::{Action, AnswerKind, FolderIcon, FolderView, ListState, ListView, RowView, SearchView};
use tern_mail::FolderKind;

use crate::layout::Layout;
use crate::reader::{self, Reader};
use crate::{grip, search, window::Ctx};

/// The folder sidebar's width bounds, in pixels, and its width as a rail of icons. Dragged narrower
/// than `RAIL_SNAP` it becomes the rail.
const FOLDERS: (u32, u32) = (180, 400);
pub const RAIL: u32 = 52;
const RAIL_SNAP: i32 = 140;
/// The conversation list's width bounds.
const LIST: (u32, u32) = (280, 720);

type MailAction = fn(&mut Mailbox);
type RowAction = fn(&Mailbox, &RowView);

/// A folder in the sidebar, with its unread count beside its name, or on its icon on the rail.
pub struct FolderRow {
	folder: FolderView,
	/// Its Ctrl+number shortcut, if it has one.
	shortcut: Option<usize>,
	rail: bool,
}

impl FolderRow {
	/// On the rail, room around the icon for the count to sit on its corner.
	fn padding(&self) -> (i32, i32) {
		if self.rail {
			(7, 6)
		} else {
			(3, 0)
		}
	}

	fn tooltip(&self) -> String {
		let mut tip = self.folder.name.clone();
		if self.rail && self.folder.unread > 0 {
			tip = format!("{tip}, {}", unread_subtitle(self.folder.unread));
		}
		if let Some(n) = self.shortcut {
			tip = format!("{tip} (Ctrl+{n})");
		}
		tip
	}
}

#[relm4::factory(pub)]
impl FactoryComponent for FolderRow {
	type Init = (FolderView, Option<usize>, bool);
	type Input = ();
	type Output = ();
	type CommandOutput = ();
	type ParentWidget = gtk::ListBox;

	view! {
		// As a line (icon, name, count) or, on the rail, the icon alone with the count on its corner.
		gtk::Box {
			set_spacing: 12,
			#[watch]
			set_halign: if self.rail { gtk::Align::Center } else { gtk::Align::Fill },
			#[watch]
			set_margin_start: if self.rail { 0 } else { 6 },
			#[watch]
			set_margin_end: if self.rail { 0 } else { 6 },
			#[watch]
			set_tooltip_text: Some(&self.tooltip()),
			gtk::Overlay {
				#[wrap(Some)]
				set_child = &gtk::Image {
					set_icon_name: Some(folder_icon(self.folder.icon)),
					#[watch]
					set_margin_top: self.padding().0,
					#[watch]
					set_margin_bottom: self.padding().0,
					#[watch]
					set_margin_start: self.padding().1,
					#[watch]
					set_margin_end: self.padding().1,
				},
				add_overlay = &gtk::Label {
					set_css_classes: &["numeric", "rail-badge"],
					set_halign: gtk::Align::End,
					set_valign: gtk::Align::Start,
					#[watch]
					set_visible: self.folder.unread > 0 && self.rail,
					#[watch]
					set_label: &if self.folder.unread > 99 { "99+".to_string() } else { self.folder.unread.to_string() },
				},
			},
			gtk::Label {
				set_label: &self.folder.name,
				set_xalign: 0.0,
				set_hexpand: true,
				set_ellipsize: gtk::pango::EllipsizeMode::End,
				#[watch]
				set_visible: !self.rail,
			},
			gtk::Label {
				set_css_classes: &["dim-label", "numeric", "caption-heading"],
				#[watch]
				set_visible: self.folder.unread > 0 && !self.rail,
				#[watch]
				set_label: &self.folder.unread.to_string(),
			},
		}
	}

	fn init_model((folder, shortcut, rail): Self::Init, _: &DynamicIndex, _: FactorySender<Self>) -> Self {
		FolderRow { folder, shortcut, rail }
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
		FolderIcon::Label => "tern-label-symbolic",
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
	row.append(&gtk::Box::builder().spacing(4).margin_top(3).visible(false).build());
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
	let labels = subject.next_sibling().and_downcast::<gtk::Box>().unwrap();
	while let Some(child) = labels.first_child() {
		labels.remove(&child);
	}
	for name in t.labels.iter().take(3) {
		labels.append(&gtk::Label::builder().label(name).ellipsize(gtk::pango::EllipsizeMode::End).css_classes(["caption", "row-label"]).build());
	}
	if t.labels.len() > 3 {
		labels.append(&gtk::Label::builder().label(format!("+{}", t.labels.len() - 3)).css_classes(["caption", "dim-label"]).build());
	}
	labels.set_visible(!t.labels.is_empty());
	if t.unread {
		subject.remove_css_class("dim-label");
	} else {
		subject.add_css_class("dim-label");
	}
}

/// The conversation list, which its signals work on as they happen.
struct List {
	window: adw::ApplicationWindow,
	store: gio::ListStore,
	selection: gtk::SingleSelection,
	view: gtk::ListView,
	/// The row widget showing each conversation on screen, so a changed one is redrawn in place.
	bound: Rc<RefCell<HashMap<glib::Object, gtk::Box>>>,
	/// The conversation whose menu was opened last, which the menu's actions act on.
	menu_row: RefCell<Option<RowView>>,
	/// The folders and labels in the sidebar, in order.
	folders: Rc<RefCell<Vec<FolderView>>>,
	/// The last selection came from the keyboard: on a narrow window it does not leave the list.
	by_keyboard: Cell<bool>,
}

impl List {
	/// Opens the menu of the conversation under `(x, y)` in the list, if there is one there.
	fn row_menu_at(&self, x: f64, y: f64) -> bool {
		let Some(picked) = self.view.pick(x, y, gtk::PickFlags::DEFAULT) else { return false };
		let found = self.bound.borrow().iter().find(|(_, w)| picked == *w.upcast_ref::<gtk::Widget>() || picked.is_ancestor(*w)).map(|(o, w)| (o.clone(), w.clone()));
		let Some((obj, widget)) = found else { return false };
		let at = self.view.compute_point(&widget, &gtk::graphene::Point::new(x as f32, y as f32)).map(|p| (p.x() as f64, p.y() as f64));
		self.show_row_menu(&widget, row_of(&obj), at);
		true
	}

	/// Opens the menu of the conversation at `i` in the list, at `at` in its row or under it.
	fn open_row_menu(&self, i: u32, at: Option<(f64, f64)>) -> Option<gtk::PopoverMenu> {
		let obj = self.store.item(i)?;
		let widget = self.bound.borrow().get(&obj).cloned()?;
		Some(self.show_row_menu(&widget, row_of(&obj), at))
	}

	fn show_row_menu(&self, widget: &gtk::Box, row: RowView, at: Option<(f64, f64)>) -> gtk::PopoverMenu {
		let kind = self.folders.borrow().iter().find(|f| f.id == row.folder_id).map(|f| f.icon);
		let menu = gio::Menu::new();
		let section = gio::Menu::new();
		section.append(Some(if row.pinned { "Un_pin" } else { "_Pin" }), Some("row.pin"));
		if row.unread {
			section.append(Some("Mark as _Read"), Some("row.read"));
		} else {
			section.append(Some("Mark as _Unread"), Some("row.unread"));
		}
		section.append(Some("_Labels…"), Some("row.labels"));
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

	/// The row holding the keyboard focus, and whether it is unread.
	fn focused_row(&self) -> Option<(u32, bool)> {
		let focus = gtk::prelude::GtkWindowExt::focus(&self.window)?;
		let bound = self.bound.borrow();
		let (obj, _) = bound.iter().find(|(_, w)| w.is_ancestor(&focus) || **w == focus)?;
		let at = (0..self.store.n_items()).find(|&i| self.store.item(i).as_ref() == Some(obj))?;
		Some((at, row_of(obj).unread))
	}

	fn focus(&self) {
		if self.store.n_items() == 0 {
			return;
		}
		let at = match self.selection.selected() {
			gtk::INVALID_LIST_POSITION => 0,
			at => at,
		};
		self.view.scroll_to(at, gtk::ListScrollFlags::FOCUS, None);
	}
}

pub struct Mailbox {
	ctx: Ctx,
	pub store: gio::ListStore,
	pub reader: Rc<Controller<Reader>>,
	list: Rc<List>,
	outer: adw::OverlaySplitView,
	inner: adw::NavigationSplitView,
	account: adw::WindowTitle,
	folder_header: adw::HeaderBar,
	/// The app's icon, centered on the folders' header in place of the window's own when they are a
	/// rail: the window's sits at the header's start, off the rail's middle.
	rail_icon: gtk::Image,
	/// The main menu: on the folders' header, or on the list's while the folders are a rail.
	menus: (gtk::MenuButton, gtk::MenuButton),
	search_bar: gtk::SearchBar,
	search_entry: gtk::SearchEntry,
	folders: FactoryVecDeque<FolderRow>,
	/// The folders are a rail, as the header above them asks when it draws.
	rail: Rc<Cell<bool>>,
	/// How much Tern takes, when it takes more than set: the sidebar then offers to clean up.
	broom: Option<String>,
	/// What the core last said about the folder on show, and about a search.
	listed: Option<ListView>,
	found: Option<SearchView>,
	/// What the list's header and its place say now.
	title: String,
	subtitle: String,
	/// Which of loading, empty and list is on show.
	pane: &'static str,
	status: (String, Option<String>),
	/// Changing what is selected to match the core, which is not the person choosing anything.
	hold: Rc<Cell<bool>>,
	/// The conversation just chosen, until the core says it is open.
	asked: Rc<RefCell<Option<String>>>,
	/// The core has a search on show instead of the folder.
	searching: bool,
	/// A search was asked for, which the core may not have heard yet.
	search_open: bool,
	/// The last search the core put in the bar itself, as for a rule's line.
	search_revision: u64,
	/// Putting the core's text in the bar, which is not the person typing.
	writing_search: Rc<Cell<bool>>,
}

#[derive(Debug)]
pub enum Input {
	Start(String),
	Clear,
	/// Whether a search is on show instead of the folder.
	Searching(bool),
	List(Box<ListView>),
	Search(Box<SearchView>),
	Offer(Option<String>),
	/// What the mail actions apply to has changed.
	Current,
	FolderChosen(usize),
	FolderKey(i32),
	Run(MailAction),
	RunOnRow(RowAction),
	DragFolders(i32),
	DragList(i32),
	SaveLayout,
	NaturalList,
	Panes,
	SearchChanged(String),
	SearchEnter,
	SearchBarClosed,
}

#[relm4::component(pub)]
impl Component for Mailbox {
	type Init = Ctx;
	type Input = Input;
	type Output = ();
	type CommandOutput = ();

	view! {
		// Folders collapse into an overlay first, then list and reader into one column.
		adw::OverlaySplitView {
			set_min_sidebar_width: 200.0,
			set_max_sidebar_width: 260.0,
			set_sidebar_width_fraction: 0.2,
			#[wrap(Some)]
			set_sidebar = &adw::NavigationPage {
				set_title: "Folders",
				#[wrap(Some)]
				#[name = "folder_pane"]
				set_child = &gtk::Overlay {
					#[wrap(Some)]
					set_child = &adw::ToolbarView {
						#[local_ref]
						add_top_bar = &folder_header -> adw::HeaderBar {
							pack_end: &menus.0,
						},
						#[wrap(Some)]
						set_content = &gtk::ScrolledWindow {
							set_hscrollbar_policy: gtk::PolicyType::Never,
							set_vexpand: true,
							set_child: Some(&folder_list),
						},
						// The offer to clean up, at the foot of the folders: a broom with a dot, "Clean Up"
						// and how much Tern takes; on the rail, the broom alone.
						add_bottom_bar = &gtk::Button {
							set_action_name: Some("win.clean-up"),
							set_css_classes: &["flat", "cleanup"],
							update_property: &[gtk::accessible::Property::Label("Clean Up")],
							#[watch]
							set_visible: model.broom.is_some(),
							#[watch]
							set_halign: if model.rail.get() { gtk::Align::Center } else { gtk::Align::Fill },
							#[watch]
							set_tooltip_text: model.broom.as_ref().map(|size| format!("Clean Up — {size} on this computer")).as_deref(),
							#[wrap(Some)]
							set_child = &gtk::Box {
								set_spacing: 12,
								gtk::Overlay {
									set_valign: gtk::Align::Center,
									#[wrap(Some)]
									set_child = &gtk::Image {
										set_icon_name: Some("tern-broom-symbolic"),
									},
									add_overlay = &gtk::Box {
										add_css_class: "cleanup-dot",
										set_halign: gtk::Align::End,
										set_valign: gtk::Align::Start,
									},
								},
								gtk::Box {
									set_orientation: gtk::Orientation::Vertical,
									#[watch]
									set_visible: !model.rail.get(),
									gtk::Label {
										set_label: "Clean Up",
										set_xalign: 0.0,
									},
									gtk::Label {
										set_xalign: 0.0,
										set_css_classes: &["caption", "dim-label", "numeric"],
										#[watch]
										set_label: &model.broom.as_ref().map(|size| format!("{size} on this computer")).unwrap_or_default(),
									},
								},
							},
						},
					},
					#[name = "folder_grip"]
					add_overlay = &grip::handle() -> gtk::Box {},
				},
			},
			#[wrap(Some)]
			set_content = &adw::NavigationPage {
				set_title: "Mail",
				#[wrap(Some)]
				set_child = &crate::window::same(&inner) -> adw::NavigationSplitView {
					set_min_sidebar_width: 300.0,
					set_max_sidebar_width: 420.0,
					set_sidebar_width_fraction: 0.38,
					set_content: Some(&reader_page),
					#[wrap(Some)]
					set_sidebar = &adw::NavigationPage {
						set_tag: Some("list"),
						#[watch]
						set_title: &model.title,
						#[wrap(Some)]
						#[name = "list_pane"]
						set_child = &gtk::Overlay {
							#[wrap(Some)]
							set_child = &adw::ToolbarView {
								add_top_bar = &adw::HeaderBar {
									#[wrap(Some)]
									set_title_widget = &adw::WindowTitle {
										#[watch]
										set_title: &model.title,
										#[watch]
										set_subtitle: &model.subtitle,
									},
									#[name = "sidebar_toggle"]
									pack_start = &gtk::ToggleButton {
										set_icon_name: "sidebar-show-symbolic",
										set_tooltip_text: Some("Show Folders"),
									},
									pack_start = &gtk::Button {
										set_action_name: Some("win.compose"),
										set_tooltip_text: Some("New Message (Ctrl+N)"),
										add_css_class: "suggested-action",
										#[wrap(Some)]
										set_child = &gtk::Box {
											set_spacing: 6,
											gtk::Image {
												set_icon_name: Some("mail-message-new-symbolic"),
											},
											// Gives way on a narrow window, as does Refresh, which the main menu also has.
											#[name = "compose_word"]
											gtk::Label {
												set_label: "_New",
												set_use_underline: true,
											},
										},
									},
									pack_end: &menus.1,
									#[name = "refresh"]
									pack_end = &gtk::Button {
										set_icon_name: "view-refresh-symbolic",
										set_action_name: Some("win.refresh"),
										set_tooltip_text: Some("Check for New Mail (F5)"),
									},
									pack_end: &activity,
									pack_end: &search_button,
								},
								add_top_bar: &search_bar,
								#[wrap(Some)]
								set_content = &gtk::Stack {
									set_transition_type: gtk::StackTransitionType::Crossfade,
									set_transition_duration: 100,
									#[watch(skip_init)]
									set_visible_child_name: model.pane,
									add_named[Some("loading")] = &adw::Spinner {
										set_width_request: 32,
										set_height_request: 32,
										set_halign: gtk::Align::Center,
										set_valign: gtk::Align::Center,
									},
									add_named[Some("empty")] = &adw::StatusPage {
										set_icon_name: Some("mail-read-symbolic"),
										add_css_class: "compact",
										#[watch]
										set_title: &model.status.0,
										#[watch]
										set_description: model.status.1.as_deref(),
									},
									add_named[Some("list")] = &gtk::ScrolledWindow {
										set_hscrollbar_policy: gtk::PolicyType::Never,
										set_vexpand: true,
										set_child: Some(&list_view),
										connect_edge_reached[engine = model.ctx.engine.clone()] => move |_, pos| {
											if pos == gtk::PositionType::Bottom {
												engine.send(Action::LoadMore);
											}
										},
									},
								},
							},
							#[name = "list_grip"]
							add_overlay = &grip::handle() -> gtk::Box {},
						},
					},
				},
			},
		}
	}

	fn init(ctx: Ctx, root: Self::Root, sender: ComponentSender<Self>) -> ComponentParts<Self> {
		let menu = gio::Menu::new();
		let section = gio::Menu::new();
		section.append(Some("_Check for New Mail"), Some("win.refresh"));
		section.append(Some("_Preferences"), Some("app.preferences"));
		section.append(Some("_Keyboard Shortcuts"), Some("app.shortcuts"));
		section.append(Some("_About Tern"), Some("app.about"));
		menu.append_section(None, &section);
		let section = gio::Menu::new();
		section.append(Some("_Sign Out"), Some("app.sign-out"));
		menu.append_section(None, &section);
		let menu_button = || gtk::MenuButton::builder().icon_name("open-menu-symbolic").menu_model(&menu).tooltip_text("Main Menu").build();
		let menus = (menu_button(), menu_button());
		let account = adw::WindowTitle::new("Tern", "");
		let folder_header = adw::HeaderBar::builder().title_widget(&account).build();
		let inner = adw::NavigationSplitView::new();
		let (search_button, search_bar, search_entry) = search::widgets();
		let activity = ctx.activity.indicator();

		let store = gio::ListStore::new::<glib::BoxedAnyObject>();
		let selection = gtk::SingleSelection::builder().model(&store).autoselect(false).can_unselect(true).build();
		let bound: Rc<RefCell<HashMap<glib::Object, gtk::Box>>> = Rc::default();
		let list_view = gtk::ListView::builder().model(&selection).factory(&row_factory(&bound)).css_classes(["navigation-sidebar"]).build();
		list_view.update_property(&[gtk::accessible::Property::Label("Conversations")]);

		let reader = Rc::new(Reader::builder().launch(ctx.clone()).forward(sender.input_sender(), |_| Input::Current));
		let reader_page = reader.widget().clone();
		let folders = FactoryVecDeque::builder().launch(gtk::ListBox::default()).detach();
		let shown_folders: Rc<RefCell<Vec<FolderView>>> = Rc::default();
		let list = Rc::new(List { window: ctx.window.clone(), store: store.clone(), selection, view: list_view.clone(), bound, menu_row: RefCell::default(), folders: shown_folders, by_keyboard: Cell::new(false) });
		let mut model = Mailbox {
			store,
			reader,
			list,
			outer: root.clone(),
			inner: inner.clone(),
			account: account.clone(),
			folder_header: folder_header.clone(),
			rail_icon: gtk::Image::builder().icon_name(crate::APP_ID).pixel_size(16).build(),
			menus: menus.clone(),
			search_bar: search_bar.clone(),
			search_entry: search_entry.clone(),
			folders,
			rail: Rc::default(),
			broom: None,
			listed: None,
			found: None,
			title: "Inbox".into(),
			subtitle: String::new(),
			pane: "loading",
			status: ("No Messages".into(), None),
			hold: Rc::default(),
			asked: Rc::default(),
			searching: false,
			search_open: false,
			search_revision: 0,
			writing_search: Rc::default(),
			ctx,
		};
		let folder_list = model.folders.widget().clone();
		folder_list.add_css_class("navigation-sidebar");
		folder_list.update_property(&[gtk::accessible::Property::Label("Folders")]);
		let widgets = view_output!();

		root.bind_property("collapsed", &widgets.sidebar_toggle, "visible").sync_create().build();
		root.bind_property("show-sidebar", &widgets.sidebar_toggle, "active").sync_create().bidirectional().build();
		root.bind_property("collapsed", &widgets.folder_grip, "visible").invert_boolean().sync_create().build();
		model.inner.bind_property("collapsed", &widgets.list_grip, "visible").invert_boolean().sync_create().build();
		model.connect_signals(&sender);
		model.connect_grips(&widgets, &sender);
		model.connect_search(&sender);
		model.add_actions(&sender);
		model.apply_panes();
		let offers = sender.input_sender().clone();
		model.ctx.connect_offer_cleanup(move |size| offers.emit(Input::Offer(size.map(String::from))));
		ComponentParts { model, widgets }
	}

	fn update_with_view(&mut self, widgets: &mut Self::Widgets, input: Input, sender: ComponentSender<Self>, _: &Self::Root) {
		match input {
			Input::Start(address) => {
				self.account.set_subtitle(&address);
				self.ctx.window.set_title(Some(&format!("Tern — {address}")));
				self.set_enabled(true);
			},
			Input::Clear => self.clear(),
			Input::Searching(searching) => {
				if self.searching != searching {
					self.searching = searching;
					self.show_current();
				}
			},
			Input::List(view) => {
				self.show_folders(&view.folders, &view.labels);
				self.listed = Some(*view);
				if !self.searching {
					self.show_current();
				}
			},
			Input::Search(view) => self.show_search(*view),
			Input::Offer(size) => self.broom = size,
			Input::Current => self.update_actions(),
			Input::FolderChosen(at) => self.folder_chosen(at),
			Input::FolderKey(at) => {
				if let Some(row) = self.folders.widget().row_at_index(at) {
					self.folders.widget().select_row(Some(&row));
					self.list.focus();
				}
			},
			Input::Run(action) => action(self),
			Input::RunOnRow(action) => {
				let row = self.list.menu_row.borrow().clone();
				if let Some(row) = row {
					action(self, &row);
				}
			},
			Input::DragFolders(w) => {
				let (folders_width, folders_rail) = folders_drag(w, self.ctx.layout().folders_width);
				self.ctx.set_layout(Layout { folders_width, folders_rail, ..self.ctx.layout() }, false);
				self.apply_panes();
			},
			Input::DragList(w) => {
				let list_width = (w.max(0) as u32).clamp(LIST.0, LIST.1);
				self.ctx.set_layout(Layout { list_width, ..self.ctx.layout() }, false);
				self.apply_panes();
			},
			Input::SaveLayout => self.ctx.set_layout(self.ctx.layout(), true),
			Input::NaturalList => {
				self.ctx.set_layout(Layout { list_width: 0, ..self.ctx.layout() }, true);
				self.apply_panes();
			},
			Input::Panes => self.apply_panes(),
			Input::SearchChanged(text) => self.search_changed(text),
			Input::SearchEnter => {
				if self.searching && self.store.n_items() > 0 {
					self.list.selection.set_selected(0);
					self.inner.set_show_content(true);
				}
			},
			Input::SearchBarClosed => {
				if self.search_open {
					self.leave_search();
					self.ctx.engine.send(Action::CloseSearch);
					self.list.focus();
				}
			},
		}
		self.update_view(widgets, sender);
	}
}

/// What to do with the window's width: the two breakpoints, for the window to hold.
pub fn breakpoints(mailbox: &Controller<Mailbox>) -> Vec<adw::Breakpoint> {
	let model = mailbox.model();
	let widgets = mailbox.widgets();
	let medium = adw::Breakpoint::new(adw::BreakpointCondition::new_length(adw::BreakpointConditionLengthType::MaxWidth, 900.0, adw::LengthUnit::Sp));
	medium.add_setter(&model.outer, "collapsed", Some(&true.to_value()));
	let narrow = adw::Breakpoint::new(adw::BreakpointCondition::new_length(adw::BreakpointConditionLengthType::MaxWidth, 600.0, adw::LengthUnit::Sp));
	narrow.add_setter(&model.outer, "collapsed", Some(&true.to_value()));
	narrow.add_setter(&model.inner, "collapsed", Some(&true.to_value()));
	let (wide_only, more) = reader::narrow_setters(&model.reader);
	let list_wide_only: [gtk::Widget; 2] = [widgets.compose_word.clone().upcast(), widgets.refresh.clone().upcast()];
	for widget in wide_only.iter().chain(&list_wide_only) {
		narrow.add_setter(widget, "visible", Some(&false.to_value()));
	}
	narrow.add_setter(&more, "visible", Some(&true.to_value()));
	vec![medium, narrow]
}

/// The list's rows, made once and bound to whichever conversation scrolls into them.
fn row_factory(bound: &Rc<RefCell<HashMap<glib::Object, gtk::Box>>>) -> gtk::SignalListItemFactory {
	let factory = gtk::SignalListItemFactory::new();
	factory.connect_setup(|_, item| {
		let item = item.downcast_ref::<gtk::ListItem>().unwrap();
		item.set_child(Some(&row_widget()));
	});
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
	factory
}

/// The conversation was left: back in a narrow layout, or nothing selected.
fn close_conversation(asked: &RefCell<Option<String>>, reader: &Controller<Reader>, engine: &tern_app::Engine) {
	*asked.borrow_mut() = None;
	if reader.model().thread().is_some() {
		engine.send(Action::Back);
	}
}

impl Mailbox {
	/// Dragging a pane's edge resizes it; the folders become a rail of icons when dragged narrow,
	/// or on a double click. The widths are kept for next time.
	fn connect_grips(&self, widgets: &MailboxWidgets, sender: &ComponentSender<Self>) {
		let (s1, s2, s3) = (sender.clone(), sender.clone(), sender.clone());
		grip::connect(&widgets.folder_grip, &widgets.folder_pane, move |w| s1.input(Input::DragFolders(w)), move || s2.input(Input::SaveLayout), move || s3.input(Input::Run(Mailbox::toggle_rail)));
		let (s1, s2, s3) = (sender.clone(), sender.clone(), sender.clone());
		grip::connect(&widgets.list_grip, &widgets.list_pane, move |w| s1.input(Input::DragList(w)), move || s2.input(Input::SaveLayout), move || s3.input(Input::NaturalList));
		let panes = sender.clone();
		self.outer.connect_collapsed_notify(move |_| panes.input(Input::Panes));
		if let Some(settings) = gtk::Settings::default() {
			let panes = sender.input_sender().clone();
			settings.connect_gtk_decoration_layout_notify(move |_| panes.emit(Input::Panes));
		}
	}

	/// F9: folders to a rail of icons and back; on a narrow window, shows or hides them.
	fn toggle_rail(&mut self) {
		if self.outer.is_collapsed() {
			self.outer.set_show_sidebar(!self.outer.shows_sidebar());
			return;
		}
		let s = self.ctx.layout();
		self.ctx.set_layout(Layout { folders_rail: !s.folders_rail, ..s }, true);
		self.apply_panes();
	}

	/// Sizes the panes as last left. Up to a share of the window: a narrower window narrows them.
	fn apply_panes(&mut self) {
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
		let list = self.folders.widget().clone();
		if rail {
			list.add_css_class("rail");
		} else {
			list.remove_css_class("rail");
		}
		self.rail.set(rail);
		{
			let mut rows = self.folders.guard();
			for i in 0..rows.len() {
				if let Some(row) = rows.get_mut(i) {
					row.rail = rail;
				}
			}
		}
		list.invalidate_headers();
		self.inner.set_sidebar_width_unit(adw::LengthUnit::Px);
		let (min, max, fraction) = match s.list_width {
			0 => (300, 420, 0.38),
			w => (LIST.0.min(w), w, 0.6),
		};
		self.inner.set_min_sidebar_width(min as f64);
		self.inner.set_max_sidebar_width(max as f64);
		self.inner.set_sidebar_width_fraction(fraction);
	}

	/// The folders show as a rail of icons: only beside the list, not over it on a narrow window.
	pub fn is_rail(&self) -> bool {
		self.ctx.layout().folders_rail && !self.outer.is_collapsed()
	}

	fn connect_signals(&self, sender: &ComponentSender<Self>) {
		let folder_list = self.folders.widget();
		let (hold, chosen) = (self.hold.clone(), sender.clone());
		folder_list.connect_row_selected(move |_, row| {
			if let Some(row) = row.filter(|_| !hold.get()) {
				chosen.input(Input::FolderChosen(row.index() as usize));
			}
		});
		// A heading over the first label, which goes when the folders are a rail.
		let (folders, rail) = (self.list.folders.clone(), self.rail.clone());
		folder_list.set_header_func(move |row, before| {
			let folders = folders.borrow();
			let label = |i: i32| folders.get(i as usize).is_some_and(|f| f.icon == FolderIcon::Label);
			let first = label(row.index()) && !before.is_some_and(|b| label(b.index()));
			if first && !rail.get() {
				row.set_header(Some(&gtk::Label::builder().label("Labels").xalign(0.0).css_classes(["caption-heading", "dim-label"]).margin_top(12).margin_bottom(4).margin_start(12).build()));
			} else {
				row.set_header(None::<&gtk::Widget>);
			}
		});
		// Enter on a folder moves on to its conversations.
		let list = self.list.clone();
		folder_list.connect_row_activated(move |_, _| list.focus());

		let (list, hold, asked, inner, reader, engine) = (self.list.clone(), self.hold.clone(), self.asked.clone(), self.inner.clone(), self.reader.clone(), self.ctx.engine.clone());
		self.list.selection.connect_selection_changed(move |selection, _, _| {
			if hold.get() {
				return;
			}
			match selection.selected_item() {
				Some(obj) => {
					let row = row_of(&obj);
					if !row.open {
						*asked.borrow_mut() = Some(row.key.clone());
						engine.send(Action::OpenThread { key: row.key, folder_id: row.folder_id });
					}
					if !list.by_keyboard.get() {
						inner.set_show_content(true);
					}
				},
				None => close_conversation(&asked, &reader, &engine),
			}
			list.by_keyboard.set(false);
		});
		// Arrow keys move through the list without leaving it, even on a narrow window.
		let keys = gtk::EventControllerKey::new();
		keys.set_propagation_phase(gtk::PropagationPhase::Capture);
		let list = self.list.clone();
		keys.connect_key_pressed(move |_, key, _, state| {
			if matches!(key, gdk::Key::Up | gdk::Key::Down | gdk::Key::Page_Up | gdk::Key::Page_Down | gdk::Key::Home | gdk::Key::End) {
				list.by_keyboard.set(true);
			}
			// The menu key, or Shift+F10, opens the focused conversation's menu.
			let menu_key = key == gdk::Key::Menu || (key == gdk::Key::F10 && state.contains(gdk::ModifierType::SHIFT_MASK));
			if menu_key && list.focused_row().is_some_and(|(i, _)| list.open_row_menu(i, None).is_some()) {
				return glib::Propagation::Stop;
			}
			glib::Propagation::Proceed
		});
		self.list.view.add_controller(keys);
		// A right click, or a long press, opens a conversation's menu where it was pressed.
		let click = gtk::GestureClick::builder().button(gdk::BUTTON_SECONDARY).build();
		let list = self.list.clone();
		click.connect_pressed(move |gesture, _, x, y| {
			if list.row_menu_at(x, y) {
				gesture.set_state(gtk::EventSequenceState::Claimed);
			}
		});
		self.list.view.add_controller(click);
		let press = gtk::GestureLongPress::builder().touch_only(true).build();
		let list = self.list.clone();
		press.connect_pressed(move |gesture, x, y| {
			if list.row_menu_at(x, y) {
				gesture.set_state(gtk::EventSequenceState::Claimed);
			}
		});
		self.list.view.add_controller(press);
		// Enter (or a click on the selected row) opens the conversation and moves focus into it.
		let (inner, reader) = (self.inner.clone(), self.reader.clone());
		self.list.view.connect_activate(move |_, _| {
			if reader.model().edit_current_draft() {
				return;
			}
			inner.set_show_content(true);
			reader.model().focus();
		});
		// Going back in the narrow layout drops the selection, so tapping the same mail opens it again.
		let (list, hold, asked, reader, engine) = (self.list.clone(), self.hold.clone(), self.asked.clone(), self.reader.clone(), self.ctx.engine.clone());
		self.inner.connect_show_content_notify(move |inner| {
			if inner.is_collapsed() && !inner.shows_content() {
				hold.set(true);
				list.selection.set_selected(gtk::INVALID_LIST_POSITION);
				hold.set(false);
				close_conversation(&asked, &reader, &engine);
			}
		});
		// Escape in the reader goes back to the list.
		let keys = gtk::EventControllerKey::new();
		let (list, inner) = (self.list.clone(), self.inner.clone());
		keys.connect_key_pressed(move |_, key, _, _| {
			if key == gdk::Key::Escape {
				inner.set_show_content(false);
				list.focus();
				return glib::Propagation::Stop;
			}
			glib::Propagation::Proceed
		});
		self.reader.widget().add_controller(keys);
	}

	fn add_actions(&self, sender: &ComponentSender<Self>) {
		let entries: [(&str, MailAction); 16] = [
			("compose", |m| m.ctx.engine.send(Action::ComposeNew)),
			("reply", |m| m.answer(AnswerKind::Reply)),
			("reply-all", |m| m.answer(AnswerKind::ReplyAll)),
			("forward", |m| m.answer(AnswerKind::Forward)),
			("trash", |m| {
				m.ctx.engine.send(Action::ConversationTrash);
				m.list.focus();
			}),
			("spam", |m| m.ctx.engine.send(Action::ConversationSpam)),
			("labels", |m| m.ctx.engine.send(Action::ConversationLabels)),
			("refresh", |m| m.ctx.engine.send(Action::Refresh)),
			("mark-unread", |m| {
				m.ctx.engine.send(Action::ConversationUnread);
				m.list.focus();
			}),
			("open-in-tuta", |m| {
				if let Some(url) = m.reader.model().web_url() {
					crate::render::open_uri(&m.ctx.window, &url);
				}
			}),
			("next", |m| m.step(1)),
			("previous", |m| m.step(-1)),
			("focus-list", |m| m.list.focus()),
			("toggle-folders", Mailbox::toggle_rail),
			("search", |m| m.start_search()),
			("clean-up", |m| m.ctx.engine.send(Action::OpenCleanup)),
		];
		for (name, f) in entries {
			let action = gio::SimpleAction::new(name, None);
			let sender = sender.clone();
			action.connect_activate(move |_, _| sender.input(Input::Run(f)));
			self.ctx.window.add_action(&action);
		}
		// Ctrl+1…9 open the folders in order.
		let folder = gio::SimpleAction::new("folder", Some(glib::VariantTy::INT32));
		let keyed = sender.clone();
		folder.connect_activate(move |_, param| {
			if let Some(i) = param.and_then(|p| p.get::<i32>()) {
				keyed.input(Input::FolderKey(i));
			}
		});
		self.ctx.window.add_action(&folder);
		// What a conversation's menu does, to the conversation it was opened on.
		let row_actions: [(&str, RowAction); 6] = [
			("pin", |m, row| m.ctx.engine.send(Action::RowPin { key: row.key.clone() })),
			("read", |m, row| m.ctx.engine.send(Action::RowRead { key: row.key.clone(), folder_id: row.folder_id.clone(), read: true })),
			("unread", |m, row| m.ctx.engine.send(Action::RowRead { key: row.key.clone(), folder_id: row.folder_id.clone(), read: false })),
			("spam", |m, row| m.ctx.engine.send(Action::RowSpam { key: row.key.clone(), folder_id: row.folder_id.clone() })),
			("trash", |m, row| m.ctx.engine.send(Action::RowTrash { key: row.key.clone(), folder_id: row.folder_id.clone() })),
			("labels", |m, row| m.ctx.engine.send(Action::RowLabels { key: row.key.clone(), folder_id: row.folder_id.clone() })),
		];
		let group = gio::SimpleActionGroup::new();
		for (name, f) in row_actions {
			let action = gio::SimpleAction::new(name, None);
			let sender = sender.clone();
			action.connect_activate(move |_, _| sender.input(Input::RunOnRow(f)));
			group.add_action(&action);
		}
		self.list.view.insert_action_group("row", Some(&group));
		self.set_enabled(false);
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
		let reader = self.reader.model();
		let answerable = reader.opened().is_some_and(|c| !c.draft);
		for name in ["reply", "reply-all", "forward"] {
			self.action(name).set_enabled(answerable);
		}
		let open = reader.current().is_some() || reader.thread().is_some();
		for name in ["trash", "spam", "labels", "mark-unread", "open-in-tuta"] {
			self.action(name).set_enabled(open);
		}
	}

	fn clear(&mut self) {
		self.leave_search();
		self.hold.set(true);
		self.folders.guard().clear();
		self.hold.set(false);
		self.list.folders.borrow_mut().clear();
		self.listed = None;
		*self.asked.borrow_mut() = None;
		self.hold.set(true);
		self.store.remove_all();
		self.hold.set(false);
		self.reader.emit(reader::Input::Show(None));
		self.set_enabled(false);
	}

	fn answer(&self, kind: AnswerKind) {
		self.reader.model().answer(kind);
	}

	/// A folder or label was chosen in the sidebar.
	fn folder_chosen(&mut self, at: usize) {
		let Some(folder) = self.list.folders.borrow().get(at).cloned() else { return };
		if !folder.selected {
			self.leave_search();
			self.hold.set(true);
			self.list.selection.set_selected(gtk::INVALID_LIST_POSITION);
			self.store.remove_all();
			self.hold.set(false);
			self.inner.set_show_content(false);
			let label = folder.icon == FolderIcon::Label;
			self.ctx.engine.send(if label { Action::OpenLabel { id: folder.id } } else { Action::OpenFolder { id: folder.id } });
		}
		if self.outer.is_collapsed() {
			self.outer.set_show_sidebar(false);
		}
	}

	/// Draws what the core says about the search.
	fn show_search(&mut self, view: SearchView) {
		// The core has put words in the bar itself, as for a rule's line.
		let revised = std::mem::replace(&mut self.search_revision, view.revision) != view.revision;
		if revised && !view.query.is_empty() && self.search_entry.text() != view.query {
			self.writing_search.set(true);
			self.search_open = true;
			self.inner.set_show_content(false);
			self.search_bar.set_search_mode(true);
			self.search_entry.set_text(&view.query);
			self.search_entry.grab_focus();
			self.writing_search.set(false);
		}
		self.found = Some(view);
		if self.searching {
			self.show_current();
		}
	}

	/// Puts what is on show, the folder or the search, in the list.
	fn show_current(&mut self) {
		if self.searching {
			let Some(view) = self.found.clone() else { return };
			self.title.clone_from(&view.title);
			self.subtitle.clear();
			self.show_rows(&view.rows);
			if !view.rows.is_empty() {
				self.pane = "list";
			} else if view.empty {
				self.show_status(&view.empty_title, Some(&view.empty_detail));
			}
		} else {
			let Some(view) = self.listed.clone() else { return };
			self.title.clone_from(&view.title);
			let unread = view.folders.iter().find(|f| f.selected).map_or(0, |f| f.unread);
			self.subtitle = unread_subtitle(unread);
			self.show_rows(&view.rows);
			match view.state {
				ListState::Rows => self.pane = "list",
				ListState::Loading => self.pane = "loading",
				ListState::Empty | ListState::Failed => self.show_status(&view.empty_title, view.empty_detail.as_deref()),
			}
		}
		self.update_actions();
	}

	/// The folders in the sidebar, then the labels under their own heading.
	fn show_folders(&mut self, folders: &[FolderView], labels: &[FolderView]) {
		let all: Vec<FolderView> = folders.iter().chain(labels).cloned().collect();
		let unchanged = {
			let old = self.list.folders.borrow();
			old.len() == all.len() && old.iter().zip(&all).all(|(a, b)| a.id == b.id && a.name == b.name)
		};
		*self.list.folders.borrow_mut() = all.clone();
		let rail = self.is_rail();
		self.hold.set(true);
		{
			let mut rows = self.folders.guard();
			if unchanged {
				// Only counts changed: update labels in place.
				for (i, new) in all.iter().enumerate() {
					if let Some(row) = rows.get_mut(i) {
						row.folder = new.clone();
					}
				}
			} else {
				rows.clear();
				for (i, folder) in all.iter().enumerate() {
					rows.push_back((folder.clone(), (i < 9 && i < folders.len()).then_some(i + 1), rail));
				}
			}
		}
		let list = self.folders.widget();
		if !unchanged {
			list.invalidate_headers();
		}
		let on_show = all.iter().position(|f| f.selected).and_then(|i| list.row_at_index(i as i32));
		if list.selected_row() != on_show {
			list.select_row(on_show.as_ref());
		}
		self.hold.set(false);
	}

	/// Shows a status page in place of the list.
	fn show_status(&mut self, title: &str, description: Option<&str>) {
		self.status = (title.to_string(), description.map(String::from));
		self.pane = "empty";
	}

	/// Puts `rows` in the list, changing only the rows that changed, and selects the one open.
	fn show_rows(&self, rows: &[RowView]) {
		let old: Vec<RowView> = self.listed();
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
			if let Some(widget) = self.list.bound.borrow().get(&obj) {
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
		if self.list.selection.selected() != selected {
			self.list.selection.set_selected(selected);
		}
		self.hold.set(false);
	}

	fn connect_search(&self, sender: &ComponentSender<Self>) {
		let (writing, changed) = (self.writing_search.clone(), sender.clone());
		self.search_entry.connect_search_changed(move |entry| {
			if !writing.get() {
				changed.input(Input::SearchChanged(entry.text().to_string()));
			}
		});
		// Enter opens the best match.
		let enter = sender.clone();
		self.search_entry.connect_activate(move |_| enter.input(Input::SearchEnter));
		// Escape, the button or Ctrl+F again close the bar and go back to the folder.
		let closed = sender.clone();
		self.search_bar.connect_search_mode_enabled_notify(move |bar| {
			if !bar.is_search_mode() {
				closed.input(Input::SearchBarClosed);
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

	fn search_changed(&mut self, text: String) {
		if text.trim().is_empty() {
			// Cleared (also on the way to new text): back to the folder, the bar stays open.
			if std::mem::replace(&mut self.search_open, false) {
				self.ctx.engine.send(Action::CloseSearch);
			}
			return;
		}
		if !std::mem::replace(&mut self.search_open, true) {
			self.hold.set(true);
			self.list.selection.set_selected(gtk::INVALID_LIST_POSITION);
			self.store.remove_all();
			self.hold.set(false);
			self.ctx.engine.send(Action::OpenSearch);
		}
		*self.asked.borrow_mut() = None;
		self.ctx.engine.send(Action::SearchText { text });
	}

	/// Clears the bar, without telling the core; whoever calls this does, one way or another.
	fn leave_search(&mut self) {
		if std::mem::replace(&mut self.search_open, false) {
			self.writing_search.set(true);
			self.search_entry.set_text("");
			self.search_bar.set_search_mode(false);
			self.writing_search.set(false);
		}
	}

	/// Moves the selection by `delta` conversations, from anywhere in the window.
	fn step(&self, delta: i32) {
		let n = self.store.n_items() as i32;
		if n == 0 {
			return;
		}
		let at = self.list.selection.selected();
		let next = if at == gtk::INVALID_LIST_POSITION { 0 } else { (at as i32 + delta).clamp(0, n - 1) };
		self.list.by_keyboard.set(true);
		self.list.selection.set_selected(next as u32);
		self.list.view.scroll_to(next as u32, gtk::ListScrollFlags::FOCUS, None);
	}
}

/// For the screenshot scenes, which look behind the views.
impl Mailbox {
	/// The widths of the folder sidebar and the conversation list.
	pub fn pane_widths(&self) -> (i32, i32) {
		(self.outer.sidebar().map_or(0, |w| w.width()), self.inner.sidebar().map_or(0, |w| w.width()))
	}

	/// How far the middle of the icon on the folders' header is from the folders' left edge.
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

	pub fn open_row_menu(&self, i: u32, at: Option<(f64, f64)>) -> Option<gtk::PopoverMenu> {
		self.list.open_row_menu(i, at)
	}

	/// Right-clicks the middle of row `i`'s sender.
	pub fn right_click_row(&self, i: u32) -> bool {
		let Some(widget) = self.store.item(i).and_then(|obj| self.list.bound.borrow().get(&obj).cloned()) else { return false };
		let Some(sender) = widget.first_child().and_then(|top| top.first_child()).and_then(|dot| dot.next_sibling()) else { return false };
		let middle = gtk::graphene::Point::new(sender.width() as f32 / 2.0, sender.height() as f32 / 2.0);
		let Some(p) = sender.compute_point(&self.list.view, &middle) else { return false };
		self.list.row_menu_at(p.x() as f64, p.y() as f64)
	}

	/// Does what the conversation menu's item `name` does.
	pub fn row_action(&self, name: &str) -> Result<(), String> {
		self.list.view.activate_action(&format!("row.{name}"), None).map_err(|e| e.to_string())
	}

	/// Whether the sidebar offers to clean up.
	pub fn cleanup_offered(&self) -> bool {
		self.broom.is_some()
	}

	/// Opens the first folder of a kind.
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
		let index = self.list.folders.borrow().iter().position(|f| f.icon == icon);
		if let Some(row) = index.and_then(|i| self.folders.widget().row_at_index(i as i32)) {
			// A scene moves straight on to a mail in the new folder, not one left listed from the old.
			if !self.list.folders.borrow().get(row.index() as usize).is_some_and(|f| f.selected) {
				self.hold.set(true);
				self.list.selection.set_selected(gtk::INVALID_LIST_POSITION);
				self.store.remove_all();
				self.hold.set(false);
			}
			self.folders.widget().select_row(Some(&row));
		}
	}

	/// Opens a label by name.
	pub fn select_label(&self, name: &str) -> bool {
		let index = self.list.folders.borrow().iter().position(|f| f.icon == FolderIcon::Label && f.name == name);
		let row = index.and_then(|i| self.folders.widget().row_at_index(i as i32));
		if let Some(row) = &row {
			self.folders.widget().select_row(Some(row));
		}
		row.is_some()
	}

	/// The labels the row widget of conversation `i` shows.
	pub fn row_chips(&self, i: u32) -> Vec<String> {
		let Some(row) = self.store.item(i).and_then(|o| self.list.bound.borrow().get(&o).cloned()) else { return Vec::new() };
		let chips = row.last_child().and_downcast::<gtk::Box>();
		std::iter::successors(chips.and_then(|b| b.first_child()), |w| w.next_sibling()).filter_map(|w| w.downcast::<gtk::Label>().ok()).map(|l| l.label().to_string()).collect()
	}

	/// How narrow the list and the reader can go, apart.
	pub fn narrowest_parts(&self) -> (i32, i32) {
		let least = |w: Option<adw::NavigationPage>| w.map_or(0, |w| w.measure(gtk::Orientation::Horizontal, -1).0);
		(least(self.inner.sidebar()), least(self.inner.content()))
	}

	/// Selects the first conversation whose subject contains `text`.
	pub fn select_by_subject(&self, text: &str) -> bool {
		let found = (0..self.store.n_items()).find(|&i| self.store.item(i).is_some_and(|o| row_of(&o).subject.contains(text)));
		if let Some(i) = found {
			self.list.selection.set_selected(i);
		}
		found.is_some()
	}

	/// Moves the keyboard focus and selection to row `i`, as an arrow key does.
	pub fn arrow_to(&self, i: u32) {
		self.list.view.grab_focus();
		self.list.by_keyboard.set(true);
		self.list.view.scroll_to(i, gtk::ListScrollFlags::FOCUS | gtk::ListScrollFlags::SELECT, None);
	}

	/// The row holding the keyboard focus, and whether it is unread.
	pub fn focused_row(&self) -> Option<(u32, bool)> {
		self.list.focused_row()
	}

	/// The title of the status page shown instead of the list, if one is.
	pub fn status(&self) -> Option<String> {
		(self.pane == "empty").then(|| self.status.0.clone())
	}

	/// Types `text` into the search bar, which is open and has the focus.
	pub fn type_search(&self, text: &str) -> Result<(), String> {
		if !self.search_bar.is_search_mode() || !self.search_focused() {
			return Err("the search bar is not open with the focus in it".into());
		}
		self.search_entry.set_text(text);
		Ok(())
	}

	/// Types `more` after the text in the search bar, as typing does.
	pub fn append_search(&self, more: &str) {
		let mut end = self.search_entry.text().chars().count() as i32;
		self.search_entry.insert_text(more, &mut end);
	}

	/// The list shows search results, and the bar is open.
	pub fn is_searching(&self) -> bool {
		self.searching && self.search_bar.is_search_mode()
	}

	/// Presses Escape in the search bar.
	pub fn escape_search(&self) {
		self.search_entry.emit_by_name::<()>("stop-search", &[]);
	}

	/// The conversations listed.
	pub fn listed(&self) -> Vec<RowView> {
		(0..self.store.n_items()).filter_map(|i| self.store.item(i)).map(|o| row_of(&o)).collect()
	}

	/// The list's title and the subjects it shows, top to bottom.
	pub fn shown(&self) -> (String, Vec<String>) {
		(self.title.clone(), self.listed().into_iter().map(|r| r.subject).collect())
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

//! The reading pane: a conversation, one card per mail, newest first, as the core publishes it. The
//! mail chosen in the list and unread ones are open; the rest show one line each and open on click,
//! Enter or Space.
//!
//! A mail's text is drawn by `render` into a text view, by hand: that is not something a view can say.

use std::cell::RefCell;
use std::rc::Rc;
use std::sync::Arc;
use std::time::Duration;

use adw::prelude::*;
use gtk::glib;
use relm4::factory::{FactorySender, FactoryVecDeque};
use relm4::prelude::*;
use relm4::RelmWidgetExt;
use tern_app::view::{Action, AnswerKind, BlockKind, BlockView, CardView, ConversationView};
use tern_app::Engine;

use crate::{attachments, render, runtime, window::Ctx};

/// How much narrower a card's body is than the conversation column: its margins and border.
const CARD_INSET: i32 = 2 * 12 + 2;
/// How much narrower the conversation column is than its widest: its margins.
const COLUMN_INSET: i32 = 2 * 18;
/// The widest the conversation column goes.
const MAX_COLUMN: i32 = 840;

type Links = Rc<RefCell<Vec<(gtk::TextTag, String)>>>;

/// A pill button with an icon and a word, for the actions under a mail.
fn labeled(icon: &str, label: &str) -> gtk::Button {
	let content = adw::ButtonContent::builder().icon_name(icon).label(label).use_underline(true).build();
	gtk::Button::builder().child(&content).css_classes(["pill", "small-pill"]).build()
}

/// A mail's blocks with where their pictures are left out: a picture arriving is no reason to draw
/// the text again, which would lose what is selected in it.
fn without_paths(blocks: &[BlockView]) -> Vec<BlockView> {
	let mut blocks = blocks.to_vec();
	for block in &mut blocks {
		if let BlockKind::Image { path, .. } = &mut block.kind {
			*path = None;
		}
	}
	blocks
}

/// Opens a composer answering a mail. Lines selected in it are what a reply answers.
fn answer(engine: &Engine, body: &gtk::TextView, mail: tern_app::view::MailRef, kind: AnswerKind) {
	let picked = body.buffer().selection_bounds().map(|(start, end)| body.buffer().text(&start, &end, false).to_string());
	engine.send(Action::AnswerPicked { mail, kind, picked });
}

/// One mail of the conversation.
pub struct Card {
	ctx: Ctx,
	view: CardView,
	sender: FactorySender<Card>,
	header: gtk::Button,
	snippet: gtk::Label,
	body: gtk::TextView,
	attachments: gtk::FlowBox,
	actions: adw::WrapBox,
	/// A wait on the server has gone on long enough to show.
	waiting: bool,
	links: Links,
	/// Pictures with the block they show, as drawn.
	pictures: Vec<(gtk::Picture, u32)>,
	/// Inline images with their natural size, refitted when the pane changes width.
	natural: Vec<(gtk::Picture, (f64, f64))>,
	/// The image files already put in their pictures.
	shown_paths: Vec<u32>,
	/// Counts drawings of the body, so an image that arrives late for one is dropped.
	generation: u64,
	width: i32,
}

#[derive(Debug)]
pub enum CardInput {
	/// The wait has gone on for a moment.
	Waited,
	/// A picture's file has been read: its natural size, if it is a picture at all.
	Pictured { picture: gtk::Picture, natural: Option<(f64, f64)>, generation: u64 },
}

#[relm4::factory(pub)]
impl FactoryComponent for Card {
	type Init = (CardView, Ctx, i32);
	type Input = CardInput;
	type Output = ();
	type CommandOutput = ();
	type ParentWidget = gtk::Box;

	view! {
		gtk::Box {
			set_orientation: gtk::Orientation::Vertical,
			set_css_classes: &["card", "thread-card"],
			#[watch]
			set_class_active: ("current", self.view.current),

			append: &self.header,
			gtk::Revealer {
				set_transition_type: gtk::RevealerTransitionType::SlideDown,
				set_transition_duration: 150,
				#[watch]
				set_reveal_child: self.view.expanded,
				#[wrap(Some)]
				set_child = &gtk::Box {
					set_orientation: gtk::Orientation::Vertical,
					set_spacing: 12,
					set_margin_start: 12,
					set_margin_end: 12,
					set_margin_bottom: 12,
					gtk::Label {
						set_xalign: 0.0,
						set_wrap: true,
						set_wrap_mode: gtk::pango::WrapMode::WordChar,
						set_selectable: true,
						set_css_classes: &["caption", "dim-label"],
						#[watch]
						set_visible: self.view.recipients.is_some(),
						#[watch]
						set_label: self.view.recipients.as_deref().unwrap_or_default(),
					},
					append: &self.attachments,
					// Mail from the cache opens at once; only a wait on the server shows what is happening.
					adw::Spinner {
						set_width_request: 24,
						set_height_request: 24,
						set_halign: gtk::Align::Center,
						#[watch]
						set_visible: self.view.loading && self.waiting,
					},
					append: &self.body,
					append: &self.actions,
				},
			},
		}
	}

	fn init_model(init: Self::Init, _: &DynamicIndex, sender: FactorySender<Self>) -> Self {
		let (view, ctx, width) = init;
		let snippet = gtk::Label::builder().xalign(0.0).ellipsize(gtk::pango::EllipsizeMode::End).css_classes(["dim-label"]).visible(false).build();
		let header = card_header(&view, &snippet);
		let engine = ctx.engine.clone();
		let id = view.id.clone();
		header.connect_clicked(move |_| engine.send(Action::ToggleCard { id: id.clone() }));
		relm4::view! {
			body = gtk::TextView {
				set_editable: false,
				set_cursor_visible: false,
				set_wrap_mode: gtk::WrapMode::WordChar,
				set_pixels_below_lines: 2,
				add_css_class: "reader-body",
				set_accessible_role: gtk::AccessibleRole::Document,
			}
		}
		let links: Links = Rc::default();
		connect_links(&body, &links, &ctx.engine);
		let attachments = gtk::FlowBox::builder().selection_mode(gtk::SelectionMode::None).column_spacing(8).row_spacing(8).max_children_per_line(4).homogeneous(false).visible(false).build();
		// Wraps onto a second line on a narrow window.
		let actions = adw::WrapBox::builder().child_spacing(8).line_spacing(8).margin_top(6).build();
		if view.draft {
			let edit = labeled("document-edit-symbolic", "Continue _Editing");
			edit.add_css_class("suggested-action");
			let discard = labeled("user-trash-symbolic", "_Discard");
			let (engine, id) = (ctx.engine.clone(), view.id.clone());
			edit.connect_clicked(move |_| engine.send(Action::EditDraft { mail: id.clone() }));
			let (engine, id) = (ctx.engine.clone(), view.id.clone());
			discard.connect_clicked(move |_| engine.send(Action::DiscardDraft { mail: id.clone() }));
			actions.append(&edit);
			actions.append(&discard);
		} else {
			for (icon, label, kind) in [("tern-reply-symbolic", "_Reply", AnswerKind::Reply), ("tern-reply-all-symbolic", "Reply _All", AnswerKind::ReplyAll), ("tern-forward-symbolic", "_Forward", AnswerKind::Forward)] {
				let button = labeled(icon, label);
				let (engine, body, id) = (ctx.engine.clone(), body.clone(), view.id.clone());
				button.connect_clicked(move |_| answer(&engine, &body, id.clone(), kind));
				actions.append(&button);
			}
		}
		let view = CardView { blocks: Vec::new(), attachments: Vec::new(), ..view };
		Card { ctx, view, sender, header, snippet, body, attachments, actions, waiting: false, links, pictures: Vec::new(), natural: Vec::new(), shown_paths: Vec::new(), generation: 0, width }
	}

	fn update(&mut self, input: CardInput, _: FactorySender<Self>) {
		match input {
			CardInput::Waited => self.waiting = true,
			CardInput::Pictured { picture, natural, generation } => match natural {
				Some(natural) if generation == self.generation => {
					render::fit(&picture, natural, self.width - CARD_INSET);
					self.natural.push((picture, natural));
				},
				// A broken inline image shows nothing.
				_ => picture.set_visible(false),
			},
		}
	}
}

/// The line a card always shows: who, when, and a snippet while closed.
fn card_header(view: &CardView, snippet: &gtk::Label) -> gtk::Button {
	let tag = if view.draft { "Draft" } else { "Sent" };
	relm4::view! {
		header = gtk::Button {
			set_css_classes: &["flat", "card-header"],
			update_property: &[gtk::accessible::Property::Label(&format!("Message from {}", view.from))],
			#[wrap(Some)]
			set_child = &gtk::Box {
				set_spacing: 12,
				adw::Avatar::new(36, Some(&view.from), true) {
					set_valign: gtk::Align::Start,
				},
				gtk::Box {
					set_orientation: gtk::Orientation::Vertical,
					set_spacing: 2,
					set_hexpand: true,
					set_valign: gtk::Align::Center,
					gtk::Box {
						set_spacing: 6,
						gtk::Label {
							set_xalign: 0.0,
							set_ellipsize: gtk::pango::EllipsizeMode::End,
							add_css_class: "heading",
							set_label: &view.from,
						},
						gtk::Label {
							set_xalign: 0.0,
							set_ellipsize: gtk::pango::EllipsizeMode::End,
							add_css_class: "dim-label",
							set_label: &view.address,
							set_visible: view.from != view.address,
						},
						gtk::Label {
							set_label: tag,
							set_css_classes: if view.draft { &["caption-heading", "error"] } else { &["caption", "dim-label"] },
							set_visible: view.draft || view.sent,
						},
					},
					append: snippet,
				},
				gtk::Label {
					set_xalign: 1.0,
					set_valign: gtk::Align::Start,
					set_ellipsize: gtk::pango::EllipsizeMode::End,
					set_css_classes: &["caption", "dim-label", "numeric"],
					set_label: &view.date,
				},
			},
		}
	}
	header
}

impl Card {
	/// Brings the card up to what the core says, leaving what has not changed alone: focus, and what
	/// is selected in the text.
	fn apply(&mut self, new: &CardView) {
		let before = std::mem::replace(&mut self.view, new.clone());
		self.snippet.set_visible(new.snippet.is_some());
		self.snippet.set_label(new.snippet.as_deref().unwrap_or_default());
		if before.attachments != new.attachments {
			while let Some(child) = self.attachments.first_child() {
				self.attachments.remove(&child);
			}
			for attachment in &new.attachments {
				self.attachments.append(&attachments::chip(&self.ctx, &new.id, attachment));
			}
		}
		self.attachments.set_visible(!new.attachments.is_empty());
		if new.loading {
			self.body.set_visible(false);
			let sender = self.sender.input_sender().clone();
			glib::timeout_add_local_once(Duration::from_millis(150), move || sender.emit(CardInput::Waited));
		} else {
			self.waiting = false;
			self.body.set_visible(!new.blocks.is_empty());
		}
		if without_paths(&before.blocks) != without_paths(&new.blocks) {
			self.draw_body();
		} else {
			self.fill_pictures();
		}
	}

	fn draw_body(&mut self) {
		if self.view.blocks.is_empty() {
			return;
		}
		self.generation += 1;
		let rendered = render::render(&self.body, &self.view.blocks, &self.ctx.engine);
		settle(&self.body);
		*self.links.borrow_mut() = rendered.links;
		self.pictures = rendered.images;
		self.natural.clear();
		self.shown_paths.clear();
		self.fill_pictures();
	}

	/// Puts in the pictures whose files have arrived.
	fn fill_pictures(&mut self) {
		let scale = self.body.scale_factor();
		for (picture, index) in &self.pictures {
			let Some(BlockKind::Image { path: Some(path), .. }) = self.view.blocks.get(*index as usize).map(|b| &b.kind) else { continue };
			if self.shown_paths.contains(index) {
				continue;
			}
			self.shown_paths.push(*index);
			let (picture, path, generation) = (picture.clone(), path.clone(), self.generation);
			let sender = self.sender.input_sender().clone();
			runtime::spawn_local(async move {
				let pixels = runtime::run(async move { std::fs::read(path).ok().and_then(|bytes| render::decode(&bytes)) }).await;
				let natural = pixels.and_then(|px| render::show_pixels(&picture, px, scale));
				// The card may have gone by now.
				sender.emit(CardInput::Pictured { picture, natural, generation });
			});
		}
	}

	fn refit(&mut self, width: i32) {
		self.width = width;
		settle(&self.body);
		for (picture, natural) in &self.natural {
			render::fit(picture, *natural, width - CARD_INSET);
		}
	}
}

/// A text view outside a scrolled window keeps the height of its first layout, done before it
/// knows its width, until asked again: ask once it has laid out the text at its real width.
fn settle(body: &gtk::TextView) {
	let body = body.clone();
	glib::idle_add_local_once(move || body.queue_resize());
}

/// Clicking a link opens it; hovering shows where it goes.
fn connect_links(body: &gtk::TextView, links: &Links, engine: &Arc<Engine>) {
	let click = gtk::GestureClick::new();
	let (shown, known, engine) = (body.clone(), links.clone(), engine.clone());
	click.connect_released(move |gesture, n, x, y| {
		// A drag that selected text is not a click.
		if n != 1 || shown.buffer().has_selection() {
			return;
		}
		if let Some(url) = link_at(&shown, &known, x, y) {
			gesture.set_state(gtk::EventSequenceState::Claimed);
			engine.send(Action::OpenLink { url });
		}
	});
	body.add_controller(click);

	let motion = gtk::EventControllerMotion::new();
	let (shown, known) = (body.clone(), links.clone());
	motion.connect_motion(move |_, x, y| {
		let link = link_at(&shown, &known, x, y);
		shown.set_cursor_from_name(Some(if link.is_some() { "pointer" } else { "text" }));
		shown.set_tooltip_text(link.as_deref());
	});
	body.add_controller(motion);
}

fn link_at(body: &gtk::TextView, links: &Links, x: f64, y: f64) -> Option<String> {
	let (bx, by) = body.window_to_buffer_coords(gtk::TextWindowType::Widget, x as i32, y as i32);
	let iter = body.iter_at_location(bx, by)?;
	let links = links.borrow();
	iter.tags().iter().find_map(|t| links.iter().find(|(tag, _)| tag == t).map(|(_, url)| url.clone()))
}

pub struct Reader {
	ctx: Ctx,
	/// The conversation on show.
	view: Option<ConversationView>,
	cards: FactoryVecDeque<Card>,
	width: i32,
}

#[derive(Debug)]
pub enum Input {
	Show(Option<Box<ConversationView>>),
	/// The pane is this wide.
	Pane(i32),
	Redraw,
}

/// What the mail actions apply to has changed, or has finished loading.
#[derive(Debug)]
pub struct Current;

#[relm4::component(pub)]
impl Component for Reader {
	type Init = Ctx;
	type Input = Input;
	type Output = Current;
	type CommandOutput = ();

	view! {
		adw::NavigationPage {
			set_tag: Some("reader"),
			#[watch]
			set_title: model.view.as_ref().map_or("Conversation", |v| &v.subject),
			#[wrap(Some)]
			set_child = &adw::ToolbarView {
				add_top_bar = &adw::HeaderBar {
					set_show_title: false,
					pack_start = &gtk::Button {
						set_icon_name: "tern-reply-symbolic",
						set_action_name: Some("win.reply"),
						set_tooltip_text: Some("Reply (Ctrl+R)"),
					},
					#[name = "reply_all"]
					pack_start = &gtk::Button {
						set_icon_name: "tern-reply-all-symbolic",
						set_action_name: Some("win.reply-all"),
						set_tooltip_text: Some("Reply All (Ctrl+Shift+R)"),
					},
					#[name = "forward"]
					pack_start = &gtk::Button {
						set_icon_name: "tern-forward-symbolic",
						set_action_name: Some("win.forward"),
						set_tooltip_text: Some("Forward (Ctrl+L)"),
					},
					#[name = "more"]
					pack_end = &gtk::MenuButton {
						set_icon_name: "view-more-symbolic",
						set_menu_model: Some(&menu),
						set_tooltip_text: Some("More"),
					},
					// On a narrow window what does not fit beside the window's own buttons goes in the menu.
					#[name = "more_narrow"]
					pack_end = &gtk::MenuButton {
						set_icon_name: "view-more-symbolic",
						set_menu_model: Some(&narrow_menu),
						set_tooltip_text: Some("More"),
						set_visible: false,
					},
					#[name = "trash"]
					pack_end = &gtk::Button {
						set_icon_name: "user-trash-symbolic",
						set_action_name: Some("win.trash"),
						set_tooltip_text: Some("Move Conversation to Trash (Delete)"),
					},
					#[name = "spam"]
					pack_end = &gtk::Button {
						set_icon_name: "mail-mark-junk-symbolic",
						set_action_name: Some("win.spam"),
						set_tooltip_text: Some("Move to Spam (Ctrl+J)"),
					},
				},
				add_top_bar = &adw::Banner {
					set_title: "This message has formatting Tern can’t show",
					set_button_label: Some("Open in Tuta"),
					set_action_name: Some("win.open-in-tuta"),
					#[watch]
					set_revealed: model.view.as_ref().is_some_and(|v| v.cards.iter().any(|c| c.current && c.notice.is_some())),
				},
				#[wrap(Some)]
				set_content = &gtk::Stack {
					set_transition_type: gtk::StackTransitionType::Crossfade,
					set_transition_duration: 100,
					#[watch]
					set_visible_child_name: if model.view.is_some() { "thread" } else { "empty" },
					add_named[Some("empty")] = &adw::StatusPage {
						set_icon_name: Some("mail-read-symbolic"),
						set_title: "No Conversation Selected",
						add_css_class: "compact",
					},
					add_named[Some("thread")] = &gtk::Overlay {
						// A picture sized to fit must not hold the pane at that size, or the window could never narrow.
						#[name = "scroll"]
						#[wrap(Some)]
						set_child = &gtk::ScrolledWindow {
							set_hscrollbar_policy: gtk::PolicyType::External,
							set_vexpand: true,
							#[wrap(Some)]
							set_child = &adw::Clamp {
								set_maximum_size: 840,
								set_tightening_threshold: 600,
								#[wrap(Some)]
								set_child = &gtk::Box {
									set_orientation: gtk::Orientation::Vertical,
									set_spacing: 18,
									set_margin_top: 24,
									set_margin_bottom: 32,
									set_margin_start: 18,
									set_margin_end: 18,
									gtk::Box {
										set_orientation: gtk::Orientation::Vertical,
										set_spacing: 8,
										gtk::Label {
											set_xalign: 0.0,
											set_wrap: true,
											set_wrap_mode: gtk::pango::WrapMode::WordChar,
											set_selectable: true,
											add_css_class: "title-2",
											#[watch]
											set_label: model.view.as_ref().map_or("", |v| &v.subject),
										},
										#[name = "labels"]
										gtk::Box {
											set_spacing: 6,
											#[watch]
											set_visible: model.view.as_ref().is_some_and(|v| !v.labels.is_empty()),
										},
									},
									#[local_ref]
									cards_box -> gtk::Box {
										set_orientation: gtk::Orientation::Vertical,
										set_spacing: 12,
									},
								},
							},
						},
						// Over the pane, drawing nothing: says how wide the pane is, and so how wide the text can be.
						add_overlay = &gtk::DrawingArea {
							set_can_target: false,
							connect_resize[sender] => move |_, pane, _| sender.input(Input::Pane(pane)),
						},
					},
				},
			},
		}
	}

	fn init(ctx: Ctx, root: Self::Root, sender: ComponentSender<Self>) -> ComponentParts<Self> {
		let menu = gtk::gio::Menu::new();
		menu.append(Some("_Labels…"), Some("win.labels"));
		menu.append(Some("Mark as _Unread"), Some("win.mark-unread"));
		menu.append(Some("_Open in Tuta"), Some("win.open-in-tuta"));
		let narrow_menu = gtk::gio::Menu::new();
		let answers = gtk::gio::Menu::new();
		answers.append(Some("Reply _All"), Some("win.reply-all"));
		answers.append(Some("_Forward"), Some("win.forward"));
		narrow_menu.append_section(None, &answers);
		let moves = gtk::gio::Menu::new();
		moves.append(Some("Move to _Spam…"), Some("win.spam"));
		moves.append(Some("Move to _Trash"), Some("win.trash"));
		narrow_menu.append_section(None, &moves);
		narrow_menu.append_section(None, &menu);

		let cards = FactoryVecDeque::builder().launch(gtk::Box::default()).detach();
		let model = Reader { ctx, view: None, cards, width: 600 };
		let cards_box = model.cards.widget();
		let widgets = view_output!();
		let style = adw::StyleManager::default();
		let redraw = sender.input_sender().clone();
		style.connect_dark_notify(move |_| redraw.emit(Input::Redraw));
		let redraw = sender.input_sender().clone();
		style.connect_accent_color_notify(move |_| redraw.emit(Input::Redraw));
		ComponentParts { model, widgets }
	}

	fn update_with_view(&mut self, widgets: &mut Self::Widgets, input: Input, sender: ComponentSender<Self>, _: &Self::Root) {
		match input {
			Input::Show(view) => {
				self.show(widgets, view.map(|v| *v));
				let _ = sender.output(Current);
			},
			Input::Pane(pane) => {
				let width = pane.min(MAX_COLUMN) - COLUMN_INSET;
				if width > 0 && width != self.width {
					self.width = width;
					let mut cards = self.cards.guard();
					for i in 0..cards.len() {
						if let Some(card) = cards.get_mut(i) {
							card.refit(width);
						}
					}
				}
			},
			Input::Redraw => {
				let mut cards = self.cards.guard();
				for i in 0..cards.len() {
					if let Some(card) = cards.get_mut(i) {
						card.draw_body();
					}
				}
			},
		}
		self.update_view(widgets, sender);
	}
}

impl Reader {
	/// Draws what the core says is open.
	fn show(&mut self, widgets: &ReaderWidgets, view: Option<ConversationView>) {
		let Some(view) = view else {
			self.cards.guard().clear();
			self.view = None;
			return;
		};
		let same_key = self.view.as_ref().is_some_and(|v| v.key == view.key);
		let same_cards = same_key && self.cards.len() == view.cards.len() && self.cards.iter().zip(&view.cards).all(|(a, b)| a.view.id == b.id && a.view.draft == b.draft);
		{
			let mut cards = self.cards.guard();
			if !same_cards {
				cards.clear();
				for card in &view.cards {
					cards.push_back((card.clone(), self.ctx.clone(), self.width));
				}
			}
			for (i, new) in view.cards.iter().enumerate() {
				if let Some(card) = cards.get_mut(i) {
					card.apply(new);
				}
			}
		}
		if self.view.as_ref().map(|v| &v.labels) != Some(&view.labels) {
			while let Some(child) = widgets.labels.first_child() {
				widgets.labels.remove(&child);
			}
			for label in &view.labels {
				widgets.labels.append(&gtk::Label::builder().label(label).css_classes(["caption", "row-label"]).build());
			}
		}
		self.view = Some(view);
		if !same_key {
			if let Some(card) = self.current_card() {
				scroll_to(&widgets.scroll, &card.header);
			}
		}
	}

	fn current_card(&self) -> Option<&Card> {
		self.cards.iter().find(|c| c.view.current)
	}

	/// The conversation on show, by key.
	pub fn thread(&self) -> Option<String> {
		self.view.as_ref().map(|v| v.key.clone())
	}

	/// The mails of the conversation on show, newest first.
	pub fn mails(&self) -> Vec<CardView> {
		self.cards.iter().map(|c| c.view.clone()).collect()
	}

	/// The mail actions apply to.
	pub fn current(&self) -> Option<CardView> {
		self.current_card().map(|c| c.view.clone())
	}

	/// The mail actions apply to, once it has opened.
	pub fn opened(&self) -> Option<CardView> {
		self.current().filter(|c| c.open_url.is_some())
	}

	/// For the screenshot scenes: selects `text` in the open mail, as dragging over it does.
	pub fn select_in_current(&self, text: &str) -> bool {
		let Some(card) = self.current_card() else { return false };
		let buffer = card.body.buffer();
		let Some((start, end)) = buffer.start_iter().forward_search(text, gtk::TextSearchFlags::empty(), None) else { return false };
		buffer.select_range(&start, &end);
		true
	}

	/// Opens a composer answering the current mail.
	pub fn answer(&self, kind: AnswerKind) {
		if let Some(card) = self.current_card() {
			answer(&self.ctx.engine, &card.body, card.view.id.clone(), kind);
		}
	}

	/// Opens the current mail in a composer when it is a draft, and says whether it is.
	pub fn edit_current_draft(&self) -> bool {
		let Some(card) = self.current().filter(|c| c.draft) else { return false };
		self.ctx.engine.send(Action::EditDraft { mail: card.id });
		true
	}

	pub fn web_url(&self) -> Option<String> {
		self.current().and_then(|c| c.open_url)
	}

	/// Moves keyboard focus into the conversation, onto the current mail.
	pub fn focus(&self) -> bool {
		self.current_card().is_some_and(|c| c.header.grab_focus())
	}
}

/// What to hide on a narrow window, and what to show there instead.
pub fn narrow_setters(reader: &Controller<Reader>) -> (Vec<gtk::Widget>, gtk::MenuButton) {
	let w = reader.widgets();
	(vec![w.reply_all.clone().upcast(), w.forward.clone().upcast(), w.more.clone().upcast(), w.trash.clone().upcast(), w.spam.clone().upcast()], w.more_narrow.clone())
}

fn scroll_to(scroll: &gtk::ScrolledWindow, header: &gtk::Button) {
	let (scroll, header) = (scroll.clone(), header.clone());
	// After layout, so the card has its place.
	glib::idle_add_local_once(move || {
		let Some(child) = scroll.child() else { return };
		if let Some(p) = header.compute_point(&child, &gtk::graphene::Point::new(0.0, 0.0)) {
			scroll.vadjustment().set_value((p.y() as f64 - 12.0).max(0.0));
		}
	});
}

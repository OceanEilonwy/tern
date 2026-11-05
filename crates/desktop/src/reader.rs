//! The reading pane: a conversation, one card per mail, newest first, as the core publishes it. The
//! mail chosen in the list and unread ones are open; the rest show one line each and open on click,
//! Enter or Space.

use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::time::Duration;

use adw::prelude::*;
use gtk::glib;
use tern_app::view::{Action, BlockKind, BlockView, CardView, ConversationView, MailRef};
use tern_compose::Answer;
use tern_html::Doc;
use tern_mail::MailId;

use crate::{attachments, composer, render, runtime, window::Ctx};

/// How much narrower a card's body is than the conversation column: its margins and border.
const CARD_INSET: i32 = 2 * 12 + 2;

/// One mail of the conversation.
struct Card {
	view: RefCell<CardView>,
	root: gtk::Box,
	header: gtk::Button,
	snippet: gtk::Label,
	revealer: gtk::Revealer,
	recipients: gtk::Label,
	attachments: gtk::FlowBox,
	body: gtk::TextView,
	loading: adw::Spinner,
	links: RefCell<Vec<(gtk::TextTag, String)>>,
	/// Pictures with the block they show, as drawn.
	pictures: RefCell<Vec<(gtk::Picture, u32)>>,
	/// Inline images with their natural size, refitted when the pane changes width.
	natural: RefCell<Vec<(gtk::Picture, (f64, f64))>>,
	/// The image files already put in their pictures.
	shown_paths: RefCell<Vec<(u32, String)>>,
	/// Counts drawings of the body, so an image that arrives late for one is dropped.
	generation: Cell<u64>,
}

pub struct Reader {
	ctx: Ctx,
	pub page: adw::NavigationPage,
	stack: gtk::Stack,
	banner: adw::Banner,
	subject: gtk::Label,
	cards_box: gtk::Box,
	scroll: gtk::ScrolledWindow,
	cards: RefCell<Vec<Rc<Card>>>,
	width: Cell<i32>,
	/// The conversation on show.
	key: RefCell<Option<String>>,
	on_current: RefCell<Option<Box<dyn Fn()>>>,
}

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

impl Reader {
	pub fn new(ctx: Ctx) -> Rc<Reader> {
		let header = adw::HeaderBar::builder().show_title(false).build();
		for (icon, action, tip) in [
			("tern-reply-symbolic", "win.reply", "Reply (Ctrl+R)"),
			("tern-reply-all-symbolic", "win.reply-all", "Reply All (Ctrl+Shift+R)"),
			("tern-forward-symbolic", "win.forward", "Forward (Ctrl+L)"),
		] {
			header.pack_start(&gtk::Button::builder().icon_name(icon).action_name(action).tooltip_text(tip).build());
		}
		let menu = gtk::gio::Menu::new();
		menu.append(Some("Mark as _Unread"), Some("win.mark-unread"));
		menu.append(Some("_Open in Tuta"), Some("win.open-in-tuta"));
		header.pack_end(&gtk::MenuButton::builder().icon_name("view-more-symbolic").menu_model(&menu).tooltip_text("More").build());
		header.pack_end(&gtk::Button::builder().icon_name("user-trash-symbolic").action_name("win.trash").tooltip_text("Move Conversation to Trash (Delete)").build());
		header.pack_end(&gtk::Button::builder().icon_name("mail-mark-junk-symbolic").action_name("win.spam").tooltip_text("Move to Spam (Ctrl+J)").build());

		let banner = adw::Banner::builder().title("This message has formatting Tern can’t show").button_label("Open in Tuta").build();
		banner.set_action_name(Some("win.open-in-tuta"));

		let subject = gtk::Label::builder().xalign(0.0).wrap(true).wrap_mode(gtk::pango::WrapMode::WordChar).selectable(true).css_classes(["title-2"]).build();
		let cards_box = gtk::Box::builder().orientation(gtk::Orientation::Vertical).spacing(12).build();
		let column = gtk::Box::builder().orientation(gtk::Orientation::Vertical).spacing(18).margin_top(24).margin_bottom(32).margin_start(18).margin_end(18).build();
		column.append(&subject);
		column.append(&cards_box);
		// Zero-height, full-width: tells inline images how wide the text is.
		let probe = gtk::DrawingArea::builder().hexpand(true).build();
		column.append(&probe);
		let clamp = adw::Clamp::builder().maximum_size(840).tightening_threshold(600).child(&column).build();
		let scroll = gtk::ScrolledWindow::builder().hscrollbar_policy(gtk::PolicyType::Never).child(&clamp).vexpand(true).build();

		let empty = adw::StatusPage::builder().icon_name("mail-read-symbolic").title("No Conversation Selected").css_classes(["compact"]).build();
		let stack = gtk::Stack::builder().transition_type(gtk::StackTransitionType::Crossfade).transition_duration(100).build();
		stack.add_named(&empty, Some("empty"));
		stack.add_named(&scroll, Some("thread"));

		let toolbar = adw::ToolbarView::new();
		toolbar.add_top_bar(&header);
		toolbar.add_top_bar(&banner);
		toolbar.set_content(Some(&stack));
		let page = adw::NavigationPage::builder().title("Conversation").tag("reader").child(&toolbar).build();

		let reader = Rc::new(Reader { ctx, page, stack, banner, subject, cards_box, scroll, cards: RefCell::default(), width: Cell::new(600), key: RefCell::default(), on_current: RefCell::default() });
		let weak = Rc::downgrade(&reader);
		probe.connect_resize(move |_, w, _| {
			let Some(this) = weak.upgrade() else { return };
			if w > 0 && w != this.width.get() {
				this.width.set(w);
				for card in this.cards.borrow().iter() {
					settle(&card.body);
					for (picture, natural) in card.natural.borrow().iter() {
						render::fit(picture, *natural, w - CARD_INSET);
					}
				}
			}
		});
		let weak = Rc::downgrade(&reader);
		let sm = adw::StyleManager::default();
		let redraw = move || {
			if let Some(r) = weak.upgrade() {
				for card in r.cards.borrow().clone() {
					r.draw_body(&card);
				}
			}
		};
		let r2 = redraw.clone();
		sm.connect_dark_notify(move |_| r2());
		sm.connect_accent_color_notify(move |_| redraw());
		reader
	}

	/// Called when the mail actions apply to changes, or finishes loading.
	pub fn connect_current(&self, f: impl Fn() + 'static) {
		*self.on_current.borrow_mut() = Some(Box::new(f));
	}

	/// The conversation on show, by key.
	pub fn thread(&self) -> Option<String> {
		self.key.borrow().clone()
	}

	/// The mails of the conversation on show, newest first.
	pub fn mails(&self) -> Vec<CardView> {
		self.cards.borrow().iter().map(|c| c.view.borrow().clone()).collect()
	}

	/// The mail actions apply to.
	pub fn current(&self) -> Option<CardView> {
		self.current_card().map(|c| c.view.borrow().clone())
	}

	/// The mail actions apply to, once it has opened.
	pub fn opened(&self) -> Option<CardView> {
		self.current().filter(|c| c.open_url.is_some())
	}

	fn clear(&self) {
		*self.key.borrow_mut() = None;
		for card in self.cards.borrow_mut().drain(..) {
			// Images still in flight for it find it gone.
			card.generation.set(card.generation.get() + 1);
		}
		while let Some(child) = self.cards_box.first_child() {
			self.cards_box.remove(&child);
		}
		self.banner.set_revealed(false);
		self.stack.set_visible_child_name("empty");
		self.page.set_title("Conversation");
	}

	/// Draws what the core says is open.
	pub fn show(self: &Rc<Self>, view: Option<&ConversationView>) {
		let Some(view) = view else {
			self.clear();
			self.told_current();
			return;
		};
		let same_key = self.key.borrow().as_deref() == Some(view.key.as_str());
		let same_cards = same_key && {
			let cards = self.cards.borrow();
			cards.len() == view.cards.len() && cards.iter().zip(&view.cards).all(|(a, b)| a.view.borrow().id == b.id && a.view.borrow().draft == b.draft)
		};
		if !same_cards {
			self.clear();
			*self.key.borrow_mut() = Some(view.key.clone());
			for card in &view.cards {
				let built = self.card(card);
				self.cards_box.append(&built.root);
				self.cards.borrow_mut().push(built);
			}
		}
		self.page.set_title(&view.subject);
		self.subject.set_label(&view.subject);
		self.stack.set_visible_child_name("thread");
		let cards = self.cards.borrow().clone();
		for (card, new) in cards.iter().zip(&view.cards) {
			self.update(card, new);
		}
		self.banner.set_revealed(view.cards.iter().any(|c| c.current && c.notice.is_some()));
		if !same_key {
			if let Some(card) = self.current_card() {
				self.scroll_to(&card);
			}
		}
		self.told_current();
	}

	fn told_current(&self) {
		if let Some(f) = self.on_current.borrow().as_ref() {
			f();
		}
	}

	fn card(self: &Rc<Self>, s: &CardView) -> Rc<Card> {
		let avatar = adw::Avatar::new(36, Some(&s.from), true);
		avatar.set_valign(gtk::Align::Start);
		let from = gtk::Label::builder().xalign(0.0).ellipsize(gtk::pango::EllipsizeMode::End).css_classes(["heading"]).label(&s.from).build();
		let address = gtk::Label::builder().xalign(0.0).ellipsize(gtk::pango::EllipsizeMode::End).css_classes(["dim-label"]).label(&s.address).build();
		address.set_visible(s.from != s.address);
		let date = gtk::Label::builder().xalign(1.0).valign(gtk::Align::Start).ellipsize(gtk::pango::EllipsizeMode::End).css_classes(["caption", "dim-label", "numeric"]).label(&s.date).build();
		let names = gtk::Box::new(gtk::Orientation::Horizontal, 6);
		names.append(&from);
		names.append(&address);
		if s.draft {
			names.append(&gtk::Label::builder().label("Draft").css_classes(["caption-heading", "error"]).build());
		} else if s.sent {
			names.append(&gtk::Label::builder().label("Sent").css_classes(["caption", "dim-label"]).build());
		}
		let snippet = gtk::Label::builder().xalign(0.0).ellipsize(gtk::pango::EllipsizeMode::End).css_classes(["dim-label"]).build();
		let who = gtk::Box::builder().orientation(gtk::Orientation::Vertical).spacing(2).hexpand(true).valign(gtk::Align::Center).build();
		who.append(&names);
		who.append(&snippet);
		let head = gtk::Box::new(gtk::Orientation::Horizontal, 12);
		head.append(&avatar);
		head.append(&who);
		head.append(&date);
		let header = gtk::Button::builder().child(&head).css_classes(["flat", "card-header"]).build();
		header.update_property(&[gtk::accessible::Property::Label(&format!("Message from {}", s.from))]);

		let recipients = gtk::Label::builder().xalign(0.0).wrap(true).wrap_mode(gtk::pango::WrapMode::WordChar).selectable(true).css_classes(["caption", "dim-label"]).visible(false).build();
		let attachments_box = gtk::FlowBox::builder().selection_mode(gtk::SelectionMode::None).column_spacing(8).row_spacing(8).max_children_per_line(4).homogeneous(false).visible(false).build();
		let body = gtk::TextView::builder().editable(false).cursor_visible(false).wrap_mode(gtk::WrapMode::WordChar).pixels_below_lines(2).css_classes(["reader-body"]).build();
		body.set_accessible_role(gtk::AccessibleRole::Document);
		let loading = adw::Spinner::builder().width_request(24).height_request(24).halign(gtk::Align::Center).visible(false).build();

		// Wraps onto a second line on a narrow window.
		let actions = adw::WrapBox::builder().child_spacing(8).line_spacing(8).margin_top(6).build();
		let details = gtk::Box::builder().orientation(gtk::Orientation::Vertical).spacing(12).margin_start(12).margin_end(12).margin_bottom(12).build();
		details.append(&recipients);
		details.append(&attachments_box);
		details.append(&loading);
		details.append(&body);
		details.append(&actions);
		let revealer = gtk::Revealer::builder().transition_type(gtk::RevealerTransitionType::SlideDown).transition_duration(150).child(&details).build();

		let root = gtk::Box::builder().orientation(gtk::Orientation::Vertical).css_classes(["card", "thread-card"]).build();
		root.append(&header);
		root.append(&revealer);

		let card = Rc::new(Card {
			view: RefCell::new(CardView { blocks: Vec::new(), attachments: Vec::new(), ..s.clone() }),
			root,
			header,
			snippet,
			revealer,
			recipients,
			attachments: attachments_box,
			body,
			loading,
			links: RefCell::default(),
			pictures: RefCell::default(),
			natural: RefCell::default(),
			shown_paths: RefCell::default(),
			generation: Cell::new(0),
		});

		if s.draft {
			let edit = labeled("document-edit-symbolic", "Continue _Editing");
			edit.add_css_class("suggested-action");
			let discard = labeled("user-trash-symbolic", "_Discard");
			let (weak, id) = (Rc::downgrade(self), s.id.clone());
			edit.connect_clicked(move |_| {
				if let Some(this) = weak.upgrade() {
					let id = id.clone();
					runtime::spawn_local(async move {
						this.edit_draft(&id).await;
					});
				}
			});
			let (engine, id) = (self.ctx.engine.clone(), s.id.clone());
			discard.connect_clicked(move |_| engine.send(Action::DiscardDraft { mail: id.clone() }));
			actions.append(&edit);
			actions.append(&discard);
		} else {
			for (icon, label, answer) in [("tern-reply-symbolic", "_Reply", Answer::Reply), ("tern-reply-all-symbolic", "Reply _All", Answer::ReplyAll), ("tern-forward-symbolic", "_Forward", Answer::Forward)] {
				let button = labeled(icon, label);
				let (weak, c) = (Rc::downgrade(self), Rc::downgrade(&card));
				button.connect_clicked(move |_| {
					if let (Some(this), Some(card)) = (weak.upgrade(), c.upgrade()) {
						runtime::spawn_local(async move {
							this.answer_card(&card, answer).await;
						});
					}
				});
				actions.append(&button);
			}
		}

		let (engine, id) = (self.ctx.engine.clone(), s.id.clone());
		card.header.connect_clicked(move |_| engine.send(Action::ToggleCard { id: id.clone() }));
		connect_links(&card);
		card
	}

	/// Brings a card up to what the core says, leaving what has not changed alone: focus, and what
	/// is selected in the text.
	fn update(self: &Rc<Self>, card: &Rc<Card>, new: &CardView) {
		let before = card.view.replace(new.clone());
		card.revealer.set_reveal_child(new.expanded);
		card.snippet.set_visible(new.snippet.is_some());
		card.snippet.set_label(new.snippet.as_deref().unwrap_or_default());
		card.recipients.set_visible(new.recipients.is_some());
		card.recipients.set_label(new.recipients.as_deref().unwrap_or_default());
		if new.current {
			card.root.add_css_class("current");
		} else {
			card.root.remove_css_class("current");
		}
		if before.attachments != new.attachments {
			while let Some(child) = card.attachments.first_child() {
				card.attachments.remove(&child);
			}
			for att in &new.attachments {
				card.attachments.append(&attachments::chip(&self.ctx, &new.id, att));
			}
		}
		card.attachments.set_visible(!new.attachments.is_empty());
		// Mail from the cache opens at once; only a wait on the server shows what is happening.
		if new.loading {
			card.body.set_visible(false);
			let c = Rc::downgrade(card);
			glib::timeout_add_local_once(Duration::from_millis(150), move || {
				if let Some(card) = c.upgrade().filter(|card| card.view.borrow().loading) {
					card.loading.set_visible(true);
				}
			});
		} else {
			card.loading.set_visible(false);
			card.body.set_visible(!new.blocks.is_empty());
		}
		if without_paths(&before.blocks) != without_paths(&new.blocks) {
			self.draw_body(card);
		} else {
			self.fill_pictures(card);
		}
	}

	fn current_card(&self) -> Option<Rc<Card>> {
		self.cards.borrow().iter().find(|c| c.view.borrow().current).cloned()
	}

	/// For the screenshot scenes: selects `text` in the open mail, as dragging over it does.
	pub fn select_in_current(&self, text: &str) -> bool {
		let Some(card) = self.current_card() else { return false };
		let buffer = card.body.buffer();
		let Some((start, end)) = buffer.start_iter().forward_search(text, gtk::TextSearchFlags::empty(), None) else { return false };
		buffer.select_range(&start, &end);
		true
	}

	fn scroll_to(&self, card: &Rc<Card>) {
		let (scroll, root) = (self.scroll.clone(), card.root.clone());
		// After layout, so the card has its place.
		glib::idle_add_local_once(move || {
			let Some(child) = scroll.child() else { return };
			if let Some(p) = root.compute_point(&child, &gtk::graphene::Point::new(0.0, 0.0)) {
				scroll.vadjustment().set_value((p.y() as f64 - 12.0).max(0.0));
			}
		});
	}

	fn draw_body(self: &Rc<Self>, card: &Rc<Card>) {
		let blocks = card.view.borrow().blocks.clone();
		if blocks.is_empty() {
			return;
		}
		card.generation.set(card.generation.get() + 1);
		let rendered = render::render(&card.body, &blocks);
		settle(&card.body);
		*card.links.borrow_mut() = rendered.links;
		*card.pictures.borrow_mut() = rendered.images;
		card.natural.borrow_mut().clear();
		card.shown_paths.borrow_mut().clear();
		self.fill_pictures(card);
	}

	/// Puts in the pictures whose files have arrived.
	fn fill_pictures(self: &Rc<Self>, card: &Rc<Card>) {
		let blocks = card.view.borrow().blocks.clone();
		let generation = card.generation.get();
		let scale = card.body.scale_factor();
		for (picture, index) in card.pictures.borrow().iter() {
			let Some(BlockKind::Image { path: Some(path), .. }) = blocks.get(*index as usize).map(|b| &b.kind) else { continue };
			if card.shown_paths.borrow().iter().any(|(i, _)| i == index) {
				continue;
			}
			card.shown_paths.borrow_mut().push((*index, path.clone()));
			let (picture, path) = (picture.clone(), path.clone());
			let (weak, c) = (Rc::downgrade(self), Rc::downgrade(card));
			runtime::spawn_local(async move {
				let pixels = runtime::run(async move { std::fs::read(path).ok().and_then(|bytes| render::decode(&bytes)) }).await;
				let (Some(this), Some(card)) = (weak.upgrade(), c.upgrade()) else { return };
				match pixels.and_then(|px| render::show_pixels(&picture, px, scale)) {
					Some(natural) if card.generation.get() == generation => {
						render::fit(&picture, natural, this.width.get() - CARD_INSET);
						card.natural.borrow_mut().push((picture, natural));
					},
					// A broken inline image shows nothing.
					_ => picture.set_visible(false),
				}
			});
		}
	}

	/// Opens a composer answering the current mail.
	pub async fn answer(self: &Rc<Self>, answer: Answer) -> Option<Rc<composer::Composer>> {
		let card = self.current_card()?;
		self.answer_card(&card, answer).await
	}

	async fn answer_card(self: &Rc<Self>, card: &Rc<Card>, answer: Answer) -> Option<Rc<composer::Composer>> {
		let id = mail_id(&card.view.borrow().id);
		if card.view.borrow().draft {
			return None;
		}
		let (me, fetcher) = (self.ctx.backend()?.account(), self.ctx.fetcher()?);
		// Lines selected in the mail are what a reply answers.
		let picked = card.body.buffer().selection_bounds().map(|(start, end)| card.body.buffer().text(&start, &end, false).to_string());
		let date = card.view.borrow().date.clone();
		let (mail, doc) = self.opened_mail(fetcher, id).await?;
		let fields = tern_compose::answer_fields(&mail, &doc.to_plain_text(), &date, answer, &me, picked.as_deref());
		let forwarded = (answer == Answer::Forward).then(|| (mail.summary.id.clone(), mail.attachments.clone()));
		Some(composer::Composer::open(&self.ctx, fields, forwarded))
	}

	/// Opens the current mail in a composer when it is a draft.
	pub async fn edit_current_draft(self: &Rc<Self>) -> Option<Rc<composer::Composer>> {
		let id = self.current().filter(|c| c.draft)?.id;
		self.edit_draft(&id).await
	}

	async fn edit_draft(self: &Rc<Self>, id: &MailRef) -> Option<Rc<composer::Composer>> {
		let id = mail_id(id);
		let fetcher = self.ctx.fetcher()?;
		// The quotes as the composer kept them, when it was this computer's.
		let kept = self.ctx.cache().and_then(|c| c.draft_quote(&id).ok().flatten()).and_then(|json| serde_json::from_str(&json).ok());
		let (mail, doc) = self.opened_mail(fetcher, id).await?;
		let fields = tern_compose::draft_fields(&mail, &doc.to_plain_text(), kept);
		Some(composer::Composer::open(&self.ctx, fields, None))
	}

	/// A mail in full, for the composer until it moves to the core.
	async fn opened_mail(&self, fetcher: tern_sync::fetcher::Fetcher, id: MailId) -> Option<(tern_mail::Mail, Doc)> {
		let result = runtime::run(async move {
			let mail = fetcher.mail(&id).await?;
			let doc = Doc::parse(&mail.body_html);
			Ok::<_, tern_mail::Error>((mail, doc))
		})
		.await;
		match result {
			Ok(opened) => Some(opened),
			Err(e) => {
				self.ctx.error(&e);
				None
			},
		}
	}

	pub fn web_url(&self) -> Option<String> {
		self.current().and_then(|c| c.open_url)
	}

	/// Moves keyboard focus into the conversation, onto the current mail.
	pub fn focus(&self) -> bool {
		self.current_card().is_some_and(|c| c.header.grab_focus())
	}
}

fn mail_id(id: &MailRef) -> MailId {
	MailId { list: id.list.clone(), element: id.element.clone() }
}

/// A text view outside a scrolled window keeps the height of its first layout, done before it
/// knows its width, until asked again: ask once it has laid out the text at its real width.
fn settle(body: &gtk::TextView) {
	let body = body.clone();
	glib::idle_add_local_once(move || body.queue_resize());
}

/// Clicking a link opens it; hovering shows where it goes.
fn connect_links(card: &Rc<Card>) {
	let click = gtk::GestureClick::new();
	let c = Rc::downgrade(card);
	click.connect_released(move |gesture, n, x, y| {
		let Some(card) = c.upgrade() else { return };
		// A drag that selected text is not a click.
		if n != 1 || card.body.buffer().has_selection() {
			return;
		}
		if let Some(url) = link_at(&card, x, y) {
			gesture.set_state(gtk::EventSequenceState::Claimed);
			render::open_uri(&card.body, &url);
		}
	});
	card.body.add_controller(click);

	let motion = gtk::EventControllerMotion::new();
	let c = Rc::downgrade(card);
	motion.connect_motion(move |_, x, y| {
		let Some(card) = c.upgrade() else { return };
		let link = link_at(&card, x, y);
		card.body.set_cursor_from_name(Some(if link.is_some() { "pointer" } else { "text" }));
		card.body.set_tooltip_text(link.as_deref());
	});
	card.body.add_controller(motion);
}

fn link_at(card: &Card, x: f64, y: f64) -> Option<String> {
	let (bx, by) = card.body.window_to_buffer_coords(gtk::TextWindowType::Widget, x as i32, y as i32);
	let iter = card.body.iter_at_location(bx, by)?;
	let links = card.links.borrow();
	iter.tags().iter().find_map(|t| links.iter().find(|(tag, _)| tag == t).map(|(_, url)| url.clone()))
}

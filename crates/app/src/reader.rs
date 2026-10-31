//! The reading pane: a conversation, one card per mail, oldest first. The mail chosen in the list
//! and unread ones are open; the rest show one line each and open on click, Enter or Space.

use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::time::Duration;

use adw::prelude::*;
use gtk::glib;
use tern_cache::policy::Want;
use tern_cache::ThreadMail;
use tern_core::{Address, FolderId, FolderKind, Mail, MailId, MailSummary};

use crate::activity::{Busy, Word};
use crate::compose_model::{self, Answer};
use crate::html_doc::{Doc, ImageSrc};
use crate::{attachments, composer, format, render, runtime, window::Ctx};

/// A mail with its parsed body.
pub struct Opened {
	pub mail: Mail,
	pub doc: Doc,
}

type MailHook = Box<dyn Fn(&MailSummary)>;

/// How much narrower a card's body is than the conversation column: its margins and border.
const CARD_INSET: i32 = 2 * 12 + 2;

/// One mail of the conversation.
struct Card {
	summary: MailSummary,
	root: gtk::Box,
	header: gtk::Button,
	snippet: gtk::Label,
	revealer: gtk::Revealer,
	recipients: gtk::Label,
	attachments: gtk::FlowBox,
	body: gtk::TextView,
	loading: adw::Spinner,
	opened: RefCell<Option<Rc<Opened>>>,
	links: RefCell<Vec<(gtk::TextTag, String)>>,
	/// Inline images with their natural size, refitted when the pane changes width.
	pictures: RefCell<Vec<(gtk::Picture, (f64, f64))>>,
	/// A load or draw in flight belongs to this generation of the card.
	generation: Cell<u64>,
	/// Waiting for the mail, from the cache or the server.
	opening: Cell<bool>,
	/// Says "Opening…" while the wait is long enough to notice.
	busy: RefCell<Option<Busy>>,
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
	/// The mail actions (reply, mark unread…) act on: the one last opened or clicked.
	pub current: RefCell<Option<Rc<Opened>>>,
	/// The conversation on show, and the folder it was opened from.
	thread: RefCell<Option<(String, FolderId)>>,
	on_read: RefCell<Option<MailHook>>,
	on_discard: RefCell<Option<MailHook>>,
	on_current: RefCell<Option<Box<dyn Fn()>>>,
}

/// A pill button with an icon and a word, for the actions under a mail.
fn labeled(icon: &str, label: &str) -> gtk::Button {
	let content = adw::ButtonContent::builder().icon_name(icon).label(label).use_underline(true).build();
	gtk::Button::builder().child(&content).css_classes(["pill", "small-pill"]).build()
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

		let reader = Rc::new(Reader {
			ctx,
			page,
			stack,
			banner,
			subject,
			cards_box,
			scroll,
			cards: RefCell::default(),
			width: Cell::new(600),
			current: RefCell::default(),
			thread: RefCell::default(),
			on_read: RefCell::default(),
			on_discard: RefCell::default(),
			on_current: RefCell::default(),
		});
		let weak = Rc::downgrade(&reader);
		probe.connect_resize(move |_, w, _| {
			let Some(this) = weak.upgrade() else { return };
			if w > 0 && w != this.width.get() {
				this.width.set(w);
				for card in this.cards.borrow().iter() {
					settle(&card.body);
					for (picture, natural) in card.pictures.borrow().iter() {
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
					if card.opened.borrow().is_some() {
						r.draw_body(&card);
					}
				}
			}
		};
		let r2 = redraw.clone();
		sm.connect_dark_notify(move |_| r2());
		sm.connect_accent_color_notify(move |_| redraw());
		reader
	}

	/// Called with each unread mail the reader opens, so it can be marked read.
	pub fn connect_read(&self, f: impl Fn(&MailSummary) + 'static) {
		*self.on_read.borrow_mut() = Some(Box::new(f));
	}

	/// Called when the mail actions apply to changes, or finishes loading.
	pub fn connect_current(&self, f: impl Fn() + 'static) {
		*self.on_current.borrow_mut() = Some(Box::new(f));
	}

	/// Called when a draft is discarded from its card.
	pub fn connect_discard(&self, f: impl Fn(&MailSummary) + 'static) {
		*self.on_discard.borrow_mut() = Some(Box::new(f));
	}

	pub fn clear(&self) {
		*self.current.borrow_mut() = None;
		*self.thread.borrow_mut() = None;
		for card in self.cards.borrow_mut().drain(..) {
			// Loads still in flight for it find it gone.
			card.generation.set(card.generation.get() + 1);
		}
		while let Some(child) = self.cards_box.first_child() {
			self.cards_box.remove(&child);
		}
		self.banner.set_revealed(false);
		self.stack.set_visible_child_name("empty");
		self.page.set_title("Conversation");
	}

	/// The conversation on show, as (thread key, folder).
	pub fn thread(&self) -> Option<(String, FolderId)> {
		self.thread.borrow().clone()
	}

	/// The mails of the conversation on show.
	pub fn mails(&self) -> Vec<MailSummary> {
		self.cards.borrow().iter().map(|c| c.summary.clone()).collect()
	}

	/// Shows a conversation with `focus` open and scrolled to. `mails` are oldest first. Showing the
	/// conversation already on show again keeps it as it is unless its mails changed.
	pub fn show_thread(self: &Rc<Self>, key: &str, folder: &FolderId, mails: Vec<ThreadMail>, focus: &MailId) {
		let same = self.thread.borrow().as_ref().is_some_and(|(k, f)| k == key && f == folder);
		if same {
			let shown = self.mails();
			let unchanged = shown.len() == mails.len()
				&& shown.iter().zip(&mails).all(|(a, b)| a.id == b.summary.id && a.draft == b.summary.draft && a.subject == b.summary.subject);
			if unchanged {
				return;
			}
		}
		let open_before: Vec<MailId> = if same { self.cards.borrow().iter().filter(|c| c.revealer.reveals_child()).map(|c| c.summary.id.clone()).collect() } else { vec![] };
		self.clear();
		*self.thread.borrow_mut() = Some((key.to_string(), folder.clone()));
		let latest = mails.last().map(|m| m.summary.subject.clone()).unwrap_or_default();
		let subject = if latest.trim().is_empty() { "(No Subject)".to_string() } else { latest };
		self.page.set_title(&subject);
		self.subject.set_label(&subject);

		// The focused mail first and alone, so it shows soon; the rest of the conversation behind it.
		if let Some(fetcher) = self.ctx.fetcher() {
			fetcher.want(Want::Reading { thread: mails.iter().map(|m| m.summary.id.clone()).collect(), focus: focus.clone() });
		}
		let mut focus_card = None;
		for m in &mails {
			let card = self.card(&m.summary, m.folder);
			self.cards_box.append(&card.root);
			self.cards.borrow_mut().push(card.clone());
			if &m.summary.id == focus || m.summary.unread || open_before.contains(&m.summary.id) {
				self.expand(&card, true);
			}
			if &m.summary.id == focus {
				focus_card = Some(card);
			}
		}
		self.stack.set_visible_child_name("thread");
		let target = focus_card.or_else(|| self.cards.borrow().last().cloned());
		if let Some(card) = target {
			self.set_current(&card);
			if !same {
				self.scroll_to(&card);
			}
		}
	}

	fn card(self: &Rc<Self>, s: &MailSummary, folder: FolderKind) -> Rc<Card> {
		let avatar = adw::Avatar::new(36, Some(s.from.display()), true);
		avatar.set_valign(gtk::Align::Start);
		let from = gtk::Label::builder().xalign(0.0).ellipsize(gtk::pango::EllipsizeMode::End).css_classes(["heading"]).build();
		from.set_label(if s.from.display().is_empty() { "(Unknown Sender)" } else { s.from.display() });
		let address = gtk::Label::builder().xalign(0.0).ellipsize(gtk::pango::EllipsizeMode::End).css_classes(["dim-label"]).label(&s.from.address).build();
		address.set_visible(!s.from.name.trim().is_empty());
		let date = gtk::Label::builder()
			.xalign(1.0)
			.valign(gtk::Align::Start)
			.ellipsize(gtk::pango::EllipsizeMode::End)
			.css_classes(["caption", "dim-label", "numeric"])
			.label(format::long_date(s.received_ms, &glib::TimeZone::local()))
			.build();
		let names = gtk::Box::new(gtk::Orientation::Horizontal, 6);
		names.append(&from);
		names.append(&address);
		if s.draft {
			names.append(&gtk::Label::builder().label("Draft").css_classes(["caption-heading", "error"]).build());
		} else if folder == FolderKind::Sent {
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
		header.update_property(&[gtk::accessible::Property::Label(&format!("Message from {}", s.from.display()))]);

		let recipients = gtk::Label::builder().xalign(0.0).wrap(true).wrap_mode(gtk::pango::WrapMode::WordChar).selectable(true).css_classes(["caption", "dim-label"]).build();
		let attachments_box =
			gtk::FlowBox::builder().selection_mode(gtk::SelectionMode::None).column_spacing(8).row_spacing(8).max_children_per_line(4).homogeneous(false).visible(false).build();
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
			summary: s.clone(),
			root,
			header,
			snippet,
			revealer,
			recipients,
			attachments: attachments_box,
			body,
			loading,
			opened: RefCell::default(),
			links: RefCell::default(),
			pictures: RefCell::default(),
			generation: Cell::new(0),
			opening: Cell::new(false),
			busy: RefCell::default(),
		});
		self.update_snippet(&card);

		if s.draft {
			let edit = labeled("document-edit-symbolic", "Continue _Editing");
			edit.add_css_class("suggested-action");
			let discard = labeled("user-trash-symbolic", "_Discard");
			let (weak, c) = (Rc::downgrade(self), Rc::downgrade(&card));
			edit.connect_clicked(move |_| {
				if let (Some(this), Some(card)) = (weak.upgrade(), c.upgrade()) {
					this.edit_draft(&card);
				}
			});
			let (weak, c) = (Rc::downgrade(self), Rc::downgrade(&card));
			discard.connect_clicked(move |_| {
				if let (Some(this), Some(card)) = (weak.upgrade(), c.upgrade()) {
					if let Some(f) = this.on_discard.borrow().as_ref() {
						f(&card.summary);
					}
				}
			});
			actions.append(&edit);
			actions.append(&discard);
		} else {
			for (icon, label, answer) in [
				("tern-reply-symbolic", "_Reply", Answer::Reply),
				("tern-reply-all-symbolic", "Reply _All", Answer::ReplyAll),
				("tern-forward-symbolic", "_Forward", Answer::Forward),
			] {
				let button = labeled(icon, label);
				let (weak, c) = (Rc::downgrade(self), Rc::downgrade(&card));
				button.connect_clicked(move |_| {
					if let (Some(this), Some(card)) = (weak.upgrade(), c.upgrade()) {
						this.set_current(&card);
						this.answer(answer);
					}
				});
				actions.append(&button);
			}
		}

		let (weak, c) = (Rc::downgrade(self), Rc::downgrade(&card));
		card.header.connect_clicked(move |_| {
			let (Some(this), Some(card)) = (weak.upgrade(), c.upgrade()) else { return };
			let open = !card.revealer.reveals_child();
			this.expand(&card, open);
			if open {
				this.set_current(&card);
			}
		});
		connect_links(&card);
		card
	}

	fn update_snippet(&self, card: &Card) {
		let text = match card.opened.borrow().as_ref() {
			Some(o) => o.doc.to_plain_text().split_whitespace().take(40).collect::<Vec<_>>().join(" "),
			None => card.summary.to.as_ref().map(|to| format!("To {}", to.display())).unwrap_or_default(),
		};
		card.snippet.set_label(&text);
	}

	fn expand(self: &Rc<Self>, card: &Rc<Card>, open: bool) {
		card.revealer.set_reveal_child(open);
		card.snippet.set_visible(!open);
		if open && card.opened.borrow().is_none() {
			self.load(card);
		}
	}

	/// Makes `card` the one actions apply to.
	fn set_current(&self, card: &Rc<Card>) {
		let opened = card.opened.borrow().clone();
		self.banner.set_revealed(opened.as_ref().is_some_and(|o| o.doc.is_complex()));
		*self.current.borrow_mut() = opened;
		for c in self.cards.borrow().iter() {
			if Rc::ptr_eq(c, card) {
				c.root.add_css_class("current");
			} else {
				c.root.remove_css_class("current");
			}
		}
		if let Some(f) = self.on_current.borrow().as_ref() {
			f();
		}
	}

	fn current_card(&self) -> Option<Rc<Card>> {
		self.cards.borrow().iter().find(|c| c.root.has_css_class("current")).cloned()
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

	/// Opens a card's mail from the cache, or waits for the fetcher to bring it.
	fn load(self: &Rc<Self>, card: &Rc<Card>) {
		let Some(fetcher) = self.ctx.fetcher() else { return };
		let generation = card.generation.get() + 1;
		card.generation.set(generation);
		card.body.set_visible(false);
		card.opening.set(true);
		// Mail from the cache opens at once; only a wait on the server shows what is happening.
		let (c, activity) = (Rc::downgrade(card), self.ctx.activity.clone());
		glib::timeout_add_local_once(Duration::from_millis(150), move || {
			if let Some(card) = c.upgrade().filter(|card| card.generation.get() == generation && card.opening.get()) {
				card.loading.set_visible(true);
				*card.busy.borrow_mut() = Some(activity.start(Word::Opening));
			}
		});
		let id = card.summary.id.clone();
		let (weak, c) = (Rc::downgrade(self), Rc::downgrade(card));
		runtime::spawn_local(async move {
			let result = runtime::run(async move {
				let mail = fetcher.mail(&id).await?;
				let doc = Doc::parse(&mail.body_html);
				Ok::<_, tern_core::Error>(Opened { mail, doc })
			})
			.await;
			let (Some(this), Some(card)) = (weak.upgrade(), c.upgrade()) else { return };
			if card.generation.get() != generation {
				return;
			}
			card.opening.set(false);
			card.busy.borrow_mut().take();
			card.loading.set_visible(false);
			match result {
				Ok(opened) => {
					this.fill(&card, Rc::new(opened));
					if card.summary.unread {
						if let Some(f) = this.on_read.borrow().as_ref() {
							f(&card.summary);
						}
					}
				},
				Err(e) => {
					card.revealer.set_reveal_child(false);
					card.snippet.set_label(&e.to_string());
					card.snippet.set_visible(true);
				},
			}
		});
	}

	fn fill(self: &Rc<Self>, card: &Rc<Card>, opened: Rc<Opened>) {
		*card.opened.borrow_mut() = Some(opened.clone());
		let mail = &opened.mail;
		card.recipients.set_label(&recipients_line(mail));
		card.recipients.set_visible(!mail.to.is_empty() || !mail.cc.is_empty() || !mail.bcc.is_empty());
		while let Some(child) = card.attachments.first_child() {
			card.attachments.remove(&child);
		}
		let inline = opened.doc.cids();
		let shown: Vec<_> = mail.attachments.iter().filter(|a| a.cid.as_ref().is_none_or(|c| !inline.contains(c))).collect();
		for att in &shown {
			card.attachments.append(&attachments::chip(&self.ctx, &mail.summary.id, att));
		}
		card.attachments.set_visible(!shown.is_empty());
		card.body.set_visible(true);
		self.update_snippet(card);
		self.draw_body(card);
		if card.root.has_css_class("current") {
			self.set_current(card);
		}
	}

	fn draw_body(self: &Rc<Self>, card: &Rc<Card>) {
		let Some(opened) = card.opened.borrow().clone() else { return };
		let rendered = render::render(&card.body, &opened.doc);
		settle(&card.body);
		*card.links.borrow_mut() = rendered.links;
		let scale = card.body.scale_factor();
		card.pictures.borrow_mut().clear();
		let generation = card.generation.get();
		for (picture, src) in rendered.images {
			let ctx = self.ctx.clone();
			let (weak, c) = (Rc::downgrade(self), Rc::downgrade(card));
			let mail = opened.mail.clone();
			runtime::spawn_local(async move {
				let bytes = match src {
					ImageSrc::Data(d) => Some(d),
					ImageSrc::Cid(cid) => {
						let att = mail.attachments.iter().find(|a| a.cid.as_deref().map(|c| c.trim_matches(['<', '>'])) == Some(cid.as_str())).cloned();
						match (att, ctx.backend()) {
							(Some(att), Some(backend)) => {
								let id = mail.summary.id.clone();
								let _busy = ctx.activity.start(Word::Opening);
								runtime::run(async move { backend.attachment(&id, &att).await.ok() }).await
							},
							_ => None,
						}
					},
				};
				let pixels = match bytes {
					Some(b) => runtime::run(async move { render::decode(&b) }).await,
					None => None,
				};
				let (Some(this), Some(card)) = (weak.upgrade(), c.upgrade()) else { return };
				match pixels.and_then(|px| render::show_pixels(&picture, px, scale)) {
					Some(natural) if card.generation.get() == generation => {
						render::fit(&picture, natural, this.width.get() - CARD_INSET);
						card.pictures.borrow_mut().push((picture, natural));
					},
					// A missing or broken inline image shows nothing.
					_ => picture.set_visible(false),
				}
			});
		}
	}

	/// Opens a composer answering the current mail.
	pub fn answer(&self, answer: Answer) -> Option<Rc<composer::Composer>> {
		let opened = self.current.borrow().clone()?;
		let backend = self.ctx.backend()?;
		let me = backend.account().address;
		let mail = &opened.mail;
		if mail.summary.draft {
			return None;
		}
		let date = format::long_date(mail.summary.received_ms, &glib::TimeZone::local());
		let fields = compose_model::answer_fields(mail, &opened.doc.to_plain_text(), &date, answer, &me);
		let forwarded = (answer == Answer::Forward).then(|| (mail.summary.id.clone(), mail.attachments.clone()));
		Some(composer::Composer::open(&self.ctx, fields, forwarded))
	}

	/// Opens the current mail in a composer when it is a loaded draft.
	pub fn edit_current_draft(self: &Rc<Self>) -> Option<Rc<composer::Composer>> {
		self.current_card().filter(|c| c.summary.draft).and_then(|card| self.edit_draft(&card))
	}

	fn edit_draft(self: &Rc<Self>, card: &Rc<Card>) -> Option<Rc<composer::Composer>> {
		let Some(opened) = card.opened.borrow().clone() else {
			// Not loaded yet: open the card first; editing waits for it.
			self.expand(card, true);
			return None;
		};
		let fields = compose_model::draft_fields(&opened.mail, &opened.doc.to_plain_text());
		Some(composer::Composer::open(&self.ctx, fields, None))
	}

	pub fn web_url(&self) -> Option<String> {
		self.current.borrow().as_ref().map(|o| o.mail.web_url.clone())
	}

	/// Moves keyboard focus into the conversation, onto the current mail.
	pub fn focus(&self) -> bool {
		self.current_card().is_some_and(|c| c.header.grab_focus())
	}
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

fn names(list: &[Address]) -> String {
	list.iter().map(|a| a.display().to_string()).collect::<Vec<_>>().join(", ")
}

fn recipients_line(mail: &Mail) -> String {
	let mut s = format!("To {}", names(&mail.to));
	if !mail.cc.is_empty() {
		s.push_str(&format!(" · Cc {}", names(&mail.cc)));
	}
	if !mail.bcc.is_empty() {
		s.push_str(&format!(" · Bcc {}", names(&mail.bcc)));
	}
	s
}

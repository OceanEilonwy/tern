//! The reading pane: one mail's header, attachments and body.

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use adw::prelude::*;
use gtk::glib;
use tern_core::{Address, Mail};

use crate::compose_model::{self, Answer};
use crate::html_doc::{Doc, ImageSrc};
use crate::{attachments, composer, format, render, runtime, window::Ctx};

/// The mail on show, with its parsed body.
pub struct Opened {
	pub mail: Mail,
	pub doc: Doc,
}

pub struct Reader {
	ctx: Ctx,
	pub page: adw::NavigationPage,
	stack: gtk::Stack,
	banner: adw::Banner,
	subject: gtk::Label,
	avatar: adw::Avatar,
	from_name: gtk::Label,
	from_address: gtk::Label,
	recipients: gtk::Label,
	date: gtk::Label,
	attachments: gtk::FlowBox,
	body: gtk::TextView,
	scroll: gtk::ScrolledWindow,
	links: RefCell<Vec<(gtk::TextTag, String)>>,
	/// Inline images with their natural size, refitted when the pane changes width.
	pictures: RefCell<Vec<(gtk::Picture, (f64, f64))>>,
	width: Cell<i32>,
	pub current: RefCell<Option<Rc<Opened>>>,
	generation: Cell<u64>,
}

impl Reader {
	pub fn new(ctx: Ctx) -> Rc<Reader> {
		let header = adw::HeaderBar::builder().show_title(false).build();
		for (icon, action, tip) in [
			("mail-reply-sender-symbolic", "win.reply", "Reply"),
			("mail-reply-all-symbolic", "win.reply-all", "Reply All"),
			("mail-forward-symbolic", "win.forward", "Forward"),
		] {
			header.pack_start(&gtk::Button::builder().icon_name(icon).action_name(action).tooltip_text(tip).build());
		}
		let menu = gtk::gio::Menu::new();
		menu.append(Some("Mark as _Unread"), Some("win.mark-unread"));
		menu.append(Some("_Open in Tuta"), Some("win.open-in-tuta"));
		header.pack_end(&gtk::MenuButton::builder().icon_name("view-more-symbolic").menu_model(&menu).tooltip_text("More").build());
		header.pack_end(&gtk::Button::builder().icon_name("user-trash-symbolic").action_name("win.trash").tooltip_text("Move to Trash").build());

		let banner = adw::Banner::builder().title("This message has formatting Tern can’t show").button_label("Open in Tuta").build();
		banner.set_action_name(Some("win.open-in-tuta"));

		let subject = gtk::Label::builder().xalign(0.0).wrap(true).wrap_mode(gtk::pango::WrapMode::WordChar).selectable(true).css_classes(["title-2"]).build();
		let avatar = adw::Avatar::new(40, None, true);
		avatar.set_valign(gtk::Align::Start);
		let from_name = gtk::Label::builder().xalign(0.0).ellipsize(gtk::pango::EllipsizeMode::End).selectable(true).css_classes(["heading"]).build();
		let from_address = gtk::Label::builder().xalign(0.0).ellipsize(gtk::pango::EllipsizeMode::End).selectable(true).css_classes(["dim-label"]).build();
		let recipients =
			gtk::Label::builder().xalign(0.0).wrap(true).wrap_mode(gtk::pango::WrapMode::WordChar).selectable(true).css_classes(["caption", "dim-label"]).build();
		let date = gtk::Label::builder().xalign(1.0).valign(gtk::Align::Start).css_classes(["caption", "dim-label", "numeric"]).build();
		let names = gtk::Box::new(gtk::Orientation::Horizontal, 6);
		names.append(&from_name);
		names.append(&from_address);
		let who = gtk::Box::builder().orientation(gtk::Orientation::Vertical).spacing(2).hexpand(true).valign(gtk::Align::Center).build();
		who.append(&names);
		who.append(&recipients);
		let from_row = gtk::Box::new(gtk::Orientation::Horizontal, 12);
		from_row.append(&avatar);
		from_row.append(&who);
		from_row.append(&date);

		let attachments_box = gtk::FlowBox::builder()
			.selection_mode(gtk::SelectionMode::None)
			.column_spacing(8)
			.row_spacing(8)
			.max_children_per_line(4)
			.homogeneous(false)
			.visible(false)
			.build();

		let body = gtk::TextView::builder()
			.editable(false)
			.cursor_visible(false)
			.wrap_mode(gtk::WrapMode::WordChar)
			.pixels_below_lines(2)
			.css_classes(["reader-body"])
			.build();
		body.set_accessible_role(gtk::AccessibleRole::Document);

		let column = gtk::Box::builder().orientation(gtk::Orientation::Vertical).spacing(18).margin_top(24).margin_bottom(32).margin_start(24).margin_end(24).build();
		column.append(&subject);
		column.append(&from_row);
		column.append(&attachments_box);
		column.append(&gtk::Separator::new(gtk::Orientation::Horizontal));
		column.append(&body);
		// Zero-height, full-width: tells inline images how wide the text is.
		let probe = gtk::DrawingArea::builder().hexpand(true).build();
		column.append(&probe);
		let clamp = adw::Clamp::builder().maximum_size(840).tightening_threshold(600).child(&column).build();
		let scroll = gtk::ScrolledWindow::builder().hscrollbar_policy(gtk::PolicyType::Never).child(&clamp).vexpand(true).css_classes(["view"]).build();

		let empty = adw::StatusPage::builder().icon_name("mail-read-symbolic").title("No Message Selected").css_classes(["compact"]).build();
		let loading = adw::Spinner::builder().width_request(32).height_request(32).halign(gtk::Align::Center).valign(gtk::Align::Center).build();
		let stack = gtk::Stack::builder().transition_type(gtk::StackTransitionType::Crossfade).transition_duration(120).build();
		stack.add_named(&empty, Some("empty"));
		stack.add_named(&loading, Some("loading"));
		stack.add_named(&scroll, Some("mail"));

		let toolbar = adw::ToolbarView::new();
		toolbar.add_top_bar(&header);
		toolbar.add_top_bar(&banner);
		toolbar.set_content(Some(&stack));
		let page = adw::NavigationPage::builder().title("Message").tag("reader").child(&toolbar).build();

		let reader = Rc::new(Reader {
			ctx,
			page,
			stack,
			banner,
			subject,
			avatar,
			from_name,
			from_address,
			recipients,
			date,
			attachments: attachments_box,
			body,
			scroll,
			links: RefCell::default(),
			pictures: RefCell::default(),
			width: Cell::new(600),
			current: RefCell::default(),
			generation: Cell::new(0),
		});
		reader.connect_links();
		let weak = Rc::downgrade(&reader);
		probe.connect_resize(move |_, w, _| {
			let Some(this) = weak.upgrade() else { return };
			if w > 0 && w != this.width.get() {
				this.width.set(w);
				for (picture, natural) in this.pictures.borrow().iter() {
					render::fit(picture, *natural, w);
				}
			}
		});
		let weak = Rc::downgrade(&reader);
		let sm = adw::StyleManager::default();
		let redraw = move || {
			if let Some(r) = weak.upgrade() {
				r.redraw();
			}
		};
		let r2 = redraw.clone();
		sm.connect_dark_notify(move |_| r2());
		sm.connect_accent_color_notify(move |_| redraw());
		reader
	}

	/// Clicking a link opens it; hovering shows where it goes.
	fn connect_links(self: &Rc<Self>) {
		let click = gtk::GestureClick::new();
		let weak = Rc::downgrade(self);
		click.connect_released(move |gesture, n, x, y| {
			let Some(this) = weak.upgrade() else { return };
			// A drag that selected text is not a click.
			if n != 1 || this.body.buffer().has_selection() {
				return;
			}
			if let Some(url) = this.link_at(x, y) {
				gesture.set_state(gtk::EventSequenceState::Claimed);
				render::open_uri(&this.body, &url);
			}
		});
		self.body.add_controller(click);

		let motion = gtk::EventControllerMotion::new();
		let weak = Rc::downgrade(self);
		motion.connect_motion(move |_, x, y| {
			let Some(this) = weak.upgrade() else { return };
			let link = this.link_at(x, y);
			this.body.set_cursor_from_name(Some(if link.is_some() { "pointer" } else { "text" }));
			this.body.set_tooltip_text(link.as_deref());
		});
		self.body.add_controller(motion);
	}

	fn link_at(&self, x: f64, y: f64) -> Option<String> {
		let (bx, by) = self.body.window_to_buffer_coords(gtk::TextWindowType::Widget, x as i32, y as i32);
		let iter = self.body.iter_at_location(bx, by)?;
		let links = self.links.borrow();
		iter.tags().iter().find_map(|t| links.iter().find(|(tag, _)| tag == t).map(|(_, url)| url.clone()))
	}

	pub fn clear(&self) {
		self.generation.set(self.generation.get() + 1);
		*self.current.borrow_mut() = None;
		self.banner.set_revealed(false);
		self.stack.set_visible_child_name("empty");
		self.page.set_title("Message");
		self.body.set_buffer(Some(&gtk::TextBuffer::new(None)));
	}

	pub fn loading(&self) -> u64 {
		let g = self.generation.get() + 1;
		self.generation.set(g);
		self.stack.set_visible_child_name("loading");
		g
	}

	pub fn is_current(&self, generation: u64) -> bool {
		self.generation.get() == generation
	}

	pub fn show(self: &Rc<Self>, opened: Opened) {
		let opened = Rc::new(opened);
		*self.current.borrow_mut() = Some(opened.clone());
		let mail = &opened.mail;
		let s = &mail.summary;
		let subject = if s.subject.trim().is_empty() { "(No Subject)" } else { s.subject.as_str() };
		self.page.set_title(subject);
		self.subject.set_label(subject);
		self.avatar.set_text(Some(s.from.display()));
		self.from_name.set_label(s.from.display());
		self.from_address.set_label(&s.from.address);
		self.from_address.set_visible(!s.from.name.trim().is_empty());
		self.recipients.set_label(&recipients_line(mail));
		self.recipients.set_visible(!mail.to.is_empty() || !mail.cc.is_empty());
		self.date.set_label(&format::long_date(s.received_ms, &glib::TimeZone::local()));
		self.date.set_tooltip_text(None);

		while let Some(child) = self.attachments.first_child() {
			self.attachments.remove(&child);
		}
		let inline = opened.doc.cids();
		let shown: Vec<_> = mail.attachments.iter().filter(|a| a.cid.as_ref().is_none_or(|c| !inline.contains(c))).collect();
		for att in &shown {
			self.attachments.append(&attachments::chip(&self.ctx, &s.id, att));
		}
		self.attachments.set_visible(!shown.is_empty());
		self.banner.set_revealed(opened.doc.is_complex());
		self.draw_body();
		self.scroll.vadjustment().set_value(0.0);
		self.stack.set_visible_child_name("mail");
	}

	fn redraw(self: &Rc<Self>) {
		if self.current.borrow().is_some() {
			self.draw_body();
		}
	}

	fn draw_body(self: &Rc<Self>) {
		let Some(opened) = self.current.borrow().clone() else { return };
		let rendered = render::render(&self.body, &opened.doc);
		*self.links.borrow_mut() = rendered.links;
		let scale = self.body.scale_factor();
		self.pictures.borrow_mut().clear();
		let generation = self.generation.get();
		for (picture, src) in rendered.images {
			let ctx = self.ctx.clone();
			let this = Rc::downgrade(self);
			let mail = opened.mail.clone();
			runtime::spawn_local(async move {
				let bytes = match src {
					ImageSrc::Data(d) => Some(d),
					ImageSrc::Cid(cid) => {
						let att = mail.attachments.iter().find(|a| a.cid.as_deref().map(|c| c.trim_matches(['<', '>'])) == Some(cid.as_str())).cloned();
						match (att, ctx.backend()) {
							(Some(att), Some(backend)) => {
								let id = mail.summary.id.clone();
								runtime::run(async move { backend.attachment(&id, &att).await.ok() }).await
							}
							_ => None,
						}
					}
				};
				let pixels = match bytes {
					Some(b) => runtime::run(async move { render::decode(&b) }).await,
					None => None,
				};
				match pixels {
					Some(px) => {
						let (Some(this), Some(natural)) = (this.upgrade(), render::show_pixels(&picture, px, scale)) else { return };
						if this.generation.get() == generation {
							render::fit(&picture, natural, this.width.get());
							this.pictures.borrow_mut().push((picture, natural));
						}
					}
					// A missing or broken inline image shows nothing.
					None => picture.set_visible(false),
				}
			});
		}
	}

	/// Opens a composer answering the mail on show.
	pub fn answer(&self, answer: Answer) -> Option<Rc<composer::Composer>> {
		let opened = self.current.borrow().clone()?;
		let backend = self.ctx.backend()?;
		let me = backend.account().address;
		let mail = &opened.mail;
		let date = format::long_date(mail.summary.received_ms, &glib::TimeZone::local());
		let fields = compose_model::answer_fields(mail, &opened.doc.to_plain_text(), &date, answer, &me);
		let forwarded = (answer == Answer::Forward).then(|| (mail.summary.id.clone(), mail.attachments.clone()));
		Some(composer::Composer::open(&self.ctx, fields, forwarded))
	}

	pub fn web_url(&self) -> Option<String> {
		self.current.borrow().as_ref().map(|o| o.mail.web_url.clone())
	}
}

fn names(list: &[Address]) -> String {
	list.iter().map(|a| a.display().to_string()).collect::<Vec<_>>().join(", ")
}

fn recipients_line(mail: &Mail) -> String {
	let mut s = format!("To {}", names(&mail.to));
	if !mail.cc.is_empty() {
		s.push_str(&format!(" · Cc {}", names(&mail.cc)));
	}
	s
}

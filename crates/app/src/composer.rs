//! A window for writing one mail.

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use adw::prelude::*;
use gtk::{gdk, gio, glib};
use tern_core::{mime_for, Attachment, FolderKind, MailId, NewAttachment};

use crate::activity::Word;
use crate::compose_model::{Fields, Quote, MAX_TOTAL_BYTES};
use crate::{attachments::icon_for, format, runtime, suggest, window::Ctx};

pub struct Composer {
	ctx: Ctx,
	pub window: adw::Window,
	title: adw::WindowTitle,
	toasts: adw::ToastOverlay,
	/// The address to send from, among the account's; shown only when it has more than one.
	pub from: gtk::DropDown,
	addresses: Vec<String>,
	to: gtk::Entry,
	cc: gtk::Entry,
	bcc: gtk::Entry,
	cc_rows: [gtk::Widget; 4],
	subject: gtk::Entry,
	body: gtk::TextView,
	/// The earlier mail this carries, kept out of the text.
	quote: RefCell<Quote>,
	/// The lines picked in the reader, above the text, with an × to leave them out.
	picked: gtk::Box,
	/// The whole earlier mail, under the text: ••• to look at it, and a button to add it or an × to
	/// leave it out.
	pill: gtk::Box,
	whole: gtk::Revealer,
	send: gtk::Button,
	files: gtk::ListBox,
	files_box: gtk::Box,
	files_summary: gtk::Label,
	attachments: RefCell<Vec<NewAttachment>>,
	/// Attachments already on the saved draft this continues.
	kept: RefCell<Vec<Attachment>>,
	previous: Option<(MailId, tern_core::ReplyKind)>,
	/// The saved draft this continues, replaced on every save.
	existing: RefCell<Option<MailId>>,
	/// The fields as last saved or opened, to tell whether closing needs to save.
	saved: RefCell<Snapshot>,
	pending: Cell<usize>,
	sending: Cell<bool>,
	done: Cell<bool>,
}

/// A label and a frameless entry, as one line of the header.
fn field(label: &str) -> (gtk::Box, gtk::Entry) {
	let name = gtk::Label::builder().label(label).xalign(1.0).width_chars(7).css_classes(["dim-label"]).build();
	let entry = gtk::Entry::builder().has_frame(false).hexpand(true).build();
	entry.update_property(&[gtk::accessible::Property::Label(label)]);
	let row = gtk::Box::builder().spacing(12).margin_start(12).margin_end(12).margin_top(4).margin_bottom(4).build();
	row.append(&name);
	row.append(&entry);
	(row, entry)
}

impl Composer {
	/// Opens a composer. `forward` names attachments of an existing mail to download and attach.
	pub fn open(ctx: &Ctx, fields: Fields, forward: Option<(MailId, Vec<Attachment>)>) -> Rc<Composer> {
		let title = adw::WindowTitle::new("New Message", "");
		let send = gtk::Button::builder().label("Send").css_classes(["suggested-action"]).tooltip_text("Send (Ctrl+Enter)").build();
		let attach = gtk::Button::builder().icon_name("mail-attachment-symbolic").tooltip_text("Attach Files… (Ctrl+Shift+A)").build();
		let discard = gtk::Button::builder().icon_name("user-trash-symbolic").tooltip_text("Discard").build();
		let header = adw::HeaderBar::builder().title_widget(&title).build();
		header.pack_start(&attach);
		header.pack_end(&send);
		header.pack_end(&discard);
		header.pack_end(&ctx.activity.indicator());

		// The account's addresses, and which one this mail goes out from.
		let addresses = ctx.backend().map(|b| b.account().addresses()).unwrap_or_default();
		let from = gtk::DropDown::from_strings(&addresses.iter().map(String::as_str).collect::<Vec<_>>());
		from.update_property(&[gtk::accessible::Property::Label("From")]);
		from.add_css_class("flat");
		if let Some(at) = addresses.iter().position(|a| a.eq_ignore_ascii_case(&fields.from)) {
			from.set_selected(at as u32);
		}
		let from_name = gtk::Label::builder().label("From").xalign(1.0).width_chars(7).css_classes(["dim-label"]).build();
		let from_row = gtk::Box::builder().spacing(12).margin_start(12).margin_end(12).margin_top(4).margin_bottom(4).build();
		from_row.append(&from_name);
		from_row.append(&from);
		let from_sep = gtk::Separator::new(gtk::Orientation::Horizontal);
		from_row.set_visible(addresses.len() > 1);
		from_sep.set_visible(addresses.len() > 1);

		let (to_row, to) = field("To");
		let (cc_row, cc) = field("Cc");
		let (bcc_row, bcc) = field("Bcc");
		let (subject_row, subject) = field("Subject");
		let cc_toggle = gtk::ToggleButton::builder().label("Cc/Bcc").css_classes(["flat", "caption"]).valign(gtk::Align::Center).tooltip_text("Show Cc and Bcc").build();
		to_row.append(&cc_toggle);
		let cc_sep = gtk::Separator::new(gtk::Orientation::Horizontal);
		let bcc_sep = gtk::Separator::new(gtk::Orientation::Horizontal);
		let cc_rows: [gtk::Widget; 4] = [cc_sep.clone().upcast(), cc_row.clone().upcast(), bcc_sep.clone().upcast(), bcc_row.clone().upcast()];

		let fields_box = gtk::Box::new(gtk::Orientation::Vertical, 0);
		for w in [
			from_row.upcast_ref::<gtk::Widget>(),
			from_sep.upcast_ref(),
			to_row.upcast_ref(),
			cc_sep.upcast_ref(),
			cc_row.upcast_ref(),
			bcc_sep.upcast_ref(),
			bcc_row.upcast_ref(),
			gtk::Separator::new(gtk::Orientation::Horizontal).upcast_ref(),
			subject_row.upcast_ref(),
			gtk::Separator::new(gtk::Orientation::Horizontal).upcast_ref(),
		] {
			fields_box.append(w);
		}

		let body = gtk::TextView::builder()
			.wrap_mode(gtk::WrapMode::WordChar)
			.top_margin(18)
			.bottom_margin(18)
			.left_margin(24)
			.right_margin(24)
			.vexpand(true)
			.accepts_tab(false)
			.build();
		body.update_property(&[gtk::accessible::Property::Label("Message")]);
		let picked = gtk::Box::builder().orientation(gtk::Orientation::Vertical).css_classes(["quote-picked"]).margin_top(18).margin_start(24).margin_end(24).visible(false).build();
		let pill = gtk::Box::builder().css_classes(["quote-pill"]).halign(gtk::Align::Start).margin_start(24).margin_bottom(12).visible(false).build();
		let whole = gtk::Revealer::builder().transition_type(gtk::RevealerTransitionType::SlideDown).build();
		let writing = gtk::Box::new(gtk::Orientation::Vertical, 0);
		writing.append(&picked);
		writing.append(&body);
		writing.append(&pill);
		writing.append(&whole);
		let body_scroll = gtk::ScrolledWindow::builder().hscrollbar_policy(gtk::PolicyType::Never).child(&writing).vexpand(true).build();

		let files = gtk::ListBox::builder().selection_mode(gtk::SelectionMode::None).css_classes(["boxed-list"]).build();
		let files_summary = gtk::Label::builder().xalign(0.0).css_classes(["caption", "dim-label"]).build();
		let files_box = gtk::Box::builder().orientation(gtk::Orientation::Vertical).spacing(6).margin_start(12).margin_end(12).margin_top(6).margin_bottom(12).visible(false).build();
		files_box.append(&files_summary);
		files_box.append(&files);
		let files_scroll = gtk::ScrolledWindow::builder()
			.hscrollbar_policy(gtk::PolicyType::Never)
			.propagate_natural_height(true)
			.max_content_height(220)
			.child(&files_box)
			.build();

		let content = gtk::Box::new(gtk::Orientation::Vertical, 0);
		content.add_css_class("view");
		content.append(&fields_box);
		content.append(&body_scroll);
		content.append(&files_scroll);

		let toasts = adw::ToastOverlay::new();
		toasts.set_child(Some(&content));
		let toolbar = adw::ToolbarView::new();
		toolbar.add_top_bar(&header);
		toolbar.set_content(Some(&toasts));

		let window = adw::Window::builder().title("New Message").default_width(720).default_height(640).content(&toolbar).build();
		window.set_size_request(360, 360);
		window.set_transient_for(Some(&ctx.window));
		window.set_application(ctx.window.application().as_ref());

		let this = Rc::new(Composer {
			ctx: ctx.clone(),
			window,
			title,
			toasts,
			from,
			addresses,
			to,
			cc,
			bcc,
			cc_rows,
			subject,
			body,
			quote: RefCell::new(fields.quote.clone()),
			picked,
			pill,
			whole,
			send,
			files,
			files_box,
			files_summary,
			attachments: RefCell::new(fields.attachments.clone()),
			kept: RefCell::new(fields.kept.clone()),
			previous: fields.previous.clone(),
			existing: RefCell::new(fields.existing.clone()),
			saved: RefCell::default(),
			pending: Cell::new(0),
			sending: Cell::new(false),
			done: Cell::new(false),
		});

		this.to.set_text(&fields.to);
		this.cc.set_text(&fields.cc);
		this.bcc.set_text(&fields.bcc);
		this.subject.set_text(&fields.subject);
		this.body.buffer().set_text(&fields.body);
		this.body.buffer().place_cursor(&this.body.buffer().start_iter());
		let show_cc = !fields.cc.is_empty() || !fields.bcc.is_empty();
		cc_toggle.set_active(show_cc);
		this.show_cc(show_cc);
		this.refresh_files();
		this.show_quote();
		this.update_title();

		let weak = Rc::downgrade(&this);
		cc_toggle.connect_toggled(move |t| {
			if let Some(this) = weak.upgrade() {
				this.show_cc(t.is_active());
			}
		});
		let weak = Rc::downgrade(&this);
		this.subject.connect_changed(move |_| {
			if let Some(this) = weak.upgrade() {
				this.update_title();
			}
		});
		for entry in [&this.to, &this.cc, &this.bcc] {
			entry.connect_changed(|e| e.remove_css_class("error"));
		}
		let weak = Rc::downgrade(&this);
		this.subject.connect_activate(move |_| {
			if let Some(this) = weak.upgrade() {
				this.body.grab_focus();
			}
		});
		let weak = Rc::downgrade(&this);
		this.send.connect_clicked(move |_| {
			if let Some(this) = weak.upgrade() {
				this.send();
			}
		});
		let weak = Rc::downgrade(&this);
		attach.connect_clicked(move |_| {
			if let Some(this) = weak.upgrade() {
				this.pick_files();
			}
		});
		let weak = Rc::downgrade(&this);
		discard.connect_clicked(move |_| {
			if let Some(this) = weak.upgrade() {
				this.ask_discard();
			}
		});
		for entry in [&this.to, &this.cc, &this.bcc] {
			suggest::attach(entry, &this.ctx);
		}
		this.connect_drop(&content);
		this.connect_keys();
		this.connect_close();

		if let Some((mail, atts)) = forward {
			for att in atts {
				this.forward_attachment(mail.clone(), att);
			}
		}

		// A draft opened again counts as saved until it changes.
		*this.saved.borrow_mut() = snapshot(&this.fields());
		this.window.present();
		for entry in [&this.to, &this.cc, &this.bcc, &this.subject] {
			entry.select_region(0, 0);
		}
		if fields.to.is_empty() {
			this.to.grab_focus();
		} else {
			this.body.grab_focus();
		}
		this
	}

	fn show_cc(&self, show: bool) {
		for w in &self.cc_rows {
			w.set_visible(show);
		}
		if show && self.window.is_visible() {
			self.cc.grab_focus();
		}
	}

	fn update_title(&self) {
		let subject = self.subject.text();
		self.title.set_title(if subject.trim().is_empty() { "New Message" } else { subject.trim() });
		self.window.set_title(Some(&self.title.title()));
	}

	fn fields(&self) -> Fields {
		let buffer = self.body.buffer();
		Fields {
			to: self.to.text().into(),
			cc: self.cc.text().into(),
			bcc: self.bcc.text().into(),
			subject: self.subject.text().into(),
			body: buffer.text(&buffer.start_iter(), &buffer.end_iter(), false).into(),
			attachments: self.attachments.borrow().clone(),
			previous: self.previous.clone(),
			existing: self.existing.borrow().clone(),
			kept: self.kept.borrow().clone(),
			// The main address is sent as none, so a draft from it stays as it was.
			from: self.addresses.get(self.from.selected() as usize).filter(|_| self.from.selected() > 0).cloned().unwrap_or_default(),
			quote: self.quote.borrow().clone(),
		}
	}

	/// Leaves out the lines picked in the reader.
	pub fn drop_picked(self: &Rc<Self>) {
		self.quote.borrow_mut().picked = None;
		self.show_quote();
	}

	/// Puts the whole earlier mail in the mail, or leaves it out.
	pub fn include_whole(self: &Rc<Self>, included: bool) {
		if let Some(whole) = self.quote.borrow_mut().whole.as_mut() {
			whole.included = included;
		}
		self.show_quote();
	}

	/// For the screenshot scenes: what the mail carries of the earlier one now.
	pub fn quote(&self) -> Quote {
		self.quote.borrow().clone()
	}

	/// Shows the picked lines and the whole mail's pill as the quote has them.
	fn show_quote(self: &Rc<Self>) {
		let quote = self.quote.borrow().clone();
		while let Some(child) = self.picked.first_child() {
			self.picked.remove(&child);
		}
		self.picked.set_visible(quote.picked.is_some());
		if let Some((who, lines)) = &quote.picked {
			let head = gtk::Box::new(gtk::Orientation::Horizontal, 6);
			head.append(&gtk::Label::builder().label(format!("{who} wrote:")).xalign(0.0).hexpand(true).css_classes(["caption", "quote-who"]).build());
			let drop = gtk::Button::builder().icon_name("window-close-symbolic").tooltip_text("Leave This Quote Out").css_classes(["circular", "quote-button"]).valign(gtk::Align::Start).build();
			let weak = Rc::downgrade(self);
			drop.connect_clicked(move |_| {
				if let Some(this) = weak.upgrade() {
					this.drop_picked();
				}
			});
			head.append(&drop);
			self.picked.append(&head);
			self.picked.append(&gtk::Label::builder().label(lines).xalign(0.0).wrap(true).wrap_mode(gtk::pango::WrapMode::WordChar).selectable(true).build());
		}

		while let Some(child) = self.pill.first_child() {
			self.pill.remove(&child);
		}
		let Some(whole) = quote.whole else {
			self.pill.set_visible(false);
			self.whole.set_child(None::<&gtk::Widget>);
			return;
		};
		self.pill.set_visible(true);
		if whole.included {
			self.pill.add_css_class("included");
		} else {
			self.pill.remove_css_class("included");
		}
		let dots = gtk::ToggleButton::builder().label("•••").tooltip_text(if whole.forward { "Show the Forwarded Mail" } else { "Show the Earlier Mail" }).css_classes(["quote-button", "quote-dots"]).active(self.whole.reveals_child()).build();
		let w = self.whole.clone();
		dots.connect_toggled(move |b| w.set_reveal_child(b.is_active()));
		self.pill.append(&dots);
		self.pill.append(&gtk::Separator::new(gtk::Orientation::Vertical));
		if whole.included {
			self.pill.append(&gtk::Label::builder().label(&whole.label).css_classes(["caption", "quote-label"]).ellipsize(gtk::pango::EllipsizeMode::End).build());
			self.pill.append(&gtk::Separator::new(gtk::Orientation::Vertical));
			let drop = gtk::Button::builder().icon_name("window-close-symbolic").tooltip_text(if whole.forward { "Leave the Forwarded Mail Out" } else { "Leave the Quote Out" }).css_classes(["quote-button"]).build();
			let weak = Rc::downgrade(self);
			drop.connect_clicked(move |_| {
				if let Some(this) = weak.upgrade() {
					this.include_whole(false);
				}
			});
			self.pill.append(&drop);
		} else {
			let add = gtk::Button::builder().label(format!("+ Quote {}", whole.label)).css_classes(["flat", "caption", "quote-add"]).build();
			let weak = Rc::downgrade(self);
			add.connect_clicked(move |_| {
				if let Some(this) = weak.upgrade() {
					this.include_whole(true);
				}
			});
			self.pill.append(&add);
		}
		let text = gtk::Label::builder()
			.label(format!("{}\n{}", whole.head.trim_end(), whole.text))
			.xalign(0.0)
			.wrap(true)
			.wrap_mode(gtk::pango::WrapMode::WordChar)
			.selectable(true)
			.css_classes(if whole.included { vec!["quote-whole"] } else { vec!["quote-whole", "left-out"] })
			.margin_start(24)
			.margin_end(24)
			.margin_bottom(18)
			.build();
		self.whole.set_child(Some(&text));
	}

	fn toast(&self, text: &str) {
		let toast = adw::Toast::new(text);
		toast.set_timeout(5);
		self.toasts.add_toast(toast);
	}

	fn update_send(&self) {
		let busy = self.sending.get() || self.pending.get() > 0;
		self.send.set_sensitive(!busy);
		if self.sending.get() {
			self.send.set_child(Some(&adw::Spinner::new()));
		} else {
			self.send.set_label("Send");
		}
	}

	fn refresh_files(self: &Rc<Self>) {
		self.files.remove_all();
		let kept = self.kept.borrow();
		for (i, att) in kept.iter().enumerate() {
			let row = adw::ActionRow::builder().title(glib::markup_escape_text(&att.name)).subtitle(format::size(att.size)).build();
			row.add_prefix(&gtk::Image::from_gicon(&icon_for(&att.mime)));
			let remove = gtk::Button::builder().icon_name("edit-delete-symbolic").valign(gtk::Align::Center).css_classes(["flat"]).tooltip_text("Remove").build();
			let weak = Rc::downgrade(self);
			remove.connect_clicked(move |_| {
				if let Some(this) = weak.upgrade() {
					this.kept.borrow_mut().remove(i);
					this.refresh_files();
				}
			});
			row.add_suffix(&remove);
			self.files.append(&row);
		}
		let kept_count = kept.len();
		let kept_size: u64 = kept.iter().map(|a| a.size).sum();
		drop(kept);
		let atts = self.attachments.borrow();
		for (i, att) in atts.iter().enumerate() {
			let row = adw::ActionRow::builder().title(glib::markup_escape_text(&att.name)).subtitle(format::size(att.data.len() as u64)).build();
			let icon = gtk::Image::from_gicon(&icon_for(&att.mime));
			row.add_prefix(&icon);
			let remove = gtk::Button::builder().icon_name("edit-delete-symbolic").valign(gtk::Align::Center).css_classes(["flat"]).tooltip_text("Remove").build();
			let weak = Rc::downgrade(self);
			remove.connect_clicked(move |_| {
				if let Some(this) = weak.upgrade() {
					this.attachments.borrow_mut().remove(i);
					this.refresh_files();
				}
			});
			row.add_suffix(&remove);
			self.files.append(&row);
		}
		let pending = self.pending.get();
		let total: u64 = atts.iter().map(|a| a.data.len() as u64).sum::<u64>() + kept_size;
		let mut summary = match atts.len() + kept_count {
			0 => String::new(),
			1 => format!("1 attachment · {}", format::size(total)),
			n => format!("{n} attachments · {}", format::size(total)),
		};
		if pending > 0 {
			if !summary.is_empty() {
				summary.push_str(" · ");
			}
			summary.push_str(&format!("downloading {pending}…"));
		}
		self.files_summary.set_label(&summary);
		self.files_box.set_visible(atts.len() + kept_count > 0 || pending > 0);
	}

	fn add_attachment(self: &Rc<Self>, att: NewAttachment) {
		let before = self.fields().total_size();
		self.attachments.borrow_mut().push(att);
		let after = self.fields().total_size();
		if before <= MAX_TOTAL_BYTES && after > MAX_TOTAL_BYTES {
			self.toast("Attachments are over 25 MB, Tuta may refuse to send them");
		}
		self.refresh_files();
	}

	fn add_files(self: &Rc<Self>, files: Vec<gio::File>) {
		for file in files {
			let this = self.clone();
			this.pending.set(this.pending.get() + 1);
			this.update_send();
			this.refresh_files();
			let busy = this.ctx.activity.start(Word::Attaching);
			runtime::spawn_local(async move {
				let name = file.basename().map(|p| p.to_string_lossy().into_owned()).unwrap_or_else(|| "attachment".into());
				let result = file.load_contents_future().await;
				drop(busy);
				this.pending.set(this.pending.get() - 1);
				this.update_send();
				match result {
					Ok((bytes, _)) => this.add_attachment(NewAttachment { mime: mime_for(&name).into(), name, data: bytes.to_vec() }),
					Err(e) => {
						this.toast(&format!("Could not attach {name}: {}", e.message()));
						this.refresh_files();
					}
				}
			});
		}
	}

	fn forward_attachment(self: &Rc<Self>, mail: MailId, att: Attachment) {
		let Some(backend) = self.ctx.backend() else { return };
		self.pending.set(self.pending.get() + 1);
		self.update_send();
		self.refresh_files();
		let this = self.clone();
		let busy = self.ctx.activity.start(Word::Attaching);
		runtime::spawn_local(async move {
			let a = att.clone();
			let result = runtime::run(async move { backend.attachment(&mail, &a).await }).await;
			drop(busy);
			this.pending.set(this.pending.get() - 1);
			this.update_send();
			match result {
				Ok(data) => this.add_attachment(NewAttachment { name: att.name, mime: att.mime, data }),
				Err(e) => {
					this.toast(&format!("Could not attach {}: {e}", att.name));
					this.refresh_files();
				}
			}
		});
	}

	/// For the screenshot scenes: types into To, and counts the suggestions shown for it.
	pub fn type_to(&self, text: &str) -> usize {
		self.to.grab_focus();
		self.to.set_text(text);
		self.to.set_position(-1);
		suggest::open_count(&self.to)
	}

	/// For the screenshot scenes: types `text` as the message.
	pub fn type_body(&self, text: &str) {
		self.body.buffer().set_text(text);
	}

	/// For the screenshot scenes: the message as written, quotes apart.
	pub fn body_text(&self) -> String {
		let buffer = self.body.buffer();
		buffer.text(&buffer.start_iter(), &buffer.end_iter(), false).to_string()
	}

	/// For the screenshot scenes: the subject and To as written.
	pub fn subject_and_to(&self) -> (String, String) {
		(self.subject.text().into(), self.to.text().into())
	}

	/// Adds attachments directly, as the screenshot scene does.
	pub fn attach(self: &Rc<Self>, atts: Vec<NewAttachment>) {
		for a in atts {
			self.add_attachment(a);
		}
	}

	fn pick_files(self: &Rc<Self>) {
		let this = self.clone();
		runtime::spawn_local(async move {
			let dialog = gtk::FileDialog::builder().title("Attach Files").modal(true).build();
			let Ok(list) = dialog.open_multiple_future(Some(&this.window)).await else { return };
			let files = (0..list.n_items()).filter_map(|i| list.item(i).and_downcast::<gio::File>()).collect();
			this.add_files(files);
		});
	}

	fn connect_drop(self: &Rc<Self>, target_widget: &gtk::Box) {
		let target = gtk::DropTarget::new(gdk::FileList::static_type(), gdk::DragAction::COPY);
		let weak = Rc::downgrade(self);
		target.connect_drop(move |_, value, _, _| {
			let (Some(this), Ok(list)) = (weak.upgrade(), value.get::<gdk::FileList>()) else { return false };
			this.add_files(list.files());
			true
		});
		target_widget.add_controller(target);
	}

	fn connect_keys(self: &Rc<Self>) {
		let keys = gtk::ShortcutController::new();
		keys.set_propagation_phase(gtk::PropagationPhase::Capture);
		let weak = Rc::downgrade(self);
		let send = gtk::CallbackAction::new(move |_, _| {
			if let Some(this) = weak.upgrade() {
				this.send();
			}
			glib::Propagation::Stop
		});
		keys.add_shortcut(gtk::Shortcut::new(gtk::ShortcutTrigger::parse_string("<Control>Return|<Control>KP_Enter"), Some(send)));
		let window = self.window.clone();
		let entries = [self.to.clone(), self.cc.clone(), self.bcc.clone()];
		let close = gtk::CallbackAction::new(move |_, _| {
			// Escape first closes open address suggestions.
			if !entries.iter().any(suggest::dismiss) {
				window.close();
			}
			glib::Propagation::Stop
		});
		keys.add_shortcut(gtk::Shortcut::new(gtk::ShortcutTrigger::parse_string("<Control>w|Escape"), Some(close)));
		let weak = Rc::downgrade(self);
		let attach = gtk::CallbackAction::new(move |_, _| {
			if let Some(this) = weak.upgrade() {
				this.pick_files();
			}
			glib::Propagation::Stop
		});
		keys.add_shortcut(gtk::Shortcut::new(gtk::ShortcutTrigger::parse_string("<Control><Shift>a"), Some(attach)));
		self.window.add_controller(keys);
	}

	fn connect_close(self: &Rc<Self>) {
		let weak = Rc::downgrade(self);
		self.window.connect_close_request(move |_| {
			let Some(this) = weak.upgrade() else { return glib::Propagation::Proceed };
			if this.done.get() {
				return glib::Propagation::Proceed;
			}
			if this.sending.get() {
				return glib::Propagation::Stop;
			}
			let fields = this.fields();
			let unchanged = snapshot(&fields) == *this.saved.borrow();
			if (fields.is_empty() && fields.existing.is_none()) || unchanged {
				return glib::Propagation::Proceed;
			}
			if this.pending.get() > 0 {
				this.toast("Attachments are still downloading");
				return glib::Propagation::Stop;
			}
			this.save_and_close(fields);
			glib::Propagation::Stop
		});
		// Break the window ↔ composer cycle once it is gone.
		let keep = RefCell::new(Some(self.clone()));
		self.window.connect_destroy(move |_| {
			keep.borrow_mut().take();
		});
	}

	/// Closing keeps the message: it is saved to Drafts, with the window hidden meanwhile. If saving
	/// fails the window comes back with the reason.
	fn save_and_close(self: &Rc<Self>, fields: Fields) {
		let Some(backend) = self.ctx.backend() else { return };
		self.window.set_visible(false);
		let draft = fields.saved_draft();
		let replaced = draft.existing.clone();
		let quote = (fields.quote != Quote::default()).then(|| serde_json::to_string(&fields.quote).ok()).flatten();
		let this = self.clone();
		let busy = self.ctx.activity.start(Word::Drafting);
		runtime::spawn_local(async move {
			let result = runtime::run(async move { backend.save_draft(draft).await }).await;
			drop(busy);
			match result {
				Ok(id) => {
					if let Some(cache) = this.ctx.cache() {
						if let Some(old) = replaced.filter(|old| old != &id) {
							let _ = cache.remove(&old);
						}
						// Kept here so reopening shows the quotes as they were, not merged into the text.
						let _ = cache.set_draft_quote(&id, quote.as_deref());
					}
					this.done.set(true);
					this.window.close();
					this.ctx.toast("Saved to Drafts");
					this.ctx.changed(&[FolderKind::Drafts]);
				},
				Err(e) => {
					this.window.present();
					this.toast(&format!("Could not save the draft: {e}"));
				},
			}
		});
	}

	/// Throws the message away, and its saved draft with it, after asking.
	fn ask_discard(self: &Rc<Self>) {
		let fields = self.fields();
		if fields.is_empty() && fields.existing.is_none() {
			self.done.set(true);
			self.window.close();
			return;
		}
		let dialog = adw::AlertDialog::new(Some("Discard Message?"), Some("The message and its draft will be deleted."));
		dialog.add_responses(&[("cancel", "_Cancel"), ("discard", "_Discard")]);
		dialog.set_response_appearance("discard", adw::ResponseAppearance::Destructive);
		dialog.set_default_response(Some("cancel"));
		dialog.set_close_response("cancel");
		let this = self.clone();
		dialog.connect_response(None, move |_, response| {
			if response != "discard" {
				return;
			}
			this.done.set(true);
			this.window.close();
			let existing = this.existing.borrow().clone();
			if let (Some(id), Some(backend), Some(cache)) = (existing, this.ctx.backend(), this.ctx.cache()) {
				let _ = cache.remove(&id);
				this.ctx.changed(&[FolderKind::Drafts]);
				let ctx = this.ctx.clone();
				let busy = ctx.activity.start(Word::Deleting);
				runtime::spawn_local(async move {
					let result = runtime::run(async move { backend.trash(&id).await }).await;
					drop(busy);
					match result {
						Ok(()) => ctx.changed(&[FolderKind::Drafts, FolderKind::Trash]),
						Err(e) => ctx.error(&e),
					}
				});
			}
		});
		dialog.present(Some(&self.window));
	}

	pub fn send(self: &Rc<Self>) {
		if self.sending.get() || self.pending.get() > 0 {
			return;
		}
		let fields = self.fields();
		let subject = fields.subject.clone();
		let draft = match fields.build_draft() {
			Ok(d) => d,
			Err(invalid) => {
				for (entry, bad) in [(&self.to, &invalid.to), (&self.cc, &invalid.cc), (&self.bcc, &invalid.bcc)] {
					if !bad.is_empty() {
						entry.add_css_class("error");
					}
				}
				if !invalid.cc.is_empty() || !invalid.bcc.is_empty() {
					self.show_cc(true);
				}
				let first = [(&self.to, &invalid.to), (&self.cc, &invalid.cc), (&self.bcc, &invalid.bcc)].into_iter().find(|(_, b)| !b.is_empty());
				match first {
					Some((e, _)) => e.grab_focus(),
					None => self.to.grab_focus(),
				};
				return self.toast(&invalid.message());
			}
		};
		let Some(backend) = self.ctx.backend() else { return self.toast("Signed out, please sign in again") };
		self.sending.set(true);
		self.update_send();
		let this = self.clone();
		let busy = self.ctx.activity.start(Word::Sending);
		runtime::spawn_local(async move {
			let result = runtime::run(async move { backend.send(draft).await }).await;
			drop(busy);
			this.sending.set(false);
			this.update_send();
			match result {
				Ok(()) => {
					if let (Some(old), Some(cache)) = (this.existing.borrow().clone(), this.ctx.cache()) {
						let _ = cache.remove(&old);
					}
					this.done.set(true);
					this.window.close();
					this.ctx.sent(&subject);
				}
				Err(e) => this.toast(&e.to_string()),
			}
		});
	}
}

/// What decides whether a message changed since it was saved.
fn snapshot(f: &Fields) -> Snapshot {
	(f.from.clone(), f.to.clone(), f.cc.clone(), f.bcc.clone(), f.subject.clone(), f.body.clone(), f.attachments.len(), f.kept.len(), f.quote.clone())
}

type Snapshot = (String, String, String, String, String, String, usize, usize, Quote);

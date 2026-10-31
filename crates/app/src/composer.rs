//! A window for writing one mail.

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use adw::prelude::*;
use gtk::{gdk, gio, glib};
use tern_core::{mime_for, Attachment, FolderKind, MailId, NewAttachment};

use crate::compose_model::{Fields, MAX_TOTAL_BYTES};
use crate::{attachments::icon_for, format, runtime, suggest, window::Ctx};

pub struct Composer {
	ctx: Ctx,
	pub window: adw::Window,
	title: adw::WindowTitle,
	toasts: adw::ToastOverlay,
	to: gtk::Entry,
	cc: gtk::Entry,
	bcc: gtk::Entry,
	cc_rows: [gtk::Widget; 4],
	subject: gtk::Entry,
	body: gtk::TextView,
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
	saved: RefCell<(String, String, String, String, String, usize, usize)>,
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
			to_row.upcast_ref::<gtk::Widget>(),
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
		let body_scroll = gtk::ScrolledWindow::builder().hscrollbar_policy(gtk::PolicyType::Never).child(&body).vexpand(true).build();

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
			to,
			cc,
			bcc,
			cc_rows,
			subject,
			body,
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
		}
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
			runtime::spawn_local(async move {
				let name = file.basename().map(|p| p.to_string_lossy().into_owned()).unwrap_or_else(|| "attachment".into());
				let result = file.load_contents_future().await;
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
		runtime::spawn_local(async move {
			let a = att.clone();
			let result = runtime::run(async move { backend.attachment(&mail, &a).await }).await;
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
		let this = self.clone();
		runtime::spawn_local(async move {
			let result = runtime::run(async move { backend.save_draft(draft).await }).await;
			match result {
				Ok(id) => {
					if let (Some(old), Some(cache)) = (replaced, this.ctx.cache()) {
						if old != id {
							let _ = cache.remove(&old);
						}
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
				runtime::spawn_local(async move {
					match runtime::run(async move { backend.trash(&id).await }).await {
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
		runtime::spawn_local(async move {
			let result = runtime::run(async move { backend.send(draft).await }).await;
			this.sending.set(false);
			this.update_send();
			match result {
				Ok(()) => {
					if let (Some(old), Some(cache)) = (this.existing.borrow().clone(), this.ctx.cache()) {
						let _ = cache.remove(&old);
					}
					this.done.set(true);
					this.window.close();
					this.ctx.sent();
				}
				Err(e) => this.toast(&e.to_string()),
			}
		});
	}
}

/// What decides whether a message changed since it was saved.
fn snapshot(f: &Fields) -> (String, String, String, String, String, usize, usize) {
	(f.to.clone(), f.cc.clone(), f.bcc.clone(), f.subject.clone(), f.body.clone(), f.attachments.len(), f.kept.len())
}

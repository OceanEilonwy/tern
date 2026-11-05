//! A window for writing one mail. What happens to it is the core's: this draws one composer of those
//! it publishes, and sends back what is written.

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use adw::prelude::*;
use gtk::{gdk, gio, glib};
use tern_app::view::{Action, AddressField, ComposeText, ComposerAttachment, ComposerView};
use tern_compose::mime_for;

use crate::activity::Word;
use crate::suggest::Suggest;
use crate::{attachments::icon_for, runtime, window::Ctx};

pub struct Composer {
	ctx: Ctx,
	pub id: u64,
	pub window: adw::Window,
	title: adw::WindowTitle,
	toasts: adw::ToastOverlay,
	/// The address to send from, among the account's; shown only when it has more than one.
	pub from: gtk::DropDown,
	to: gtk::Entry,
	cc: gtk::Entry,
	bcc: gtk::Entry,
	cc_toggle: gtk::ToggleButton,
	cc_rows: [gtk::Widget; 4],
	subject: gtk::Entry,
	body: gtk::TextView,
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
	suggestions: [(AddressField, Rc<Suggest>); 3],
	/// What the core last said.
	shown: RefCell<Option<ComposerView>>,
	/// Files being read from disk, on their way to the core.
	reading: Cell<usize>,
	/// Drawing what the core said, which is not the person doing anything.
	drawing: Cell<bool>,
	/// How many problems have been shown, for the screenshot scenes.
	toasts_shown: Cell<u32>,
	/// The core has finished with it, so it may close.
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
	/// Opens the window for a composer the core has just opened.
	pub fn open(ctx: &Ctx, view: &ComposerView) -> Rc<Composer> {
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
		let addresses = &view.addresses;
		let from = gtk::DropDown::from_strings(&addresses.iter().map(String::as_str).collect::<Vec<_>>());
		from.update_property(&[gtk::accessible::Property::Label("From")]);
		from.add_css_class("flat");
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

		let suggestions = [(AddressField::To, Suggest::new(&to)), (AddressField::Cc, Suggest::new(&cc)), (AddressField::Bcc, Suggest::new(&bcc))];
		let this = Rc::new(Composer {
			ctx: ctx.clone(),
			id: view.id,
			window,
			title,
			toasts,
			from,
			to,
			cc,
			bcc,
			cc_toggle: cc_toggle.clone(),
			cc_rows,
			subject,
			body,
			picked,
			pill,
			whole,
			send,
			files,
			files_box,
			files_summary,
			suggestions,
			shown: RefCell::default(),
			reading: Cell::new(0),
			drawing: Cell::new(false),
			toasts_shown: Cell::new(0),
			done: Cell::new(false),
		});

		this.show(view);
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
		for (field, suggest) in &this.suggestions {
			let entry = this.entry(*field).clone();
			let weak = Rc::downgrade(&this);
			let field = *field;
			entry.connect_changed(move |entry| {
				entry.remove_css_class("error");
				let Some(this) = weak.upgrade().filter(|t| !t.drawing.get()) else { return };
				if this.shown.borrow().as_ref().is_some_and(|v| v.error.is_some() || v.bad_to || v.bad_cc || v.bad_bcc) {
					this.act(Action::ComposeEdited);
				}
				this.act(Action::ComposeSuggest { field, text: entry.text().to_string() });
			});
			let weak = Rc::downgrade(&this);
			suggest.connect_take(move |index| {
				if let Some(this) = weak.upgrade() {
					this.act(Action::ComposeTakeSuggestion { text: this.text(), field, index });
				}
			});
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
				this.discard();
			}
		});
		this.connect_drop(&content);
		this.connect_keys();
		this.connect_close();

		this.window.present();
		for entry in [&this.to, &this.cc, &this.bcc, &this.subject] {
			entry.select_region(0, 0);
		}
		if view.text.to.is_empty() {
			this.to.grab_focus();
		} else {
			this.body.grab_focus();
		}
		this
	}

	fn entry(&self, field: AddressField) -> &gtk::Entry {
		match field {
			AddressField::To => &self.to,
			AddressField::Cc => &self.cc,
			AddressField::Bcc => &self.bcc,
		}
	}

	/// Tells the core something about this composer.
	fn act(&self, action: Action) {
		self.ctx.engine.send(Action::ComposeFocus { id: self.id });
		self.ctx.engine.send(action);
	}

	/// What is written now.
	fn text(&self) -> ComposeText {
		let buffer = self.body.buffer();
		ComposeText {
			to: self.to.text().into(),
			cc: self.cc.text().into(),
			bcc: self.bcc.text().into(),
			subject: self.subject.text().into(),
			body: buffer.text(&buffer.start_iter(), &buffer.end_iter(), false).into(),
			from_index: self.from.selected(),
		}
	}

	/// Draws what the core says about this composer.
	pub fn show(self: &Rc<Self>, view: &ComposerView) {
		self.drawing.set(true);
		let before = self.shown.borrow_mut().replace(view.clone());
		// The text is the person's while they write; the core only sets it when it changes it itself.
		if before.as_ref().map(|b| b.revision) != Some(view.revision) {
			for (entry, text) in [(&self.to, &view.text.to), (&self.cc, &view.text.cc), (&self.bcc, &view.text.bcc), (&self.subject, &view.text.subject)] {
				if entry.text() != text.as_str() {
					entry.set_text(text);
					entry.set_position(-1);
				}
			}
			let buffer = self.body.buffer();
			if buffer.text(&buffer.start_iter(), &buffer.end_iter(), false) != view.text.body {
				buffer.set_text(&view.text.body);
				buffer.place_cursor(&buffer.start_iter());
			}
			self.from.set_selected(view.text.from_index);
		}
		if view.show_cc && !self.cc_toggle.is_active() {
			self.cc_toggle.set_active(true);
			self.show_cc(true);
		} else if before.is_none() {
			self.show_cc(view.show_cc);
		}
		let quote = |v: &ComposerView| (v.picked_who.clone(), v.picked_text.clone(), v.whole_label.clone(), v.whole_text.clone(), v.whole_included, v.whole_forward);
		if before.as_ref().map(quote) != Some(quote(view)) {
			self.show_quote(view);
		}
		if before.as_ref().map(|b| &b.attachments) != Some(&view.attachments) || before.as_ref().map(|b| &b.total) != Some(&view.total) {
			self.show_files(&view.attachments, view.total.as_deref());
		}
		self.update_send();
		for (entry, bad, was) in [(&self.to, view.bad_to, before.as_ref().is_some_and(|b| b.bad_to)), (&self.cc, view.bad_cc, before.as_ref().is_some_and(|b| b.bad_cc)), (&self.bcc, view.bad_bcc, before.as_ref().is_some_and(|b| b.bad_bcc))] {
			if bad {
				entry.add_css_class("error");
			}
			if bad && !was {
				entry.grab_focus();
			}
		}
		// Trying again and getting the same answer is worth saying again.
		let told = before.as_ref().is_some_and(|b| b.error == view.error && b.tries == view.tries);
		if let Some(error) = view.error.as_ref().filter(|_| !told) {
			let toast = adw::Toast::new(error);
			toast.set_timeout(5);
			self.toasts.add_toast(toast);
			self.toasts_shown.set(self.toasts_shown.get() + 1);
		}
		for (field, suggest) in &self.suggestions {
			suggest.show(if view.suggest_field == Some(*field) { &view.suggestions } else { &[] });
		}
		// Saving goes on behind the screen; it comes back if that fails.
		let was_saving = before.as_ref().is_some_and(|b| b.saving);
		if view.saving && !was_saving {
			self.window.set_visible(false);
		} else if was_saving && !view.saving {
			self.window.present();
		}
		self.drawing.set(false);
	}

	/// The core has finished with this composer.
	pub fn finish(&self) {
		self.done.set(true);
		// Closing would only close a dialog open over it, such as the one asking to discard.
		self.window.destroy();
	}

	fn show_cc(&self, show: bool) {
		for w in &self.cc_rows {
			w.set_visible(show);
		}
		if show && self.window.is_visible() && !self.drawing.get() {
			self.cc.grab_focus();
		}
	}

	fn update_title(&self) {
		let subject = self.subject.text();
		self.title.set_title(if subject.trim().is_empty() { "New Message" } else { subject.trim() });
		self.window.set_title(Some(&self.title.title()));
	}

	/// Leaves out the lines picked in the reader.
	pub fn drop_picked(&self) {
		self.act(Action::ComposeDropPicked);
	}

	/// Puts the whole earlier mail in the mail, or leaves it out.
	pub fn include_whole(&self, included: bool) {
		self.act(Action::ComposeIncludeWhole { included });
	}

	/// For the screenshot scenes: what the core says about this composer.
	pub fn view(&self) -> Option<ComposerView> {
		self.shown.borrow().clone()
	}

	/// Shows the picked lines and the whole mail's pill as the core has them.
	fn show_quote(self: &Rc<Self>, view: &ComposerView) {
		while let Some(child) = self.picked.first_child() {
			self.picked.remove(&child);
		}
		self.picked.set_visible(view.picked_text.is_some());
		if let (Some(who), Some(lines)) = (&view.picked_who, &view.picked_text) {
			let head = gtk::Box::new(gtk::Orientation::Horizontal, 6);
			head.append(&gtk::Label::builder().label(who).xalign(0.0).hexpand(true).css_classes(["caption", "quote-who"]).build());
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
		let (Some(label), Some(text)) = (&view.whole_label, &view.whole_text) else {
			self.pill.set_visible(false);
			self.whole.set_child(None::<&gtk::Widget>);
			return;
		};
		self.pill.set_visible(true);
		if view.whole_included {
			self.pill.add_css_class("included");
		} else {
			self.pill.remove_css_class("included");
		}
		let dots = gtk::ToggleButton::builder().label("•••").tooltip_text(if view.whole_forward { "Show the Forwarded Mail" } else { "Show the Earlier Mail" }).css_classes(["quote-button", "quote-dots"]).active(self.whole.reveals_child()).build();
		let w = self.whole.clone();
		dots.connect_toggled(move |b| w.set_reveal_child(b.is_active()));
		self.pill.append(&dots);
		self.pill.append(&gtk::Separator::new(gtk::Orientation::Vertical));
		if view.whole_included {
			self.pill.append(&gtk::Label::builder().label(label).css_classes(["caption", "quote-label"]).ellipsize(gtk::pango::EllipsizeMode::End).build());
			self.pill.append(&gtk::Separator::new(gtk::Orientation::Vertical));
			let drop = gtk::Button::builder().icon_name("window-close-symbolic").tooltip_text(if view.whole_forward { "Leave the Forwarded Mail Out" } else { "Leave the Quote Out" }).css_classes(["quote-button"]).build();
			let weak = Rc::downgrade(self);
			drop.connect_clicked(move |_| {
				if let Some(this) = weak.upgrade() {
					this.include_whole(false);
				}
			});
			self.pill.append(&drop);
		} else {
			let add = gtk::Button::builder().label(format!("+ Quote {label}")).css_classes(["flat", "caption", "quote-add"]).build();
			let weak = Rc::downgrade(self);
			add.connect_clicked(move |_| {
				if let Some(this) = weak.upgrade() {
					this.include_whole(true);
				}
			});
			self.pill.append(&add);
		}
		let shown = gtk::Label::builder()
			.label(text.trim_end())
			.xalign(0.0)
			.wrap(true)
			.wrap_mode(gtk::pango::WrapMode::WordChar)
			.selectable(true)
			.css_classes(if view.whole_included { vec!["quote-whole"] } else { vec!["quote-whole", "left-out"] })
			.margin_start(24)
			.margin_end(24)
			.margin_bottom(18)
			.build();
		self.whole.set_child(Some(&shown));
	}

	fn update_send(&self) {
		let view = self.shown.borrow();
		let pending = view.as_ref().is_some_and(|v| v.attachments.iter().any(|a| a.pending));
		let sending = view.as_ref().is_some_and(|v| v.sending);
		self.send.set_sensitive(!sending && !pending && self.reading.get() == 0);
		if sending {
			self.send.set_child(Some(&adw::Spinner::new()));
		} else if self.send.label().as_deref() != Some("Send") {
			self.send.set_label("Send");
		}
	}

	fn show_files(self: &Rc<Self>, attachments: &[ComposerAttachment], total: Option<&str>) {
		self.files.remove_all();
		let mut waiting = 0;
		for (i, att) in attachments.iter().enumerate() {
			if att.pending {
				waiting += 1;
				continue;
			}
			let row = adw::ActionRow::builder().title(glib::markup_escape_text(&att.name)).subtitle(&att.size).build();
			row.add_prefix(&gtk::Image::from_gicon(&icon_for(&att.mime)));
			let remove = gtk::Button::builder().icon_name("edit-delete-symbolic").valign(gtk::Align::Center).css_classes(["flat"]).tooltip_text("Remove").build();
			let weak = Rc::downgrade(self);
			remove.connect_clicked(move |_| {
				if let Some(this) = weak.upgrade() {
					this.act(Action::ComposeRemoveAttachment { index: i as u32 });
				}
			});
			row.add_suffix(&remove);
			self.files.append(&row);
		}
		let ready = attachments.len() - waiting;
		let size = total.unwrap_or_default();
		let mut summary = match ready {
			0 => String::new(),
			1 => format!("1 attachment · {size}"),
			n => format!("{n} attachments · {size}"),
		};
		if waiting > 0 {
			if !summary.is_empty() {
				summary.push_str(" · ");
			}
			summary.push_str(&format!("downloading {waiting}…"));
		}
		self.files_summary.set_label(&summary);
		self.files_box.set_visible(!attachments.is_empty());
	}

	fn add_files(self: &Rc<Self>, files: Vec<gio::File>) {
		for file in files {
			let this = self.clone();
			this.reading.set(this.reading.get() + 1);
			this.update_send();
			let busy = this.ctx.activity.start(Word::Attaching);
			runtime::spawn_local(async move {
				let name = file.basename().map(|p| p.to_string_lossy().into_owned()).unwrap_or_else(|| "attachment".into());
				let result = file.load_contents_future().await;
				drop(busy);
				this.reading.set(this.reading.get() - 1);
				this.update_send();
				match result {
					Ok((bytes, _)) => this.act(Action::ComposeAttach { mime: mime_for(&name).into(), name, data: bytes.to_vec() }),
					Err(_) => this.act(Action::ComposeAttachFailed { name }),
				}
			});
		}
	}

	/// For the screenshot scenes: how many problems have been shown.
	pub fn toasts_shown(&self) -> u32 {
		self.toasts_shown.get()
	}

	/// For the screenshot scenes: types into To.
	pub fn type_to(&self, text: &str) {
		self.to.grab_focus();
		self.to.set_text(text);
		self.to.set_position(-1);
	}

	/// For the screenshot scenes: how many suggestions are open under To.
	pub fn suggested(&self) -> usize {
		self.suggestions[0].1.open_count()
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

	/// For the screenshot scenes: adds an attachment, as picking a file does.
	pub fn attach(&self, name: &str, mime: &str, data: &[u8]) {
		self.act(Action::ComposeAttach { name: name.into(), mime: mime.into(), data: data.to_vec() });
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
		let weak = Rc::downgrade(self);
		let close = gtk::CallbackAction::new(move |_, _| {
			if let Some(this) = weak.upgrade() {
				// Escape first closes open address suggestions.
				if !this.suggestions.iter().any(|(_, s)| s.dismiss()) {
					this.window.close();
				}
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
			// Closing keeps the message, which is the core's to do; it says when this may go.
			this.act(Action::ComposeClose { text: this.text() });
			glib::Propagation::Stop
		});
	}

	pub fn send(&self) {
		self.act(Action::ComposeSend { text: self.text() });
	}

	/// Throws the mail away, once the core has asked if it is worth asking.
	pub fn discard(&self) {
		self.ctx.ask_discard_from(self.id, &self.window);
		self.act(Action::ComposeDiscard { text: self.text() });
	}

	/// For the screenshot scenes: adds files, as dragging them in does.
	pub fn drop_files(self: &Rc<Self>, files: Vec<gio::File>) {
		self.add_files(files);
	}
}

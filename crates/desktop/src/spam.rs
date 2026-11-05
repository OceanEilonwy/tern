//! Move to Spam: the conversation on show, and other mail like it picked from a list, as the core
//! offers them.
//!
//! The list is what a rule's line catches, starting with the sender's address; Sender, Domain and
//! Subject fill the line in, and it can be edited. The conversation's own mails head the list, ticked.
//! The line can also be kept as a rule, so new mail like it goes to Spam as it arrives.

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use adw::prelude::*;
use gtk::glib;
use tern_app::view::{Action, ChoiceView, SpamRow};

use crate::window::Ctx;

/// What the core says about the sheet.
pub struct Offer {
	pub revision: u64,
	pub line: String,
	pub problem: Option<String>,
	pub rows: Vec<SpamRow>,
	pub count: String,
	pub keep_rule: bool,
	pub confirm: String,
	pub choices: Vec<ChoiceView>,
}

pub struct Picker {
	ctx: Ctx,
	pub dialog: adw::Dialog,
	pub entry: gtk::Entry,
	problem: gtk::Label,
	choices: gtk::Box,
	list: gtk::ListBox,
	pub all: gtk::CheckButton,
	count: gtk::Label,
	pub keep: adw::SwitchRow,
	pub confirm: gtk::Button,
	shown: RefCell<Vec<(SpamRow, gtk::CheckButton)>>,
	revision: Cell<u64>,
	/// Drawing what the core said, which is not the person doing anything.
	filling: Cell<bool>,
}

/// Opens the dialog; [`Picker::show`] then fills it in.
pub fn open(ctx: &Ctx) -> Rc<Picker> {
	let entry = gtk::Entry::builder().css_classes(["monospace"]).hexpand(true).build();
	entry.update_property(&[gtk::accessible::Property::Label("Mail like this")]);
	let problem = gtk::Label::builder().xalign(0.0).wrap(true).css_classes(["caption", "error"]).visible(false).build();
	let choices = gtk::Box::builder().css_classes(["linked"]).halign(gtk::Align::Start).build();
	let all = gtk::CheckButton::builder().label("Select All").build();
	let count = gtk::Label::builder().xalign(1.0).hexpand(true).css_classes(["dim-label", "caption"]).build();
	let bar = gtk::Box::builder().spacing(12).build();
	bar.append(&all);
	bar.append(&count);
	let list = gtk::ListBox::builder().selection_mode(gtk::SelectionMode::None).css_classes(["boxed-list"]).valign(gtk::Align::Start).build();
	list.update_property(&[gtk::accessible::Property::Label("Mail like this")]);
	let scroll = gtk::ScrolledWindow::builder().hscrollbar_policy(gtk::PolicyType::Never).child(&list).vexpand(true).min_content_height(200).build();
	let keep = adw::SwitchRow::builder().title("Always Move Mail Like This to Spam").subtitle("New mail this line catches goes straight to Spam; rules are in Preferences").build();
	let keep_list = gtk::ListBox::builder().selection_mode(gtk::SelectionMode::None).css_classes(["boxed-list"]).build();
	keep_list.append(&keep);

	let column = gtk::Box::builder().orientation(gtk::Orientation::Vertical).spacing(12).margin_top(6).margin_bottom(18).margin_start(18).margin_end(18).build();
	let heading = gtk::Label::builder().label("Find more mail like this").xalign(0.0).css_classes(["heading"]).build();
	column.append(&heading);
	column.append(&choices);
	column.append(&entry);
	column.append(&problem);
	column.append(&bar);
	column.append(&scroll);
	column.append(&keep_list);

	let confirm = gtk::Button::builder().label("Move to Spam").css_classes(["destructive-action"]).build();
	let cancel = gtk::Button::builder().label("Cancel").build();
	let header = adw::HeaderBar::builder().show_end_title_buttons(false).show_start_title_buttons(false).build();
	header.pack_start(&cancel);
	header.pack_end(&confirm);
	let view = adw::ToolbarView::new();
	view.add_top_bar(&header);
	view.set_content(Some(&column));
	let dialog = adw::Dialog::builder().title("Move to Spam").content_width(560).content_height(640).child(&view).default_widget(&confirm).build();

	let this = Rc::new(Picker {
		ctx: ctx.clone(),
		dialog,
		entry,
		problem,
		choices,
		list,
		all,
		count,
		keep,
		confirm,
		shown: RefCell::default(),
		revision: Cell::new(0),
		filling: Cell::new(false),
	});
	let weak = Rc::downgrade(&this);
	this.entry.connect_changed(move |entry| {
		if let Some(this) = weak.upgrade().filter(|t| !t.filling.get()) {
			this.ctx.engine.send(Action::SpamLine { line: entry.text().to_string() });
		}
	});
	let weak = Rc::downgrade(&this);
	this.all.connect_toggled(move |all| {
		if let Some(this) = weak.upgrade().filter(|t| !t.filling.get()) {
			this.ctx.engine.send(Action::SpamSelectAll { on: all.is_active() });
		}
	});
	let weak = Rc::downgrade(&this);
	this.keep.connect_active_notify(move |keep| {
		if let Some(this) = weak.upgrade().filter(|t| !t.filling.get()) {
			this.ctx.engine.send(Action::SpamKeepRule { on: keep.is_active() });
		}
	});
	let engine = ctx.engine.clone();
	this.confirm.connect_clicked(move |_| engine.send(Action::SpamConfirm));
	let dialog = this.dialog.clone();
	cancel.connect_clicked(move |_| {
		dialog.close();
	});
	let engine = ctx.engine.clone();
	this.dialog.connect_closed(move |_| engine.send(Action::CloseSheet));
	this.dialog.present(Some(&ctx.window));
	this.entry.grab_focus();
	this
}

impl Picker {
	/// Draws what the core offers.
	pub fn show(self: &Rc<Self>, offer: Offer) {
		self.filling.set(true);
		// The line is the person's while they type; the core only sets it when it changes it itself.
		if self.revision.replace(offer.revision) != offer.revision {
			self.entry.set_text(&offer.line);
		}
		self.problem.set_visible(offer.problem.is_some());
		self.problem.set_label(offer.problem.as_deref().unwrap_or_default());
		if offer.problem.is_some() {
			self.entry.add_css_class("error");
		} else {
			self.entry.remove_css_class("error");
		}
		if self.choices.first_child().is_none() {
			for choice in &offer.choices {
				let button = gtk::Button::builder().label(&choice.label).build();
				let (engine, value) = (self.ctx.engine.clone(), choice.value.clone());
				button.connect_clicked(move |_| engine.send(Action::SpamUse { value: value.clone() }));
				self.choices.append(&button);
			}
		}
		let same = {
			let shown = self.shown.borrow();
			shown.len() == offer.rows.len() && shown.iter().zip(&offer.rows).all(|((a, _), b)| a.id == b.id)
		};
		if same {
			for ((row, check), new) in self.shown.borrow_mut().iter_mut().zip(&offer.rows) {
				check.set_active(new.picked);
				*row = new.clone();
			}
		} else {
			self.fill(&offer.rows);
		}
		let picked = offer.rows.iter().filter(|r| r.picked).count();
		self.count.set_label(&offer.count);
		self.all.set_active(picked > 0 && picked == offer.rows.len());
		self.all.set_inconsistent(picked > 0 && picked < offer.rows.len());
		self.keep.set_active(offer.keep_rule);
		self.confirm.set_sensitive(picked > 0 || offer.keep_rule);
		self.confirm.set_label(&offer.confirm);
		self.filling.set(false);
	}

	fn fill(self: &Rc<Self>, rows: &[SpamRow]) {
		self.list.remove_all();
		let mut shown = Vec::new();
		for mail in rows {
			let check = gtk::CheckButton::builder().active(mail.picked).valign(gtk::Align::Center).build();
			let row = adw::ActionRow::builder().title(glib::markup_escape_text(&mail.who)).subtitle(glib::markup_escape_text(&mail.subject)).subtitle_lines(1).activatable_widget(&check).build();
			row.add_prefix(&check);
			row.add_suffix(&gtk::Label::builder().label(&mail.date).css_classes(["dim-label", "caption", "numeric"]).build());
			self.list.append(&row);
			let (weak, id) = (Rc::downgrade(self), mail.id.clone());
			check.connect_toggled(move |_| {
				if let Some(this) = weak.upgrade().filter(|t| !t.filling.get()) {
					this.ctx.engine.send(Action::SpamToggle { id: id.clone() });
				}
			});
			shown.push((mail.clone(), check));
		}
		*self.shown.borrow_mut() = shown;
	}

	/// For the screenshot scenes: the subjects listed, and whether each is ticked.
	pub fn listed(&self) -> Vec<(String, bool)> {
		self.shown.borrow().iter().map(|(m, c)| (m.subject.clone(), c.is_active())).collect()
	}
}

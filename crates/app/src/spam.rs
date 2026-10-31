//! Move to Spam: the conversation on show, and other mail like it picked from a list.
//!
//! The list is what a rule's line catches (see [`tern_core::rules`]), starting with the sender's
//! address; Sender, Domain and Subject fill the line in, and it can be edited. The conversation's
//! own mails head the list, ticked. The line can also be kept as a rule, so new mail like it goes
//! to Spam as it arrives.

use std::cell::{Cell, RefCell};
use std::collections::HashSet;
use std::rc::Rc;
use std::time::Duration;

use adw::prelude::*;
use gtk::glib;
use tern_core::rules::{parse, term, Action, Rule};
use tern_core::{FolderId, FolderKind, MailId, MailSummary};

use crate::activity::Word;
use crate::window::Ctx;
use crate::{format, rules, runtime};

/// Matches listed at most; Select All picks these.
const SHOWN: usize = 500;
/// Mail in these is not offered: it is already gone, or this account's own.
const SKIP: [FolderKind; 4] = [FolderKind::Spam, FolderKind::Trash, FolderKind::Sent, FolderKind::Drafts];

pub struct Picker {
	ctx: Ctx,
	pub dialog: adw::Dialog,
	pub entry: gtk::Entry,
	problem: gtk::Label,
	list: gtk::ListBox,
	pub all: gtk::CheckButton,
	count: gtk::Label,
	pub keep: adw::SwitchRow,
	pub confirm: gtk::Button,
	/// The conversation's own mails, listed first whatever the line says.
	own: Vec<MailSummary>,
	shown: RefCell<Vec<(MailSummary, gtk::CheckButton)>>,
	picked: RefCell<HashSet<MailId>>,
	spam: FolderId,
	/// Set while ticks are changed from here, so they do not count as the user's.
	filling: Cell<bool>,
	pending: RefCell<Option<glib::SourceId>>,
	done: Box<dyn Fn()>,
}

/// Opens the dialog for `own`, the conversation's mails; `done` runs once mail has moved.
pub fn open(ctx: &Ctx, own: Vec<MailSummary>, spam: FolderId, done: impl Fn() + 'static) -> Option<Rc<Picker>> {
	let first = own.first()?.clone();
	let entry = gtk::Entry::builder().text(term("from", &first.from.address)).css_classes(["monospace"]).hexpand(true).build();
	entry.update_property(&[gtk::accessible::Property::Label("Mail like this")]);
	let problem = gtk::Label::builder().xalign(0.0).wrap(true).css_classes(["caption", "error"]).visible(false).build();
	let choices = gtk::Box::builder().css_classes(["linked"]).halign(gtk::Align::Start).build();
	let domain = first.from.address.rsplit_once('@').map(|(_, d)| format!("@{d}")).unwrap_or_default();
	for (label, line) in [
		("Sender", term("from", &first.from.address)),
		("Domain", term("from", &domain)),
		("Subject", term("subject", tern_core::bare_subject(&first.subject))),
	] {
		let button = gtk::Button::builder().label(label).build();
		let entry = entry.clone();
		button.connect_clicked(move |_| entry.set_text(&line));
		choices.append(&button);
	}
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

	let picked = own.iter().map(|m| m.id.clone()).collect();
	let this = Rc::new(Picker {
		ctx: ctx.clone(),
		dialog,
		entry,
		problem,
		list,
		all,
		count,
		keep,
		confirm,
		own,
		shown: RefCell::default(),
		picked: RefCell::new(picked),
		spam,
		filling: Cell::new(false),
		pending: RefCell::default(),
		done: Box::new(done),
	});
	let weak = Rc::downgrade(&this);
	this.entry.connect_changed(move |_| {
		if let Some(this) = weak.upgrade() {
			this.refresh_soon();
		}
	});
	let weak = Rc::downgrade(&this);
	this.all.connect_toggled(move |all| {
		let Some(this) = weak.upgrade() else { return };
		if this.filling.get() {
			return;
		}
		this.filling.set(true);
		for (_, check) in this.shown.borrow().iter() {
			check.set_active(all.is_active());
		}
		this.filling.set(false);
		this.picks_changed();
	});
	let weak = Rc::downgrade(&this);
	this.keep.connect_active_notify(move |_| {
		if let Some(this) = weak.upgrade() {
			this.picks_changed();
		}
	});
	let weak = Rc::downgrade(&this);
	this.confirm.connect_clicked(move |_| {
		if let Some(this) = weak.upgrade() {
			this.move_picked();
		}
	});
	let dialog = this.dialog.clone();
	cancel.connect_clicked(move |_| {
		dialog.close();
	});
	// The dialog keeps its picker alive while open.
	let keep = RefCell::new(Some(this.clone()));
	this.dialog.connect_closed(move |_| {
		keep.borrow_mut().take();
	});
	this.refresh();
	this.dialog.present(Some(&ctx.window));
	this.entry.grab_focus();
	Some(this)
}

impl Picker {
	fn refresh_soon(self: &Rc<Self>) {
		if let Some(id) = self.pending.borrow_mut().take() {
			id.remove();
		}
		let weak = Rc::downgrade(self);
		let id = glib::timeout_add_local_once(Duration::from_millis(150), move || {
			if let Some(this) = weak.upgrade() {
				this.pending.borrow_mut().take();
				this.refresh();
			}
		});
		*self.pending.borrow_mut() = Some(id);
	}

	/// Lists the conversation's mails and what the line catches.
	pub fn refresh(self: &Rc<Self>) {
		let found = match parse(&self.entry.text()) {
			Ok(rule) => {
				self.problem.set_visible(false);
				self.entry.remove_css_class("error");
				self.ctx.cache().and_then(|c| c.matching(&rule, &SKIP, SHOWN).ok())
			},
			Err(e) => {
				self.problem.set_label(&e.message);
				self.problem.set_visible(true);
				self.entry.add_css_class("error");
				None
			},
		};
		let (found, total) = found.unwrap_or_default();
		let own: HashSet<&MailId> = self.own.iter().map(|m| &m.id).collect();
		let mails: Vec<MailSummary> = self.own.iter().cloned().chain(found.into_iter().map(|(m, _)| m).filter(|m| !own.contains(&m.id))).collect();
		self.list.remove_all();
		let (now, tz) = (format::now_ms(), glib::TimeZone::local());
		let mut shown = Vec::new();
		self.filling.set(true);
		for mail in mails {
			let check = gtk::CheckButton::builder().active(self.picked.borrow().contains(&mail.id)).valign(gtk::Align::Center).build();
			let subject = if mail.subject.trim().is_empty() { "(No Subject)".to_string() } else { mail.subject.clone() };
			let row = adw::ActionRow::builder()
				.title(glib::markup_escape_text(mail.from.display()))
				.subtitle(glib::markup_escape_text(&subject))
				.subtitle_lines(1)
				.activatable_widget(&check)
				.build();
			row.add_prefix(&check);
			row.add_suffix(&gtk::Label::builder().label(format::short_date(mail.received_ms, now, &tz)).css_classes(["dim-label", "caption"]).build());
			self.list.append(&row);
			let (weak, id) = (Rc::downgrade(self), mail.id.clone());
			check.connect_toggled(move |check| {
				let Some(this) = weak.upgrade() else { return };
				if this.filling.get() {
					return;
				}
				if check.is_active() {
					this.picked.borrow_mut().insert(id.clone());
				} else {
					this.picked.borrow_mut().remove(&id);
				}
				this.picks_changed();
			});
			shown.push((mail, check));
		}
		self.filling.set(false);
		let more = total.saturating_sub(SHOWN);
		self.count.set_label(&match (total, more) {
			(0, _) => "Nothing else matches".to_string(),
			(1, _) => "1 match".to_string(),
			(n, 0) => format!("{n} matches"),
			(n, _) => format!("Newest {SHOWN} of {n} matches"),
		});
		*self.shown.borrow_mut() = shown;
		self.picks_changed();
	}

	/// For the screenshot scenes: the subjects listed, and whether each is ticked.
	pub fn listed(&self) -> Vec<(String, bool)> {
		self.shown.borrow().iter().map(|(m, c)| (m.subject.clone(), c.is_active())).collect()
	}

	/// The ticked mails on show.
	fn chosen(&self) -> Vec<MailId> {
		self.shown.borrow().iter().filter(|(_, c)| c.is_active()).map(|(m, _)| m.id.clone()).collect()
	}

	/// Keeps Select All and the button in step with the ticks.
	fn picks_changed(&self) {
		let shown = self.shown.borrow();
		let n = shown.iter().filter(|(_, c)| c.is_active()).count();
		for (mail, check) in shown.iter() {
			if check.is_active() {
				self.picked.borrow_mut().insert(mail.id.clone());
			} else {
				self.picked.borrow_mut().remove(&mail.id);
			}
		}
		self.filling.set(true);
		self.all.set_active(n > 0 && n == shown.len());
		self.all.set_inconsistent(n > 0 && n < shown.len());
		self.filling.set(false);
		self.confirm.set_sensitive(n > 0 || self.keep.is_active());
		self.confirm.set_label(&match n {
			0 | 1 => "Move to Spam".to_string(),
			n => format!("Move {n} to Spam"),
		});
	}

	fn move_picked(self: &Rc<Self>) {
		let (Some(backend), Some(cache)) = (self.ctx.backend(), self.ctx.cache()) else { return };
		let ids = self.chosen();
		let line = self.entry.text().trim().to_string();
		let ruled = self.keep.is_active() && parse(&line).is_ok();
		if ruled {
			let mut all = rules::load();
			all.push(Rule { when: line, action: Action::Spam });
			rules::save(&all);
		}
		self.dialog.close();
		if ids.is_empty() {
			return self.ctx.toast("Rule added");
		}
		for id in &ids {
			let _ = cache.move_to(id, &self.spam);
		}
		(self.done)();
		let (ctx, spam, n) = (self.ctx.clone(), self.spam.clone(), ids.len());
		let busy = ctx.activity.start(Word::Moving);
		runtime::spawn_local(async move {
			let result = runtime::run(async move { backend.move_mails(&ids, &spam).await }).await;
			drop(busy);
			match result {
				Ok(()) => ctx.toast(&moved_toast(n, ruled)),
				Err(e) => {
					ctx.error(&e);
					ctx.sync_now();
				},
			}
		});
	}
}

fn moved_toast(n: usize, ruled: bool) -> String {
	let moved = if n == 1 { "Moved to Spam".to_string() } else { format!("Moved {n} mails to Spam") };
	if ruled { format!("{moved}, and added a rule") } else { moved }
}

#[cfg(test)]
mod tests {
	use super::*;

	#[test]
	fn the_toast_counts_what_moved() {
		assert_eq!(moved_toast(1, false), "Moved to Spam");
		assert_eq!(moved_toast(3, true), "Moved 3 mails to Spam, and added a rule");
	}
}

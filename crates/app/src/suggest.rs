//! Address suggestions under the To, Cc and Bcc fields, from contacts and mail sent and received.
//!
//! The word being typed (after the last comma) is looked up as it changes. Up and Down move through
//! the suggestions, Tab or Enter takes one, Escape closes them.

use std::cell::Cell;
use std::rc::Rc;

use adw::prelude::*;
use gtk::{gdk, glib};
use tern_core::Address;

use crate::compose_model::format_address;
use crate::window::Ctx;

const SHOWN: usize = 6;

struct Suggest {
	entry: gtk::Entry,
	popover: gtk::Popover,
	list: gtk::ListBox,
	found: std::cell::RefCell<Vec<Address>>,
	/// Set while the entry's text is changed from here, so that change looks nothing up.
	filling: Cell<bool>,
}

pub fn attach(entry: &gtk::Entry, ctx: &Ctx) {
	let list = gtk::ListBox::builder().selection_mode(gtk::SelectionMode::Single).css_classes(["navigation-sidebar"]).build();
	let popover = gtk::Popover::builder().autohide(false).has_arrow(false).position(gtk::PositionType::Bottom).halign(gtk::Align::Start).child(&list).build();
	popover.set_parent(entry);
	let this = Rc::new(Suggest { entry: entry.clone(), popover: popover.clone(), list: list.clone(), found: Default::default(), filling: Cell::new(false) });

	let (weak, ctx) = (Rc::downgrade(&this), ctx.clone());
	entry.connect_changed(move |_| {
		if let Some(this) = weak.upgrade() {
			if !this.filling.get() {
				this.lookup(&ctx);
			}
		}
	});
	let weak = Rc::downgrade(&this);
	list.connect_row_activated(move |_, row| {
		if let Some(this) = weak.upgrade() {
			this.take(row.index() as usize);
		}
	});
	let keys = gtk::EventControllerKey::new();
	keys.set_propagation_phase(gtk::PropagationPhase::Capture);
	let weak = Rc::downgrade(&this);
	keys.connect_key_pressed(move |_, key, _, _| {
		let Some(this) = weak.upgrade() else { return glib::Propagation::Proceed };
		if !this.popover.is_visible() {
			return glib::Propagation::Proceed;
		}
		let n = this.found.borrow().len() as i32;
		let at = this.list.selected_row().map_or(0, |r| r.index());
		match key {
			gdk::Key::Down => this.select((at + 1).min(n - 1)),
			gdk::Key::Up => this.select((at - 1).max(0)),
			gdk::Key::Tab | gdk::Key::Return | gdk::Key::KP_Enter | gdk::Key::ISO_Enter => this.take(at as usize),
			gdk::Key::Escape => this.close(),
			_ => return glib::Propagation::Proceed,
		}
		glib::Propagation::Stop
	});
	entry.add_controller(keys);
	let focus = gtk::EventControllerFocus::new();
	let weak = Rc::downgrade(&this);
	focus.connect_leave(move |_| {
		if let Some(this) = weak.upgrade() {
			this.close();
		}
	});
	entry.add_controller(focus);
	// The popover is the entry's child; it must go before the entry does. Holding `this` here also
	// keeps the suggestions alive as long as the entry.
	let keep = std::cell::RefCell::new(Some(this));
	entry.connect_destroy(move |_| {
		if let Some(this) = keep.borrow_mut().take() {
			this.popover.unparent();
		}
	});
}

fn popover_of(entry: &gtk::Entry) -> Option<gtk::Popover> {
	let mut child = entry.first_child();
	while let Some(w) = child {
		if let Ok(p) = w.clone().downcast::<gtk::Popover>() {
			return Some(p);
		}
		child = w.next_sibling();
	}
	None
}

/// How many suggestions are open under `entry`.
pub fn open_count(entry: &gtk::Entry) -> usize {
	let Some(p) = popover_of(entry).filter(|p| p.is_visible()) else { return 0 };
	let mut n = 0;
	let mut row = p.child().and_downcast::<gtk::ListBox>().and_then(|l| l.first_child());
	while let Some(r) = row {
		n += 1;
		row = r.next_sibling();
	}
	n
}

/// Closes the suggestions under `entry`, if open. Returns whether they were.
pub fn dismiss(entry: &gtk::Entry) -> bool {
	match popover_of(entry).filter(|p| p.is_visible()) {
		Some(p) => {
			p.popdown();
			true
		},
		None => false,
	}
}

/// The word being typed: what follows the last comma or semicolon.
fn current_word(text: &str) -> (usize, &str) {
	let start = text.rfind([',', ';']).map_or(0, |i| i + 1);
	(start, text[start..].trim_start())
}

impl Suggest {
	fn lookup(&self, ctx: &Ctx) {
		let text = self.entry.text();
		let (start, word) = current_word(&text);
		let Some(cache) = ctx.cache() else { return };
		if word.chars().count() < 1 {
			return self.close();
		}
		let earlier = text[..start].to_lowercase();
		let found: Vec<Address> = cache
			.suggest(word, SHOWN + 4)
			.unwrap_or_default()
			.into_iter()
			// Already in the field.
			.filter(|a| !earlier.contains(&a.address.to_lowercase()))
			.take(SHOWN)
			.collect();
		if found.is_empty() {
			return self.close();
		}
		while let Some(row) = self.list.first_child() {
			self.list.remove(&row);
		}
		for a in &found {
			let name = gtk::Label::builder().xalign(0.0).label(if a.name.is_empty() { &a.address } else { &a.name }).css_classes(["heading"]).build();
			let address = gtk::Label::builder().xalign(0.0).label(&a.address).css_classes(["caption", "dim-label"]).visible(!a.name.is_empty()).build();
			let column = gtk::Box::builder().orientation(gtk::Orientation::Vertical).margin_top(3).margin_bottom(3).build();
			column.append(&name);
			column.append(&address);
			self.list.append(&column);
		}
		*self.found.borrow_mut() = found;
		self.select(0);
		self.popover.set_size_request(self.entry.width().max(240), -1);
		self.popover.popup();
	}

	fn select(&self, i: i32) {
		if let Some(row) = self.list.row_at_index(i) {
			self.list.select_row(Some(&row));
		}
	}

	/// Puts suggestion `i` in place of the word being typed.
	fn take(&self, i: usize) {
		let Some(a) = self.found.borrow().get(i).cloned() else { return };
		let text = self.entry.text().to_string();
		let (start, _) = current_word(&text);
		let before = text[..start].trim_end();
		let joined = if before.is_empty() { format!("{}, ", format_address(&a)) } else { format!("{before} {}, ", format_address(&a)) };
		self.filling.set(true);
		self.entry.set_text(&joined);
		self.entry.set_position(-1);
		self.filling.set(false);
		self.close();
	}

	fn close(&self) {
		self.popover.popdown();
	}
}

#[cfg(test)]
mod tests {
	use super::*;

	#[test]
	fn the_word_being_typed_follows_the_last_separator() {
		assert_eq!(current_word("mi"), (0, "mi"));
		assert_eq!(current_word("Ann <ann@x.org>, sa"), (16, "sa"));
		assert_eq!(current_word("a@x.org; "), (8, ""));
	}
}

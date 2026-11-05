//! Address suggestions under the To, Cc and Bcc fields, as the core finds them.
//!
//! The field's text goes to the core as it changes. Up and Down move through the suggestions, Tab or
//! Enter takes one, Escape closes them.

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use adw::prelude::*;
use gtk::{gdk, glib};
use tern_app::view::SuggestionView;

type TakeHook = Box<dyn Fn(u32)>;

pub struct Suggest {
	popover: gtk::Popover,
	list: gtk::ListBox,
	shown: RefCell<Vec<SuggestionView>>,
	entry: gtk::Entry,
	/// Taking suggestion `i`.
	on_take: RefCell<Option<TakeHook>>,
	/// Closed by the person, so the same suggestions do not come back until new ones do.
	dismissed: Cell<bool>,
}

impl Suggest {
	pub fn new(entry: &gtk::Entry) -> Rc<Suggest> {
		let list = gtk::ListBox::builder().selection_mode(gtk::SelectionMode::Single).css_classes(["navigation-sidebar"]).build();
		let popover = gtk::Popover::builder().autohide(false).has_arrow(false).position(gtk::PositionType::Bottom).halign(gtk::Align::Start).child(&list).build();
		popover.set_parent(entry);
		let this = Rc::new(Suggest { popover, list, shown: RefCell::default(), entry: entry.clone(), on_take: RefCell::default(), dismissed: Cell::new(false) });

		let weak = Rc::downgrade(&this);
		this.list.connect_row_activated(move |_, row| {
			if let Some(this) = weak.upgrade() {
				this.take(row.index());
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
			let n = this.shown.borrow().len() as i32;
			let at = this.list.selected_row().map_or(0, |r| r.index());
			match key {
				gdk::Key::Down => this.select((at + 1).min(n - 1)),
				gdk::Key::Up => this.select((at - 1).max(0)),
				gdk::Key::Tab | gdk::Key::Return | gdk::Key::KP_Enter | gdk::Key::ISO_Enter => this.take(at),
				gdk::Key::Escape => {
					this.dismiss();
				},
				_ => return glib::Propagation::Proceed,
			}
			glib::Propagation::Stop
		});
		entry.add_controller(keys);
		let focus = gtk::EventControllerFocus::new();
		let weak = Rc::downgrade(&this);
		focus.connect_leave(move |_| {
			if let Some(this) = weak.upgrade() {
				this.dismiss();
			}
		});
		entry.add_controller(focus);
		// The popover is the entry's child; it must go before the entry does.
		let popover = this.popover.clone();
		entry.connect_destroy(move |_| popover.unparent());
		this
	}

	pub fn connect_take(&self, f: impl Fn(u32) + 'static) {
		*self.on_take.borrow_mut() = Some(Box::new(f));
	}

	/// Draws what the core has found for this field; none closes them.
	pub fn show(&self, found: &[SuggestionView]) {
		if found.is_empty() {
			self.shown.borrow_mut().clear();
			self.popover.popdown();
			return;
		}
		if *self.shown.borrow() != found {
			self.dismissed.set(false);
			while let Some(row) = self.list.first_child() {
				self.list.remove(&row);
			}
			for a in found {
				let name = gtk::Label::builder().xalign(0.0).label(if a.name.is_empty() { &a.address } else { &a.name }).css_classes(["heading"]).build();
				let address = gtk::Label::builder().xalign(0.0).label(&a.address).css_classes(["caption", "dim-label"]).visible(!a.name.is_empty()).build();
				let column = gtk::Box::builder().orientation(gtk::Orientation::Vertical).margin_top(3).margin_bottom(3).build();
				column.append(&name);
				column.append(&address);
				self.list.append(&column);
			}
			*self.shown.borrow_mut() = found.to_vec();
			self.select(0);
		}
		if !self.dismissed.get() && self.entry.focus_child().is_some() {
			self.popover.set_size_request(self.entry.width().max(240), -1);
			self.popover.popup();
		}
	}

	/// How many suggestions are open.
	pub fn open_count(&self) -> usize {
		if self.popover.is_visible() {
			self.shown.borrow().len()
		} else {
			0
		}
	}

	/// Closes the suggestions, if open. Returns whether they were.
	pub fn dismiss(&self) -> bool {
		let open = self.popover.is_visible();
		self.dismissed.set(true);
		self.popover.popdown();
		open
	}

	fn select(&self, i: i32) {
		if let Some(row) = self.list.row_at_index(i) {
			self.list.select_row(Some(&row));
		}
	}

	fn take(&self, i: i32) {
		self.popover.popdown();
		if let Some(f) = self.on_take.borrow().as_ref() {
			f(i as u32);
		}
	}
}

//! Address suggestions under the To, Cc and Bcc fields, as the core finds them.
//!
//! The field's text goes to the core as it changes. Up and Down move through the suggestions, Tab or
//! Enter takes one, Escape closes them.

use adw::prelude::*;
use gtk::{gdk, glib};
use relm4::factory::FactoryVecDeque;
use relm4::prelude::*;
use tern_app::view::SuggestionView;

/// One suggestion.
struct Row(SuggestionView);

#[relm4::factory]
impl FactoryComponent for Row {
	type Init = SuggestionView;
	type Input = ();
	type Output = ();
	type CommandOutput = ();
	type ParentWidget = gtk::ListBox;

	view! {
		gtk::Box {
			set_orientation: gtk::Orientation::Vertical,
			set_margin_top: 3,
			set_margin_bottom: 3,
			gtk::Label {
				set_xalign: 0.0,
				set_label: if self.0.name.is_empty() { &self.0.address } else { &self.0.name },
				add_css_class: "heading",
			},
			gtk::Label {
				set_xalign: 0.0,
				set_label: &self.0.address,
				set_css_classes: &["caption", "dim-label"],
				set_visible: !self.0.name.is_empty(),
			},
		}
	}

	fn init_model(view: SuggestionView, _: &DynamicIndex, _: FactorySender<Self>) -> Self {
		Row(view)
	}
}

pub struct Suggest {
	entry: gtk::Entry,
	rows: FactoryVecDeque<Row>,
	shown: Vec<SuggestionView>,
	/// Closed by the person, so the same suggestions do not come back until new ones do.
	dismissed: bool,
}

#[derive(Debug)]
pub enum Input {
	/// What the core has found for this field; none closes them.
	Found(Vec<SuggestionView>),
	Dismiss,
}

/// Taking suggestion `i`.
#[derive(Debug)]
pub struct Take(pub u32);

fn select(list: &gtk::ListBox, i: i32) {
	if let Some(row) = list.row_at_index(i) {
		list.select_row(Some(&row));
	}
}

impl Component for Suggest {
	type Init = gtk::Entry;
	type Input = Input;
	type Output = Take;
	type Root = gtk::Popover;
	type Widgets = ();
	type CommandOutput = ();

	fn init_root() -> gtk::Popover {
		gtk::Popover::builder().autohide(false).has_arrow(false).position(gtk::PositionType::Bottom).halign(gtk::Align::Start).build()
	}

	fn init(entry: gtk::Entry, root: gtk::Popover, sender: ComponentSender<Self>) -> ComponentParts<Self> {
		let list = gtk::ListBox::builder().selection_mode(gtk::SelectionMode::Single).css_classes(["navigation-sidebar"]).build();
		root.set_child(Some(&list));
		root.set_parent(&entry);
		let take = {
			let (root, sender) = (root.clone(), sender.clone());
			move |i: i32| {
				root.popdown();
				let _ = sender.output(Take(i as u32));
			}
		};
		let taking = take.clone();
		list.connect_row_activated(move |_, row| taking(row.index()));

		let keys = gtk::EventControllerKey::new();
		keys.set_propagation_phase(gtk::PropagationPhase::Capture);
		let (popover, shown) = (root.clone(), list.clone());
		let dismissing = sender.clone();
		keys.connect_key_pressed(move |_, key, _, _| {
			if !popover.is_visible() {
				return glib::Propagation::Proceed;
			}
			let n = std::iter::successors(shown.first_child(), |w| w.next_sibling()).count() as i32;
			let at = shown.selected_row().map_or(0, |r| r.index());
			match key {
				gdk::Key::Down => select(&shown, (at + 1).min(n - 1)),
				gdk::Key::Up => select(&shown, (at - 1).max(0)),
				gdk::Key::Tab | gdk::Key::Return | gdk::Key::KP_Enter | gdk::Key::ISO_Enter => take(at),
				gdk::Key::Escape => {
					popover.popdown();
					dismissing.input(Input::Dismiss);
				},
				_ => return glib::Propagation::Proceed,
			}
			glib::Propagation::Stop
		});
		entry.add_controller(keys);
		let focus = gtk::EventControllerFocus::new();
		let leaving = sender.clone();
		focus.connect_leave(move |_| leaving.input(Input::Dismiss));
		entry.add_controller(focus);
		// The popover is the entry's child; it must go before the entry does.
		let popover = root.clone();
		entry.connect_destroy(move |_| popover.unparent());

		let rows = FactoryVecDeque::builder().launch(list).detach();
		ComponentParts { model: Suggest { entry, rows, shown: Vec::new(), dismissed: false }, widgets: () }
	}

	fn update(&mut self, input: Input, _: ComponentSender<Self>, root: &gtk::Popover) {
		match input {
			Input::Dismiss => self.dismissed = true,
			Input::Found(found) if found.is_empty() => self.shown.clear(),
			Input::Found(found) => {
				if self.shown != found {
					self.dismissed = false;
					{
						let mut rows = self.rows.guard();
						rows.clear();
						for suggestion in &found {
							rows.push_back(suggestion.clone());
						}
					}
					select(self.rows.widget(), 0);
					self.shown = found;
				}
			},
		}
		self.draw(root);
	}
}

impl Suggest {
	/// Opens or closes the popover to match what the core has said.
	fn draw(&self, root: &gtk::Popover) {
		if self.shown.is_empty() || self.dismissed {
			root.popdown();
		} else if self.entry.focus_child().is_some() {
			root.set_size_request(self.entry.width().max(240), -1);
			root.popup();
		}
	}
}

/// Closes the suggestions, if open. Returns whether they were.
pub fn dismiss(suggest: &Controller<Suggest>) -> bool {
	let open = suggest.widget().is_visible();
	suggest.widget().popdown();
	suggest.emit(Input::Dismiss);
	open
}

/// How many suggestions are open.
pub fn open_count(suggest: &Controller<Suggest>) -> usize {
	if suggest.widget().is_visible() {
		suggest.model().shown.len()
	} else {
		0
	}
}

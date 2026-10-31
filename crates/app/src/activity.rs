//! What Tern is doing right now, in one word beside a spinner: "Fetching…", "Sending…".
//!
//! Each piece of work holds a [`Busy`] for as long as it runs. While several run, the word shown is
//! the one that matters most to the person waiting: their own sending over a sync in the background.

use std::cell::RefCell;
use std::rc::Rc;

use adw::prelude::*;
use gtk::glib;

/// From least to most important.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Word {
	Fetching,
	Opening,
	Downloading,
	Deleting,
	Attaching,
	Drafting,
	Sending,
}

const WORDS: usize = 7;

impl Word {
	pub fn label(self) -> &'static str {
		match self {
			Word::Fetching => "Fetching…",
			Word::Opening => "Opening…",
			Word::Downloading => "Downloading…",
			Word::Deleting => "Deleting…",
			Word::Attaching => "Attaching…",
			Word::Drafting => "Drafting…",
			Word::Sending => "Sending…",
		}
	}
}

#[derive(Default)]
struct State {
	running: [usize; WORDS],
	shown: Vec<(glib::WeakRef<gtk::Box>, glib::WeakRef<gtk::Label>)>,
}

impl State {
	fn word(&self) -> Option<Word> {
		use Word::*;
		[Sending, Drafting, Attaching, Deleting, Downloading, Opening, Fetching].into_iter().find(|w| self.running[*w as usize] > 0)
	}

	fn update(&mut self) {
		let word = self.word();
		self.shown.retain(|(row, label)| {
			let (Some(row), Some(label)) = (row.upgrade(), label.upgrade()) else { return false };
			row.set_visible(word.is_some());
			if let Some(w) = word {
				label.set_label(w.label());
			}
			true
		});
	}
}

#[derive(Clone, Default)]
pub struct Activity(Rc<RefCell<State>>);

/// Work under way; it ends when dropped.
pub struct Busy {
	activity: Activity,
	word: Word,
}

impl Drop for Busy {
	fn drop(&mut self) {
		let mut state = self.activity.0.borrow_mut();
		state.running[self.word as usize] -= 1;
		state.update();
	}
}

impl Activity {
	pub fn start(&self, word: Word) -> Busy {
		let mut state = self.0.borrow_mut();
		state.running[word as usize] += 1;
		state.update();
		Busy { activity: self.clone(), word }
	}

	/// The word on show now, if anything is under way.
	pub fn word(&self) -> Option<Word> {
		self.0.borrow().word()
	}

	/// A spinner with the word, hidden while nothing is under way.
	pub fn indicator(&self) -> gtk::Box {
		let row = gtk::Box::builder().spacing(6).build();
		let label = gtk::Label::builder().css_classes(["dim-label"]).ellipsize(gtk::pango::EllipsizeMode::End).build();
		row.append(&adw::Spinner::new());
		row.append(&label);
		let mut state = self.0.borrow_mut();
		state.shown.push((row.downgrade(), label.downgrade()));
		state.update();
		row
	}
}

#[cfg(test)]
mod tests {
	use super::*;

	#[test]
	fn the_most_important_work_names_the_wait() {
		let activity = Activity::default();
		assert_eq!(activity.word(), None);
		let sync = activity.start(Word::Fetching);
		assert_eq!(activity.word(), Some(Word::Fetching));
		let send = activity.start(Word::Sending);
		let open = activity.start(Word::Opening);
		assert_eq!(activity.word(), Some(Word::Sending));
		drop(send);
		assert_eq!(activity.word(), Some(Word::Opening));
		// Two of a kind: the word stays until both end.
		let again = activity.start(Word::Opening);
		drop(open);
		assert_eq!(activity.word(), Some(Word::Opening));
		drop(again);
		drop(sync);
		assert_eq!(activity.word(), None);
	}
}

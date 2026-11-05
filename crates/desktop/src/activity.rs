//! What Tern is doing right now, in one word beside a spinner: "Fetching…", "Sending…".
//!
//! Each piece of work holds a [`Busy`] for as long as it runs. While several run, the word shown is
//! the one that matters most to the person waiting: their own sending over a sync in the background.
//! With nothing under way, a problem with the connection, if there is one, stands in its place.

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
	Moving,
	Deleting,
	Attaching,
	Drafting,
	Sending,
}

const WORDS: usize = 8;

impl Word {
	pub fn label(self) -> &'static str {
		match self {
			Word::Fetching => "Fetching…",
			Word::Opening => "Opening…",
			Word::Downloading => "Downloading…",
			Word::Moving => "Moving…",
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
	problem: Option<String>,
	shown: Vec<(glib::WeakRef<gtk::Box>, glib::WeakRef<gtk::Label>, glib::WeakRef<adw::Spinner>)>,
}

impl State {
	fn word(&self) -> Option<Word> {
		use Word::*;
		[Sending, Drafting, Attaching, Deleting, Moving, Downloading, Opening, Fetching].into_iter().find(|w| self.running[*w as usize] > 0)
	}

	fn update(&mut self) {
		let word = self.word();
		let text = word.map(Word::label).or(self.problem.as_deref());
		self.shown.retain(|(row, label, spinner)| {
			let (Some(row), Some(label), Some(spinner)) = (row.upgrade(), label.upgrade(), spinner.upgrade()) else { return false };
			row.set_visible(text.is_some());
			spinner.set_visible(word.is_some());
			if let Some(text) = text {
				label.set_label(text);
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

	/// Says what is wrong with the connection, or that nothing is.
	pub fn set_problem(&self, problem: Option<String>) {
		let mut state = self.0.borrow_mut();
		state.problem = problem;
		state.update();
	}

	/// The problem with the connection last told, if any.
	pub fn problem(&self) -> Option<String> {
		self.0.borrow().problem.clone()
	}

	/// The word on show now, if anything is under way.
	pub fn word(&self) -> Option<Word> {
		self.0.borrow().word()
	}

	/// A spinner with the word, hidden while nothing is under way.
	pub fn indicator(&self) -> gtk::Box {
		let row = gtk::Box::builder().spacing(6).build();
		let label = gtk::Label::builder().css_classes(["dim-label"]).ellipsize(gtk::pango::EllipsizeMode::End).build();
		let spinner = adw::Spinner::new();
		row.append(&spinner);
		row.append(&label);
		let mut state = self.0.borrow_mut();
		state.shown.push((row.downgrade(), label.downgrade(), spinner.downgrade()));
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

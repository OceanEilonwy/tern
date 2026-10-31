//! Which mail bodies to fetch next, and in what batches. Pure: the fetcher asks, then does the work.
//!
//! Opening a mail costs several round trips; a batch costs about the same as one mail (one request
//! per mail list, per body archive and per file list), so mail is fetched in batches. What is waited
//! for comes first and alone, so it paints quickly; the rest rides in batches.

use std::collections::HashSet;

use tern_core::MailId;

/// The most mails in one batch: Tuta takes at most 100 ids per request.
pub const MAX_BATCH: usize = 100;

/// Why mail is wanted, from least to most urgent.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Priority {
	/// Opened ahead by the sync, so it shows without waiting later.
	Background,
	/// On screen in the list, likely to be clicked.
	Viewing,
	/// In the conversation being read.
	Reading,
}

/// What the window wants fetched.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Want {
	/// A conversation opened for reading, oldest first, with the mail chosen in the list.
	Reading { thread: Vec<MailId>, focus: MailId },
	/// The newest mail of each conversation on screen.
	Viewing(Vec<MailId>),
	/// Mails to open ahead, newest first.
	Background(Vec<MailId>),
}

/// Mails to fetch in one request, and why.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Batch {
	pub priority: Priority,
	pub ids: Vec<MailId>,
}

/// Background batch sizes: the first small, so a foreground request that queues behind it waits
/// little and the first bodies land soon; then larger, as fewer requests mean fewer round trips.
/// The last size repeats.
pub const BACKGROUND_SIZES: [usize; 4] = [10, 25, 50, MAX_BATCH];

/// The batches for `want`, most urgent first, leaving out mails `have` says are stored.
pub fn batches(want: Want, have: impl Fn(&MailId) -> bool) -> Vec<Batch> {
	let mut seen = HashSet::new();
	let mut fresh = |ids: Vec<MailId>| -> Vec<MailId> { ids.into_iter().filter(|id| !have(id) && seen.insert(id.clone())).collect() };
	let batch = |priority, ids: &[MailId]| Batch { priority, ids: ids.to_vec() };
	match want {
		Want::Reading { thread, focus } => {
			// The focused mail alone, then the rest of the conversation newest first.
			let first = fresh(vec![focus]);
			let rest = fresh(thread.into_iter().rev().collect());
			let mut out: Vec<Batch> = first.chunks(1).map(|ids| batch(Priority::Reading, ids)).collect();
			out.extend(rest.chunks(MAX_BATCH).map(|ids| batch(Priority::Reading, ids)));
			out
		},
		Want::Viewing(ids) => fresh(ids).chunks(MAX_BATCH).map(|ids| batch(Priority::Viewing, ids)).collect(),
		Want::Background(ids) => {
			let ids = fresh(ids);
			let mut out = Vec::new();
			let mut rest = &ids[..];
			for n in 0.. {
				if rest.is_empty() {
					break;
				}
				let size = BACKGROUND_SIZES[n.min(BACKGROUND_SIZES.len() - 1)].min(rest.len());
				out.push(batch(Priority::Background, &rest[..size]));
				rest = &rest[size..];
			}
			out
		},
	}
}

#[cfg(test)]
mod tests {
	use super::*;

	fn id(n: usize) -> MailId {
		MailId { list: "l".into(), element: format!("m{n}") }
	}

	fn ids(range: impl IntoIterator<Item = usize>) -> Vec<MailId> {
		range.into_iter().map(id).collect()
	}

	#[test]
	fn reading_fetches_the_focus_alone_then_the_rest_newest_first() {
		// Oldest first: m0 … m9; the one clicked is m4.
		let got = batches(Want::Reading { thread: ids(0..10), focus: id(4) }, |_| false);
		assert_eq!(got.len(), 2);
		assert_eq!(got[0], Batch { priority: Priority::Reading, ids: vec![id(4)] });
		assert_eq!(got[1].ids, ids([9, 8, 7, 6, 5, 3, 2, 1, 0]));
	}

	#[test]
	fn stored_mail_is_left_out() {
		let have = |m: &MailId| m == &id(4) || m == &id(9);
		let got = batches(Want::Reading { thread: ids(0..10), focus: id(4) }, have);
		assert_eq!(got.len(), 1, "the focus is stored, so no batch of one");
		assert_eq!(got[0].ids, ids([8, 7, 6, 5, 3, 2, 1, 0]));
		assert!(batches(Want::Viewing(ids(0..3)), |_| true).is_empty());
		assert!(batches(Want::Reading { thread: vec![id(1)], focus: id(1) }, |_| true).is_empty());
	}

	#[test]
	fn viewing_is_one_batch_without_repeats() {
		let got = batches(Want::Viewing(vec![id(1), id(2), id(1), id(3)]), |_| false);
		assert_eq!(got, vec![Batch { priority: Priority::Viewing, ids: ids(1..4) }]);
		let many = batches(Want::Viewing(ids(0..150)), |_| false);
		assert_eq!(many.iter().map(|b| b.ids.len()).collect::<Vec<_>>(), [MAX_BATCH, 50]);
	}

	#[test]
	fn background_batches_grow_and_keep_the_order() {
		let got = batches(Want::Background(ids(0..300)), |_| false);
		assert_eq!(got.iter().map(|b| b.ids.len()).collect::<Vec<_>>(), [10, 25, 50, 100, 100, 15]);
		assert!(got.iter().all(|b| b.priority == Priority::Background && b.ids.len() <= MAX_BATCH));
		assert_eq!(got.into_iter().flat_map(|b| b.ids).collect::<Vec<_>>(), ids(0..300), "newest first, as given");
		assert_eq!(batches(Want::Background(ids(0..4)), |_| false).iter().map(|b| b.ids.len()).collect::<Vec<_>>(), [4]);
		assert!(batches(Want::Background(vec![]), |_| false).is_empty());
	}
}

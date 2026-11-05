//! Clean Up: how much Tern takes on disk, whether to offer cleaning up, and freeing the room.
//!
//! Mail past Keep Ready is trimmed after every sync anyway (see `Cache::trim`); this is for what
//! is left: the text of older mail opened, meanings worked out for search, the language model, or
//! everything at once.

use tern_cache::{sync::Level, Cache};

use crate::semantic;

/// What Tern takes on disk, in bytes, and what each choice would free.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Usage {
	/// The cache with its log, and the language model.
	pub total: u64,
	/// Mail past Keep Ready, the text of older mail opened, and room already free in the file.
	pub older: u64,
	/// The mails' meanings for search; 0 when search by meaning is off, as they are gone then.
	pub vectors: u64,
	/// The language model, counted only while search by meaning is off: in use, it would only be
	/// downloaded again.
	pub model: u64,
}

/// What to clear.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Pick {
	pub older: bool,
	pub vectors: bool,
	pub model: bool,
	pub all: bool,
}

/// With the disk this full (free below 1/20 of it), Tern offers to clean up whatever the setting
/// says, once it takes at least `LOW_DISK_MIN`.
const LOW_DISK_SHARE: u64 = 20;
const LOW_DISK_MIN: u64 = 100_000_000;

/// Whether to offer cleaning up: Tern takes more than `limit_mb` megabytes (0 for never), or the
/// disk is nearly full and Tern takes enough to matter. `disk` is (free, size) in bytes.
pub fn offer(total: u64, limit_mb: u32, disk: Option<(u64, u64)>) -> bool {
	let past = limit_mb > 0 && total > u64::from(limit_mb) * 1_000_000;
	let low = disk.is_some_and(|(free, size)| size > 0 && free * LOW_DISK_SHARE < size);
	past || (low && total >= LOW_DISK_MIN)
}

/// Measures what Tern takes now. Reads the disk, so it does not belong on a UI thread.
pub fn measure(cache: &Cache, level: Level, meaning: bool) -> Usage {
	let model = semantic::model_bytes();
	Usage {
		total: cache.disk_bytes() + model,
		older: cache.older_bytes(level.headers(), level.bodies()).unwrap_or(0),
		vectors: cache.vector_bytes().unwrap_or(0),
		model: if meaning { 0 } else { model },
	}
}

/// Clears what was picked and gives the room back to the disk. Returns the bytes freed. Reads and
/// writes the disk, so it does not belong on a UI thread.
pub fn clean(cache: &Cache, level: Level, pick: Pick) -> u64 {
	let before = cache.disk_bytes();
	let done = if pick.all {
		cache.clear_all()
	} else {
		(if pick.older { cache.clear_older(level.headers(), level.bodies()) } else { Ok(()) }).and_then(|()| if pick.vectors { cache.clear_vectors() } else { Ok(()) })
	};
	if let Err(e) = done.and_then(|()| cache.compact()) {
		eprintln!("tern: could not clean up: {e}");
	}
	let freed = before.saturating_sub(cache.disk_bytes());
	let model = if pick.model {
		let bytes = semantic::model_bytes();
		semantic::remove_model();
		bytes
	} else {
		0
	};
	freed + model
}

#[cfg(test)]
mod tests {
	use super::*;

	#[test]
	fn the_offer_shows_past_the_limit_or_on_a_nearly_full_disk() {
		assert!(!offer(500_000_000, 1000, None));
		assert!(offer(1_500_000_000, 1000, None));
		assert!(!offer(5_000_000_000, 0, None), "0 is never");
		let (roomy, full) = ((500_000_000_000, 1_000_000_000_000), (10_000_000_000, 1_000_000_000_000));
		assert!(!offer(500_000_000, 1000, Some(roomy)));
		assert!(offer(500_000_000, 1000, Some(full)), "whatever the setting says");
		assert!(offer(500_000_000, 0, Some(full)));
		assert!(!offer(50_000_000, 1000, Some(full)), "too little to matter");
	}
}

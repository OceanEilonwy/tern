//! Bringing the cache up to date from the server, in the background.

use tern_core::{Backend, Error, Folder, FolderId, Result};

use crate::embed::Model;
use crate::{now_ms, Cache, CacheError, FolderState};

/// How much mail to keep ready offline, a setting.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Level {
	Low,
	#[default]
	Medium,
	High,
}

impl Level {
	pub const ALL: [Level; 3] = [Level::Low, Level::Medium, Level::High];

	/// Mails per folder whose list rows are fetched.
	pub fn headers(self) -> usize {
		match self {
			Level::Low => 100,
			Level::Medium => 500,
			Level::High => 2000,
		}
	}

	/// Newest mails, across folders, opened ahead so they show without waiting.
	pub fn bodies(self) -> usize {
		match self {
			Level::Low => 25,
			Level::Medium => 200,
			Level::High => 1000,
		}
	}

	pub fn name(self) -> &'static str {
		match self {
			Level::Low => "low",
			Level::Medium => "medium",
			Level::High => "high",
		}
	}

	pub fn from_name(name: &str) -> Option<Level> {
		Level::ALL.into_iter().find(|l| l.name() == name)
	}
}

/// Mails per request when paging through a folder.
pub const PAGE: usize = 50;

fn cache_error(e: CacheError) -> Error {
	Error::Other(e.to_string())
}

/// Fetches the folder list and stores it. Returns the folders.
pub async fn folders(backend: &dyn Backend, cache: &Cache) -> Result<Vec<Folder>> {
	let folders = backend.folders().await?;
	cache.set_folders(&folders).map_err(cache_error)?;
	cache.folders().map_err(cache_error)
}

/// Brings the newest mails of `folder` up to date: pages from the newest down until a page brings
/// nothing new (after the first sync) or `keep` mails were fetched. Returns whether anything changed.
pub async fn folder(backend: &dyn Backend, cache: &Cache, folder: &FolderId, keep: usize) -> Result<bool> {
	let mut state = cache.folder_state(folder).map_err(cache_error)?;
	let synced_before = state.synced_ms > 0;
	let stored = cache.count(folder).map_err(cache_error)?;
	let (mut next, mut fetched, mut changed, mut first) = (None, 0, false, true);
	loop {
		let page = backend.list(folder, next.clone(), PAGE).await?;
		let last = page.next.is_none();
		let changes = cache.merge_page(folder, &page.mails, first, last).map_err(cache_error)?;
		remember_cursor(cache, &page)?;
		fetched += page.mails.len();
		changed |= changes > 0;
		// The deepest point reached so far is where "load more" continues.
		if fetched >= stored || state.next.is_none() {
			state.next = page.next.clone();
			state.complete = last;
		}
		if last || fetched >= keep || (synced_before && changes == 0) {
			break;
		}
		next = page.next;
		first = false;
	}
	state.synced_ms = now_ms();
	cache.set_folder_state(folder, &state).map_err(cache_error)?;
	Ok(changed)
}

/// The cursor after a page's last mail, so mail trimmed past it can be fetched again from there.
fn remember_cursor(cache: &Cache, page: &tern_core::MailPage) -> Result<()> {
	match (page.mails.last(), &page.next) {
		(Some(mail), Some(next)) => cache.set_cursor(&mail.id, next).map_err(cache_error),
		_ => Ok(()),
	}
}

/// Pages fetched at most for one "load more", passing those already stored.
const OLDER_PAGES: usize = 100;

/// Fetches older mail of `folder`, beyond what is stored: pages on until one brings mail not
/// stored yet, as after a trim that could not cut at a page's edge. Returns whether the folder
/// has more.
pub async fn older(backend: &dyn Backend, cache: &Cache, folder: &FolderId) -> Result<bool> {
	let mut state = cache.folder_state(folder).map_err(cache_error)?;
	if state.complete {
		return Ok(false);
	}
	let before = cache.count(folder).map_err(cache_error)?;
	if state.next.is_none() {
		// Never synced, or trimmed back to the start: the first page comes first.
		self::folder(backend, cache, folder, PAGE).await?;
		state = cache.folder_state(folder).map_err(cache_error)?;
	}
	for _ in 0..OLDER_PAGES {
		if state.complete || cache.count(folder).map_err(cache_error)? > before {
			break;
		}
		let page = backend.list(folder, state.next.clone(), PAGE).await?;
		let last = page.next.is_none();
		cache.merge_page(folder, &page.mails, false, last).map_err(cache_error)?;
		remember_cursor(cache, &page)?;
		state = FolderState { next: page.next, complete: last, ..state };
		cache.set_folder_state(folder, &state).map_err(cache_error)?;
	}
	Ok(!state.complete)
}

/// Mails whose meaning is worked out at a time.
pub const VECTOR_BATCH: usize = 100;

/// Works out the meaning of every stored mail that lacks it with `model`, stored under `name`.
/// Returns how many. Runs on the CPU for about a millisecond per hundred mails; call it off the UI thread.
pub fn vectors(cache: &Cache, model: &Model, name: &str) -> std::result::Result<usize, CacheError> {
	let mut done = 0;
	loop {
		let batch: Vec<(i64, String, Vec<f32>)> = cache
			.unembedded(name, VECTOR_BATCH)?
			.into_iter()
			.map(|(id, text)| {
				let vector = model.embed(&text);
				(id, text, vector)
			})
			.collect();
		let stored = cache.put_vectors(name, &batch)?;
		done += stored;
		// Mails that changed meanwhile wait for the next round rather than going round here.
		if batch.len() < VECTOR_BATCH || stored == 0 {
			return Ok(done);
		}
	}
}

/// Stores the account's contacts for suggestions.
pub async fn contacts(backend: &dyn Backend, cache: &Cache) -> Result<()> {
	let contacts = backend.contacts().await?;
	cache.note_addresses(&contacts, crate::Met::Contact).map_err(cache_error)
}

#[cfg(test)]
mod tests {
	use super::*;
	use tern_core::demo::DemoBackend;
	use tern_core::{Address, Draft, FolderKind};

	#[tokio::test]
	async fn a_full_sync_fills_the_cache_and_later_ones_pick_up_changes() {
		let demo = DemoBackend::new();
		let cache = Cache::in_memory();
		let all = folders(&demo, &cache).await.unwrap();
		let inbox = all.iter().find(|f| f.kind == FolderKind::Inbox).unwrap().id.clone();
		let sent = all.iter().find(|f| f.kind == FolderKind::Sent).unwrap().id.clone();
		for f in &all {
			folder(&demo, &cache, &f.id, Level::Low.headers()).await.unwrap();
		}
		assert_eq!(cache.count(&inbox).unwrap(), 7);
		assert!(cache.folder_state(&inbox).unwrap().complete);
		assert_eq!(cache.folders().unwrap().iter().find(|f| f.id == inbox).unwrap().unread, 2);

		// Nothing new: nothing changes.
		assert!(!folder(&demo, &cache, &inbox, 100).await.unwrap());
		// A sent mail and a read mail show up.
		demo.send(Draft { to: vec![Address::new("", "mia@tuta.com")], subject: "New".into(), ..Default::default() }).await.unwrap();
		let first = cache.list(&inbox, 0, 1).unwrap().remove(0);
		demo.set_unread(&first.id, false).await.unwrap();
		assert!(folder(&demo, &cache, &sent, 100).await.unwrap());
		assert!(folder(&demo, &cache, &inbox, 100).await.unwrap());
		assert_eq!(cache.count(&sent).unwrap(), 2);
		assert_eq!(cache.folders().unwrap().iter().find(|f| f.id == inbox).unwrap().unread, 1);

	}

	#[tokio::test]
	async fn older_pages_continue_where_the_sync_stopped() {
		let demo = DemoBackend::new();
		let cache = Cache::in_memory();
		let all = folders(&demo, &cache).await.unwrap();
		let inbox = all.iter().find(|f| f.kind == FolderKind::Inbox).unwrap().id.clone();
		// Keep only one page worth (the demo inbox has 7; pages are 50, so fake a small keep).
		folder(&demo, &cache, &inbox, 1).await.unwrap();
		assert_eq!(cache.count(&inbox).unwrap(), 7, "a page is the smallest step");
		assert!(!older(&demo, &cache, &inbox).await.unwrap(), "that page was the whole folder");
	}

	/// The demo's Sent folder with `n` more mails, synced to its newest page.
	async fn many_sent(n: usize) -> (DemoBackend, Cache, FolderId) {
		let demo = DemoBackend::new();
		for i in 0..n {
			demo.send(Draft { to: vec![Address::new("", "mia@tuta.com")], subject: format!("Sent {i}"), ..Default::default() }).await.unwrap();
		}
		let cache = Cache::in_memory();
		let all = folders(&demo, &cache).await.unwrap();
		let sent = all.iter().find(|f| f.kind == FolderKind::Sent).unwrap().id.clone();
		folder(&demo, &cache, &sent, PAGE).await.unwrap();
		(demo, cache, sent)
	}

	#[tokio::test]
	async fn after_a_trim_older_mail_comes_back_where_it_was_cut() {
		let (demo, cache, sent) = many_sent(130).await;
		assert!(older(&demo, &cache, &sent).await.unwrap());
		assert!(!older(&demo, &cache, &sent).await.unwrap(), "50, 50 and the last 31");
		assert_eq!(cache.count(&sent).unwrap(), 131);
		// Trimmed back to the first page's edge, the next "load more" brings the second page again.
		assert_eq!(cache.trim(&sent, PAGE).unwrap(), 81);
		assert!(older(&demo, &cache, &sent).await.unwrap());
		assert_eq!(cache.count(&sent).unwrap(), 2 * PAGE);
	}

	#[tokio::test]
	async fn without_a_cursor_older_passes_the_pages_already_stored() {
		let (demo, cache, sent) = many_sent(130).await;
		older(&demo, &cache, &sent).await.unwrap();
		older(&demo, &cache, &sent).await.unwrap();
		cache.conn().execute("DELETE FROM cursors", []).unwrap();
		cache.trim(&sent, 2 * PAGE).unwrap();
		assert_eq!(cache.folder_state(&sent).unwrap().next, None);
		// A sync sets the cursor after its first page, which is already stored.
		folder(&demo, &cache, &sent, PAGE).await.unwrap();
		assert!(!older(&demo, &cache, &sent).await.unwrap(), "the third page was the last");
		assert_eq!(cache.count(&sent).unwrap(), 131);
	}

	#[tokio::test]
	async fn contacts_become_suggestions() {
		let demo = DemoBackend::new();
		let cache = Cache::in_memory();
		contacts(&demo, &cache).await.unwrap();
		assert_eq!(cache.suggest("pri", 5).unwrap(), vec![Address::new("Priya Natarajan", "priya@example.org")]);
	}

	#[test]
	fn levels_round_trip_by_name() {
		for level in Level::ALL {
			assert_eq!(Level::from_name(level.name()), Some(level));
		}
		assert!(Level::Low.headers() < Level::Medium.headers() && Level::Medium.bodies() < Level::High.bodies());
	}
}

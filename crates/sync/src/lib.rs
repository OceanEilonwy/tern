//! Bringing the cache up to date from the server, in the background: which mail, in what order, and
//! opening it ahead. Stored by `tern-cache`; nothing here knows which server it is, or what is on screen.

use tern_mail::{Backend, Error, Folder, FolderId, Result};

pub mod fetcher;
pub mod policy;
pub mod round;
mod rules;

pub use round::{round, Event, Finished, Job, Stops};
pub use rules::run_on_new;
use tern_cache::{now_ms, Cache, CacheError, FolderState, PAGE};

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
	folder_by_page(backend, cache, folder, keep, PAGE, &|| {}).await
}

/// [`folder`], calling `on_change` after each page that changed something, so that new mail shows
/// when it arrives and not after the last page of a first sync. The first page holds `first_page`
/// mails: fewer than a full page reach the screen sooner, and the next page makes up the rest.
pub async fn folder_by_page(
	backend: &dyn Backend,
	cache: &Cache,
	folder: &FolderId,
	keep: usize,
	first_page: usize,
	on_change: &(dyn Fn() + Send + Sync),
) -> Result<bool> {
	let mut state = cache.folder_state(folder).map_err(cache_error)?;
	let synced_before = state.synced_ms > 0;
	let stored = cache.count(folder).map_err(cache_error)?;
	let (mut next, mut fetched, mut changed, mut first) = (None, 0, false, true);
	loop {
		let count = if first { first_page } else if fetched < PAGE { PAGE - fetched } else { PAGE };
		let page = backend.list(folder, next.clone(), count).await?;
		let last = page.next.is_none();
		let changes = cache.merge_page(folder, &page.mails, first, last).map_err(cache_error)?;
		remember_cursor(cache, &page)?;
		fetched += page.mails.len();
		changed |= changes > 0;
		if changes > 0 {
			on_change();
		}
		// The deepest point reached so far is where "load more" continues.
		if fetched >= stored || state.next.is_none() {
			state.next = page.next.clone();
			state.complete = last;
		}
		// A short first page says nothing of the mails a full one would have checked.
		if last || fetched >= keep || (synced_before && changes == 0 && fetched >= PAGE) {
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
fn remember_cursor(cache: &Cache, page: &tern_mail::MailPage) -> Result<()> {
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

/// Stores the account's contacts for suggestions.
pub async fn contacts(backend: &dyn Backend, cache: &Cache) -> Result<()> {
	let contacts = backend.contacts().await?;
	cache.note_addresses(&contacts, tern_cache::Met::Contact).map_err(cache_error)
}

/// How long contacts fetched count as fresh. They only feed suggestions, and can be large.
const CONTACTS_FRESH_MS: i64 = 24 * 60 * 60 * 1000;
const CONTACTS_MARK: &str = "contacts";

/// [`contacts`], unless they were fetched within the last day.
pub async fn contacts_if_stale(backend: &dyn Backend, cache: &Cache) -> Result<()> {
	let fetched = cache.mark(CONTACTS_MARK).map_err(cache_error)?;
	if fetched.is_some_and(|ms| now_ms() - ms < CONTACTS_FRESH_MS) {
		return Ok(());
	}
	contacts(backend, cache).await?;
	cache.set_mark(CONTACTS_MARK, now_ms()).map_err(cache_error)
}

#[cfg(test)]
mod tests {
	use super::*;
	use tern_demo::DemoBackend;
	use tern_mail::{Address, Draft, FolderKind};

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
		cache.clear_cursors().unwrap();
		cache.trim(&sent, 2 * PAGE).unwrap();
		assert_eq!(cache.folder_state(&sent).unwrap().next, None);
		// A sync sets the cursor after its first page, which is already stored.
		folder(&demo, &cache, &sent, PAGE).await.unwrap();
		assert!(!older(&demo, &cache, &sent).await.unwrap(), "the third page was the last");
		assert_eq!(cache.count(&sent).unwrap(), 131);
	}

	#[tokio::test]
	async fn each_page_of_a_first_sync_is_told_as_it_lands() {
		let (demo, _, sent) = many_sent(130).await;
		let cache = Cache::in_memory();
		folders(&demo, &cache).await.unwrap();
		let shown = std::sync::Mutex::new(Vec::new());
		folder_by_page(&demo, &cache, &sent, 500, PAGE, &|| shown.lock().unwrap().push(cache.count(&sent).unwrap())).await.unwrap();
		assert_eq!(*shown.lock().unwrap(), vec![50, 100, 131]);
	}

	#[tokio::test]
	async fn a_short_first_page_still_checks_a_full_page() {
		let (demo, _, sent) = many_sent(130).await;
		let cache = Cache::in_memory();
		folders(&demo, &cache).await.unwrap();
		folder(&demo, &cache, &sent, PAGE).await.unwrap();
		// Read elsewhere, past the short page but within a full one.
		let far = cache.list(&sent, 30, 1).unwrap().remove(0);
		demo.set_unread(&far.id, !far.unread).await.unwrap();
		assert!(folder_by_page(&demo, &cache, &sent, PAGE, 20, &|| {}).await.unwrap(), "a change on the second page was found");
		assert_eq!(cache.list(&sent, 30, 1).unwrap().remove(0).unread, !far.unread);
		assert!(!folder_by_page(&demo, &cache, &sent, PAGE, 20, &|| {}).await.unwrap(), "and nothing the next time");
	}

	#[tokio::test]
	async fn contacts_are_fetched_once_a_day() {
		let (demo, cache) = (DemoBackend::new(), Cache::in_memory());
		contacts_if_stale(&demo, &cache).await.unwrap();
		let fetched = cache.mark(CONTACTS_MARK).unwrap().unwrap();
		contacts_if_stale(&demo, &cache).await.unwrap();
		assert_eq!(cache.mark(CONTACTS_MARK).unwrap(), Some(fetched), "fresh: not fetched again");
		cache.set_mark(CONTACTS_MARK, fetched - CONTACTS_FRESH_MS).unwrap();
		contacts_if_stale(&demo, &cache).await.unwrap();
		assert!(cache.mark(CONTACTS_MARK).unwrap().unwrap() >= fetched, "a day on: fetched again");
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

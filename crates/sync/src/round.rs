//! One round of keeping the cache up to date, the same on every platform, what a person is looking
//! at first: the Inbox, its newest mails before the rest of its page; the folder list; the other
//! folders a few at a time; the rules on new mail in the Inbox; the newest mails opened ahead; and
//! contacts, at most daily. Each folder is trimmed back to what is kept after its sync, except the
//! one on screen, whose older mail may be scrolled to, and Drafts.
//!
//! Nearly all of a round's time is waiting on the server, a round trip for each request.
//!
//! What it finds out goes to the caller as it happens, as [`Event`]s, for whatever is on screen. When
//! rounds run is the caller's to say.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use futures::stream::{FuturesUnordered, StreamExt};
use tern_cache::Cache;
use tern_mail::{Backend, Error, Folder, FolderId, FolderKind, Label, Result};
use tern_rules::Rule;
use tokio::task::AbortHandle;

use crate::fetcher::Fetcher;
use crate::Level;

/// A task that ends when this is dropped: work that must not outlive what started it.
pub struct Stops(pub AbortHandle);

impl Drop for Stops {
	fn drop(&mut self) {
		self.0.abort();
	}
}

/// What a round works from.
pub struct Job {
	pub backend: Arc<dyn Backend>,
	pub cache: Arc<Cache>,
	pub fetcher: Fetcher,
	pub level: Level,
	pub rules: Vec<Rule>,
	/// The folder on screen as of now. Asked again for each folder, as it may change meanwhile.
	pub on_screen: Arc<dyn Fn() -> Option<FolderId> + Send + Sync>,
	/// Whether the caller shows labels, and wants them fetched.
	pub labels: bool,
	/// Contacts are fetched until this is set, once per session.
	pub contacts_done: Arc<AtomicBool>,
}

#[derive(Debug, PartialEq)]
pub enum Event {
	/// The server answered, with the folder list.
	Folders(Vec<Folder>),
	Labels(Vec<Label>),
	/// This folder's list changed.
	FolderChanged(FolderId),
	/// The rules moved mail, so any folder may have changed.
	RulesMoved,
	/// Something about the connection, or a failure opening mail ahead.
	Problem(Error),
	/// The folder list itself could not be fetched.
	NoFolders(Error),
}

/// How a round ended.
#[derive(Debug, PartialEq)]
pub struct Finished {
	/// The server gave the folder list, so there is mail worth more work.
	pub listed: bool,
	/// An error here means the round stopped short, and is worth trying again soon.
	pub result: Result<()>,
}

/// Folders listed at once, the same as the fetcher's limit on requests.
const FOLDERS_AT_ONCE: usize = 3;

/// Mails in the first page of what is on screen: about a window's worth. On a new connection a full
/// page takes three round trips as TCP ramps up, this one two.
const FIRST_ON_SCREEN: usize = 20;

/// An error that says more folders would fail the same way, or that asking again makes things worse.
fn stops_the_round(e: &Error) -> bool {
	matches!(e, Error::Offline | Error::SessionExpired | Error::RateLimited(_))
}

/// Syncs one folder and trims it, telling what changed as it goes.
async fn sync_folder(job: &Job, folder: &Folder, on: &(dyn Fn(Event) + Send + Sync)) -> Result<()> {
	let (backend, cache) = (job.backend.as_ref(), job.cache.as_ref());
	let seen_first = folder.kind == FolderKind::Inbox || (job.on_screen)().as_ref() == Some(&folder.id);
	let first_page = if seen_first { FIRST_ON_SCREEN } else { tern_cache::PAGE };
	crate::folder_by_page(backend, cache, &folder.id, job.level.headers(), first_page, &|| on(Event::FolderChanged(folder.id.clone()))).await?;
	if folder.kind != FolderKind::Drafts && (job.on_screen)().as_ref() != Some(&folder.id) {
		match cache.trim(&folder.id, job.level.headers()) {
			Ok(0) => {},
			Ok(_) => on(Event::FolderChanged(folder.id.clone())),
			Err(e) => log::warn!("could not trim {}: {e}", folder.name),
		}
	}
	Ok(())
}

/// [`sync_folder`], saying which folder it was.
async fn sync_told<'a>(job: &Job, folder: &'a Folder, on: &(dyn Fn(Event) + Send + Sync)) -> (&'a Folder, Result<()>) {
	(folder, sync_folder(job, folder, on).await)
}

pub async fn round(job: &Job, on: &(dyn Fn(Event) + Send + Sync)) -> Finished {
	let (backend, cache) = (job.backend.as_ref(), job.cache.as_ref());
	// A cached Inbox need not wait for the folder list, which is three requests away and on screen already.
	let cached_inbox = cache.folders().ok().and_then(|folders| folders.into_iter().find(|f| f.kind == FolderKind::Inbox));
	let early = match &cached_inbox {
		Some(inbox) => Some(sync_folder(job, inbox, on).await),
		None => None,
	};
	let listed = crate::folders(backend, cache).await;
	if let Ok(folders) = &listed {
		on(Event::Folders(folders.clone()));
	}
	let mut folders = match listed {
		Ok(folders) => folders,
		Err(e) => {
			on(Event::NoFolders(e.clone()));
			return Finished { listed: false, result: Err(e) };
		},
	};
	if job.labels {
		match backend.labels().await {
			Ok(labels) => on(Event::Labels(labels)),
			Err(e) => log::warn!("could not fetch labels: {e}"),
		}
	}
	let mut result = Ok(());
	let mut stopped = |folder: &Folder, outcome: Result<()>| match outcome {
		Ok(()) => false,
		Err(e) if stops_the_round(&e) => {
			log::warn!("sync stopped: {e}");
			on(Event::Problem(e.clone()));
			result = Err(e);
			true
		},
		Err(e) => {
			log::warn!("could not sync {}: {e}", folder.name);
			false
		},
	};
	let mut stop = false;
	if let (Some(inbox), Some(outcome)) = (&cached_inbox, early) {
		stop = stopped(inbox, outcome);
		folders.retain(|f| f.id != inbox.id);
	}
	// The inbox first, if it has not been done, then the others in their order.
	folders.sort_by_key(|f| f.kind != FolderKind::Inbox);
	let mut rest = folders.as_slice();
	if !stop && rest.first().is_some_and(|f| f.kind == FolderKind::Inbox) {
		stop = stopped(&rest[0], sync_folder(job, &rest[0], on).await);
		rest = &rest[1..];
	}
	if !stop {
		let (mut waiting, mut syncing) = (rest.iter(), FuturesUnordered::new());
		loop {
			while syncing.len() < FOLDERS_AT_ONCE {
				let Some(folder) = waiting.next() else { break };
				syncing.push(sync_told(job, folder, on));
			}
			let Some((folder, outcome)) = syncing.next().await else { break };
			if stopped(folder, outcome) {
				break;
			}
		}
	}
	// New mail in the Inbox through the rules, before it is opened ahead.
	match crate::run_on_new(backend, cache, &job.rules).await {
		Ok(0) => {},
		Ok(_) => on(Event::RulesMoved),
		Err(e) => log::warn!("could not run the rules: {e}"),
	}
	if let Err(e) = job.fetcher.ahead(job.level.bodies()).await {
		log::warn!("could not open mail ahead: {e}");
		on(Event::Problem(e));
	}
	// Last, as nothing on screen depends on them, and they can take seconds.
	if !job.contacts_done.load(Ordering::Relaxed) && crate::contacts_if_stale(backend, cache).await.is_ok() {
		job.contacts_done.store(true, Ordering::Relaxed);
	}
	Finished { listed: true, result }
}

#[cfg(test)]
mod tests {
	use std::collections::HashSet;
	use std::sync::Mutex;

	use async_trait::async_trait;
	use tern_demo::DemoBackend;
	use tern_mail::{Account, Address, Attachment, Draft, Mail, MailId, MailPage, NewAttachment};
	use tern_rules::Action;

	use super::*;
	use crate::fetcher::Tuning;

	/// The demo account, but each folder's list and any move can be told to fail, and folders listed
	/// are noted.
	struct Troubled {
		demo: DemoBackend,
		list_fails: Mutex<Option<Error>>,
		open_fails: Mutex<Option<Error>>,
		move_fails: bool,
		listed: Mutex<Vec<FolderId>>,
	}

	impl Troubled {
		fn new() -> Self {
			Troubled { demo: DemoBackend::new(), list_fails: Mutex::default(), open_fails: Mutex::default(), move_fails: false, listed: Mutex::default() }
		}
	}

	#[async_trait]
	impl Backend for Troubled {
		fn account(&self) -> Account {
			self.demo.account()
		}
		async fn folders(&self) -> Result<Vec<Folder>> {
			self.demo.folders().await
		}
		async fn list(&self, folder: &FolderId, next: Option<String>, count: usize) -> Result<MailPage> {
			self.listed.lock().unwrap().push(folder.clone());
			if let Some(e) = self.list_fails.lock().unwrap().clone() {
				return Err(e);
			}
			self.demo.list(folder, next, count).await
		}
		async fn open(&self, id: &MailId) -> Result<Mail> {
			if let Some(e) = self.open_fails.lock().unwrap().clone() {
				return Err(e);
			}
			self.demo.open(id).await
		}
		async fn open_many(&self, ids: &[MailId]) -> Result<Vec<(MailId, Result<Mail>)>> {
			if let Some(e) = self.open_fails.lock().unwrap().clone() {
				return Err(e);
			}
			self.demo.open_many(ids).await
		}
		async fn attachment(&self, mail: &MailId, attachment: &Attachment) -> Result<Vec<u8>> {
			self.demo.attachment(mail, attachment).await
		}
		async fn send(&self, draft: Draft) -> Result<()> {
			self.demo.send(draft).await
		}
		async fn save_draft(&self, draft: Draft) -> Result<MailId> {
			self.demo.save_draft(draft).await
		}
		async fn contacts(&self) -> Result<Vec<Address>> {
			self.demo.contacts().await
		}
		async fn set_unread(&self, id: &MailId, unread: bool) -> Result<()> {
			self.demo.set_unread(id, unread).await
		}
		async fn trash(&self, id: &MailId) -> Result<()> {
			self.demo.trash(id).await
		}
		async fn move_mails(&self, ids: &[MailId], folder: &FolderId) -> Result<()> {
			if self.move_fails {
				return Err(Error::Other("refused".into()));
			}
			self.demo.move_mails(ids, folder).await
		}
		async fn labels(&self) -> Result<Vec<Label>> {
			self.demo.labels().await
		}
		async fn create_label(&self, name: &str) -> Result<Label> {
			self.demo.create_label(name).await
		}
		async fn add_label(&self, ids: &[MailId], label: &FolderId) -> Result<()> {
			self.demo.add_label(ids, label).await
		}
		async fn remove_label(&self, ids: &[MailId], label: &FolderId) -> Result<()> {
			self.demo.remove_label(ids, label).await
		}
		async fn sign_out(&self) -> Result<()> {
			Ok(())
		}
	}

	fn job(backend: Arc<Troubled>, rules: Vec<Rule>) -> Job {
		let cache = Arc::new(Cache::in_memory());
		let fetcher = Fetcher::new(backend.clone(), cache.clone(), tokio::runtime::Handle::current(), Tuning::DEFAULT);
		Job { backend, cache, fetcher, level: Level::Medium, rules, on_screen: Arc::new(|| None), labels: false, contacts_done: Arc::default() }
	}

	async fn run(job: &Job) -> (Finished, Vec<Event>) {
		let events = Mutex::new(Vec::new());
		let finished = round(job, &|event| events.lock().unwrap().push(event)).await;
		(finished, events.into_inner().unwrap())
	}

	#[tokio::test(flavor = "multi_thread")]
	async fn a_round_brings_everything_up_to_date_and_the_next_finds_nothing() {
		let job = job(Arc::new(Troubled::new()), Vec::new());
		let (finished, events) = run(&job).await;
		assert_eq!(finished, Finished { listed: true, result: Ok(()) });
		assert!(matches!(events[0], Event::Folders(_)), "the folder list comes first");
		assert!(events.iter().any(|e| matches!(e, Event::FolderChanged(_))));
		assert!(!events.iter().any(|e| matches!(e, Event::Labels(_))), "nobody asked for labels");
		assert!(job.contacts_done.load(Ordering::Relaxed));
		let inbox = job.cache.folders().unwrap().into_iter().find(|f| f.kind == FolderKind::Inbox).unwrap();
		assert!(job.cache.count(&inbox.id).unwrap() > 0);

		let (finished, events) = run(&job).await;
		assert_eq!(finished.result, Ok(()));
		assert!(!events.iter().any(|e| matches!(e, Event::FolderChanged(_))), "nothing new: {events:?}");
	}

	#[tokio::test(flavor = "multi_thread")]
	async fn a_later_round_syncs_the_cached_inbox_once() {
		let backend = Arc::new(Troubled::new());
		let job = job(backend.clone(), Vec::new());
		run(&job).await;
		backend.listed.lock().unwrap().clear();
		let (finished, _) = run(&job).await;
		assert_eq!(finished.result, Ok(()));
		let inbox = job.cache.folders().unwrap().into_iter().find(|f| f.kind == FolderKind::Inbox).unwrap();
		let listed = backend.listed.lock().unwrap();
		assert_eq!(listed.iter().filter(|f| **f == inbox.id).count(), 1);
		assert_eq!(listed.iter().collect::<HashSet<_>>().len(), job.cache.folders().unwrap().len(), "every folder was synced");
	}

	#[tokio::test(flavor = "multi_thread")]
	async fn labels_are_fetched_when_asked_for() {
		let mut job = job(Arc::new(Troubled::new()), Vec::new());
		job.labels = true;
		let (_, events) = run(&job).await;
		assert!(events.iter().any(|e| matches!(e, Event::Labels(_))));
	}

	#[tokio::test(flavor = "multi_thread")]
	async fn every_connection_error_stops_at_the_first_folder() {
		for error in [Error::Offline, Error::SessionExpired, Error::RateLimited(Some(30))] {
			let backend = Arc::new(Troubled::new());
			*backend.list_fails.lock().unwrap() = Some(error.clone());
			let (finished, events) = run(&job(backend.clone(), Vec::new())).await;
			assert_eq!(backend.listed.lock().unwrap().len(), 1, "{error:?} stops the round");
			assert_eq!(finished.result, Err(error.clone()));
			assert!(events.contains(&Event::Problem(error)));
		}
	}

	#[tokio::test(flavor = "multi_thread")]
	async fn any_other_folder_error_leaves_the_rest_to_sync() {
		let backend = Arc::new(Troubled::new());
		*backend.list_fails.lock().unwrap() = Some(Error::Other("this one is broken".into()));
		let job = job(backend.clone(), Vec::new());
		let (finished, events) = run(&job).await;
		let seen: HashSet<_> = backend.listed.lock().unwrap().iter().cloned().collect();
		assert_eq!(seen.len(), job.cache.folders().unwrap().len(), "every folder was tried");
		assert_eq!(finished.result, Ok(()));
		assert!(!events.iter().any(|e| matches!(e, Event::Problem(_))));
	}

	#[tokio::test(flavor = "multi_thread")]
	async fn a_failure_opening_mail_ahead_is_told_but_does_not_fail_the_round() {
		let backend = Arc::new(Troubled::new());
		*backend.open_fails.lock().unwrap() = Some(Error::Offline);
		let (finished, events) = run(&job(backend, Vec::new())).await;
		assert_eq!(finished.result, Ok(()));
		assert!(events.contains(&Event::Problem(Error::Offline)), "{events:?}");
	}

	#[tokio::test(flavor = "multi_thread")]
	async fn rules_that_fail_do_not_fail_the_round_and_are_tried_again() {
		let mut backend = Troubled::new();
		backend.move_fails = true;
		let rules = vec![Rule { when: "from:@deals.example".into(), action: Action::Spam }];
		let job = job(Arc::new(backend), rules);
		run(&job).await;
		// As if the offers came in after the first look at the Inbox.
		job.cache.set_mark("rules", i64::MIN + 1).unwrap();
		let (finished, events) = run(&job).await;
		assert_eq!(finished.result, Ok(()));
		assert!(!events.contains(&Event::RulesMoved));
		assert_eq!(job.cache.mark("rules").unwrap(), Some(i64::MIN + 1), "the same mail is tried next time");
	}

	#[tokio::test(flavor = "multi_thread")]
	async fn the_folder_on_screen_keeps_its_older_mail() {
		let backend = Arc::new(Troubled::new());
		let sent = FolderId("sent".into());
		for n in 0..150 {
			backend.demo.send(Draft { to: vec![Address::new("", "a@b.org")], subject: format!("Note {n}"), body_html: "<p>Hi</p>".into(), attachments: Vec::<NewAttachment>::new(), ..Default::default() }).await.unwrap();
		}
		let mut on_screen = job(backend.clone(), Vec::new());
		on_screen.level = Level::Low;
		let shown = sent.clone();
		on_screen.on_screen = Arc::new(move || Some(shown.clone()));
		run(&on_screen).await;
		crate::older(on_screen.backend.as_ref(), &on_screen.cache, &sent).await.unwrap();
		let scrolled_to = on_screen.cache.count(&sent).unwrap();
		assert!(scrolled_to > Level::Low.headers(), "only {scrolled_to} to keep or trim");
		run(&on_screen).await;
		assert_eq!(on_screen.cache.count(&sent).unwrap(), scrolled_to, "kept while it is on screen");

		on_screen.on_screen = Arc::new(|| None);
		run(&on_screen).await;
		assert_eq!(on_screen.cache.count(&sent).unwrap(), Level::Low.headers(), "trimmed once it is not");
	}

	#[tokio::test(flavor = "multi_thread")]
	async fn dropping_stops_ends_the_task() {
		let task = tokio::spawn(async { std::future::pending::<()>().await });
		let handle = task.abort_handle();
		drop(Stops(handle));
		assert!(task.await.unwrap_err().is_cancelled());
	}
}

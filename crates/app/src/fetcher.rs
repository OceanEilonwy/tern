//! The one place mail bodies come from the server. The reader, the list and the background sync say
//! what they want; [`tern_cache::policy`] turns that into batches; this runs them, most urgent first,
//! a few at a time, stores what arrives and hands it to whoever waits for it.
//!
//! It runs on the Tokio runtime and touches no widgets, so it is tested without a display.
//!
//! When things go wrong it falls back rather than giving up: a batch the server refuses is opened
//! one mail at a time; a mail that fails in a batch is tried once alone before it is marked
//! unopenable; when the server asks for fewer requests the work waits and goes again; offline or
//! signed out, the queue stops and keeps its work for the next round.

use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use tern_cache::policy::{self, Batch, Priority, Want};
use tern_cache::Cache;
use tern_core::{Backend, Error, Mail, MailId, Result};
use tokio::runtime::Handle;
use tokio::sync::{oneshot, watch};

/// How hard the fetcher pushes the server.
#[derive(Clone, Copy, Debug)]
pub struct Tuning {
	/// Requests in flight at once.
	pub lanes: usize,
	/// The longest wait when the server asks for a pause, however long it asks for.
	pub max_wait: Duration,
	/// The wait when it asks for a pause without saying how long.
	pub default_wait: Duration,
}

impl Tuning {
	/// Three lanes: the background may use one, the list two, and the reader all three, so a
	/// conversation being read always has a lane free.
	pub const DEFAULT: Tuning = Tuning { lanes: 3, max_wait: Duration::from_secs(60), default_wait: Duration::from_secs(5) };

	/// Requests that may be in flight at `priority` and below: a lane fewer for each step down, and
	/// at least one.
	fn limit(&self, priority: Priority) -> usize {
		let steps = Priority::Reading as usize - priority as usize;
		self.lanes.saturating_sub(steps).max(1)
	}
}

type Waiter = oneshot::Sender<Result<Mail>>;

#[derive(Default)]
struct State {
	queue: Vec<Batch>,
	/// Mails being fetched now, with the priority of the request fetching them.
	in_flight: HashMap<MailId, Priority>,
	waiters: HashMap<MailId, Vec<Waiter>>,
	/// Requests in flight, by priority.
	running: [usize; 3],
	/// No request starts before this: the server asked for a pause.
	paused_until: Option<Instant>,
	/// A task is asleep until the pause ends.
	waking: bool,
	/// Offline or signed out: nothing starts until something asks again.
	halted: Option<Error>,
	stopped: bool,
	/// Mails the background stored so far, for [`Fetcher::ahead`] to count.
	opened_ahead: usize,
	/// The batched request failed once already this session, and the log said so.
	fell_back: bool,
}

impl State {
	fn queued(&self, id: &MailId) -> bool {
		self.queue.iter().any(|b| b.ids.contains(id))
	}

	/// Adds `batch`, leaving out mails already on their way; a mail queued less urgently moves
	/// into this batch.
	fn enqueue(&mut self, mut batch: Batch) {
		batch.ids.retain(|id| !self.in_flight.contains_key(id));
		let mut taken = Vec::new();
		for queued in &mut self.queue {
			queued.ids.retain(|id| {
				let found = batch.ids.contains(id);
				if found && queued.priority >= batch.priority {
					taken.push(id.clone());
				}
				!found || queued.priority >= batch.priority
			});
		}
		batch.ids.retain(|id| !taken.contains(id));
		self.queue.retain(|b| !b.ids.is_empty());
		if !batch.ids.is_empty() {
			self.queue.push(batch);
		}
	}

	/// Puts mails back at the head of the queue, to go again.
	fn requeue(&mut self, priority: Priority, ids: Vec<MailId>) {
		if !ids.is_empty() {
			self.queue.insert(0, Batch { priority, ids });
		}
	}

	/// The next batch to start, if a lane is free for it: the most urgent, oldest first.
	fn next(&mut self, tuning: &Tuning) -> Option<Batch> {
		let top = self.queue.iter().map(|b| b.priority).max()?;
		let at_or_below: usize = self.running[..=top as usize].iter().sum();
		if at_or_below >= tuning.limit(top) {
			return None;
		}
		let at = self.queue.iter().position(|b| b.priority == top)?;
		Some(self.queue.remove(at))
	}

	fn background_left(&self) -> bool {
		self.running[Priority::Background as usize] > 0 || self.queue.iter().any(|b| b.priority == Priority::Background)
	}
}

struct Inner {
	backend: Arc<dyn Backend>,
	cache: Arc<Cache>,
	tuning: Tuning,
	runtime: Handle,
	state: Mutex<State>,
	/// Ticks whenever a batch ends or the queue halts.
	changed: watch::Sender<()>,
}

#[derive(Clone)]
pub struct Fetcher {
	inner: Arc<Inner>,
}

/// What a batch came to, for the queue.
#[derive(Default)]
struct Outcome {
	/// Mails to try again after a pause.
	later: Vec<MailId>,
	pause: Option<Option<u64>>,
	halt: Option<Error>,
}

impl Fetcher {
	pub fn new(backend: Arc<dyn Backend>, cache: Arc<Cache>, runtime: Handle, tuning: Tuning) -> Fetcher {
		let inner = Inner { backend, cache, tuning, runtime, state: Mutex::default(), changed: watch::channel(()).0 };
		Fetcher { inner: Arc::new(inner) }
	}

	/// Queues the bodies `want` names that the cache does not have. A conversation opened for
	/// reading goes ahead of one opened before it, and starts the queue again if it had stopped.
	pub fn want(&self, want: Want) {
		let reading = matches!(want, Want::Reading { .. });
		let batches = policy::batches(want, |id| self.inner.cache.has_mail(id).unwrap_or(false));
		let mut state = self.inner.state.lock().unwrap();
		if reading {
			state.halted = None;
			for batch in state.queue.iter_mut().filter(|b| b.priority == Priority::Reading) {
				batch.priority = Priority::Viewing;
			}
		}
		for batch in batches {
			state.enqueue(batch);
		}
		drop(state);
		self.inner.pump();
	}

	/// One mail, from the cache or the server. Someone waits on screen: unless a reading batch is
	/// fetching it already, it is fetched alone at once rather than after a bigger, less urgent batch.
	pub async fn mail(&self, id: &MailId) -> Result<Mail> {
		let cached = |inner: &Inner| inner.cache.mail(id).ok().flatten();
		if let Some(mail) = cached(&self.inner) {
			return Ok(mail);
		}
		let (tx, rx) = oneshot::channel();
		{
			let mut state = self.inner.state.lock().unwrap();
			if state.stopped {
				return Err(cancelled());
			}
			let on_its_way = state.queue.iter().any(|b| b.priority == Priority::Reading && b.ids.contains(id))
				|| state.in_flight.get(id) == Some(&Priority::Reading);
			if !on_its_way {
				// Stored meanwhile, by a batch that ended after the look above.
				if !state.queued(id) && !state.in_flight.contains_key(id) {
					if let Some(mail) = cached(&self.inner) {
						return Ok(mail);
					}
				}
				let batch = Batch { priority: Priority::Reading, ids: vec![id.clone()] };
				// Past the in-flight check, so a batch holding it does not hold it up.
				state.queue.retain_mut(|b| {
					b.ids.retain(|i| i != id);
					!b.ids.is_empty()
				});
				state.queue.insert(0, batch);
			}
			state.halted = None;
			state.waiters.entry(id.clone()).or_default().push(tx);
		}
		self.inner.pump();
		rx.await.unwrap_or_else(|_| Err(cancelled()))
	}

	/// Opens up to `count` of the newest mails not stored yet, in the background, and returns how
	/// many it stored once they are done. Stops early with the error that halted the queue.
	pub async fn ahead(&self, count: usize) -> Result<usize> {
		let missing = self.inner.cache.missing_bodies(count).map_err(|e| Error::Other(e.to_string()))?;
		let mut changed = self.inner.changed.subscribe();
		let before = {
			let mut state = self.inner.state.lock().unwrap();
			state.halted = None;
			for batch in policy::batches(Want::Background(missing), |_| false) {
				state.enqueue(batch);
			}
			state.opened_ahead
		};
		self.inner.pump();
		loop {
			{
				let state = self.inner.state.lock().unwrap();
				if let Some(e) = &state.halted {
					return Err(e.clone());
				}
				if state.stopped || !state.background_left() {
					return Ok(state.opened_ahead - before);
				}
			}
			if changed.changed().await.is_err() {
				return Err(cancelled());
			}
		}
	}

	/// Signed out: drops the queue and its waiters; requests in flight finish unheard.
	pub fn stop(&self) {
		let mut state = self.inner.state.lock().unwrap();
		state.stopped = true;
		state.queue.clear();
		state.waiters.clear();
		drop(state);
		self.inner.changed.send_replace(());
	}
}

fn cancelled() -> Error {
	Error::Other("Cancelled".into())
}

impl Inner {
	/// Starts what may start now.
	fn pump(self: &Arc<Self>) {
		let mut state = self.state.lock().unwrap();
		if state.stopped || state.halted.is_some() {
			return;
		}
		if let Some(until) = state.paused_until {
			if Instant::now() < until {
				if !state.waking {
					state.waking = true;
					let this = self.clone();
					self.runtime.spawn(async move {
						tokio::time::sleep_until(until.into()).await;
						this.state.lock().unwrap().waking = false;
						this.pump();
					});
				}
				return;
			}
			state.paused_until = None;
		}
		while let Some(batch) = state.next(&self.tuning) {
			state.running[batch.priority as usize] += 1;
			for id in &batch.ids {
				let current = state.in_flight.entry(id.clone()).or_insert(batch.priority);
				*current = (*current).max(batch.priority);
			}
			let this = self.clone();
			self.runtime.spawn(async move { this.run(batch).await });
		}
	}

	async fn run(self: Arc<Self>, batch: Batch) {
		let mut outcome = Outcome::default();
		match self.backend.open_many(&batch.ids).await {
			Ok(results) => {
				let mut answered = HashSet::new();
				let mut failed = Vec::new();
				for (id, result) in results {
					answered.insert(id.clone());
					match result {
						Ok(mail) => self.finish(&id, batch.priority, Ok(mail)),
						Err(_) => failed.push(id),
					}
				}
				failed.extend(batch.ids.iter().filter(|id| !answered.contains(*id)).cloned());
				// Tried once more, alone.
				self.one_by_one(&batch, failed, &mut outcome).await;
			},
			Err(Error::RateLimited(seconds)) => {
				outcome.pause = Some(seconds);
				outcome.later = batch.ids.clone();
			},
			Err(e @ (Error::Offline | Error::SessionExpired)) => {
				outcome.halt = Some(e);
				outcome.later = batch.ids.clone();
			},
			Err(e) => {
				let first = !std::mem::replace(&mut self.state.lock().unwrap().fell_back, true);
				if first {
					tern_tuta::trace(&format!("opening {} mails at once failed ({e}); opening them one by one", batch.ids.len()));
				}
				self.one_by_one(&batch, batch.ids.clone(), &mut outcome).await;
			},
		}
		self.end(batch, outcome);
	}

	/// Opens `ids` one at a time, the way Tern opened mail before batches. A mail that fails here
	/// for a reason of its own is marked unopenable.
	async fn one_by_one(&self, batch: &Batch, ids: Vec<MailId>, outcome: &mut Outcome) {
		for id in ids {
			if outcome.pause.is_some() || outcome.halt.is_some() {
				outcome.later.push(id);
				continue;
			}
			match self.backend.open(&id).await {
				Ok(mail) => self.finish(&id, batch.priority, Ok(mail)),
				Err(Error::RateLimited(seconds)) => {
					outcome.pause = Some(seconds);
					outcome.later.push(id);
				},
				Err(e @ (Error::Offline | Error::SessionExpired)) => {
					outcome.halt = Some(e);
					outcome.later.push(id);
				},
				Err(e) => {
					if let Err(e) = self.cache.mark_unopenable(&id) {
						eprintln!("tern: could not store a mail as unopenable: {e}");
					}
					self.finish(&id, batch.priority, Err(e));
				},
			}
		}
	}

	/// Stores a mail (or its failure) and hands it to whoever waits for it.
	fn finish(&self, id: &MailId, priority: Priority, result: Result<Mail>) {
		if let Ok(mail) = &result {
			if let Err(e) = self.cache.put_mail(mail) {
				eprintln!("tern: could not store a mail: {e}");
			}
		}
		let mut state = self.state.lock().unwrap();
		if result.is_ok() && priority == Priority::Background {
			state.opened_ahead += 1;
		}
		if state.in_flight.get(id) == Some(&priority) {
			state.in_flight.remove(id);
		}
		for waiter in state.waiters.remove(id).unwrap_or_default() {
			let _ = waiter.send(result.clone());
		}
	}

	/// A batch is over: frees its lane, queues what goes again, and starts what can start.
	fn end(self: &Arc<Self>, batch: Batch, outcome: Outcome) {
		let mut state = self.state.lock().unwrap();
		state.running[batch.priority as usize] -= 1;
		for id in &outcome.later {
			if state.in_flight.get(id) == Some(&batch.priority) {
				state.in_flight.remove(id);
			}
		}
		if !state.stopped {
			state.requeue(batch.priority, outcome.later);
		}
		if let Some(seconds) = outcome.pause {
			let wait = seconds.map_or(self.tuning.default_wait, Duration::from_secs).min(self.tuning.max_wait);
			tern_tuta::trace(&format!("the server asks for fewer requests; waiting {} s", wait.as_secs()));
			let until = Instant::now() + wait;
			state.paused_until = Some(state.paused_until.map_or(until, |u| u.max(until)));
		}
		if let Some(e) = outcome.halt {
			tern_tuta::trace(&format!("fetching stopped: {e}"));
			// Waiting cards show the error; the queue keeps its work for the next round.
			let waiting: Vec<MailId> = state.waiters.keys().filter(|id| !state.in_flight.contains_key(*id)).cloned().collect();
			for id in waiting {
				for waiter in state.waiters.remove(&id).unwrap_or_default() {
					let _ = waiter.send(Err(e.clone()));
				}
			}
			state.halted = Some(e);
		}
		drop(state);
		self.changed.send_replace(());
		self.pump();
	}
}

#[cfg(test)]
mod tests {
	use super::*;
	use std::collections::VecDeque;
	use tern_cache::sync;
	use tern_core::demo::DemoBackend;
	use tern_core::{Address, Attachment, Draft, Folder, FolderId, MailPage};

	/// The demo account, recording each call, with failures and holds scripted in.
	#[derive(Default)]
	struct Scripted {
		demo: DemoBackend,
		calls: Mutex<Vec<(&'static str, Vec<MailId>)>>,
		/// What the next batches answer instead of opening.
		batch_errors: Mutex<VecDeque<Error>>,
		/// What the next single opens answer instead of opening.
		single_errors: Mutex<VecDeque<Error>>,
		/// Mails that fail inside a batch (but open alone).
		flaky: Vec<MailId>,
		/// Batches of more than one mail wait for this.
		hold: Option<Arc<tokio::sync::Semaphore>>,
	}

	impl Scripted {
		fn calls(&self) -> Vec<(&'static str, Vec<MailId>)> {
			self.calls.lock().unwrap().clone()
		}
	}

	#[async_trait::async_trait]
	impl Backend for Scripted {
		fn account(&self) -> tern_core::Account {
			self.demo.account()
		}
		async fn folders(&self) -> Result<Vec<Folder>> {
			self.demo.folders().await
		}
		async fn list(&self, folder: &FolderId, next: Option<String>, count: usize) -> Result<MailPage> {
			self.demo.list(folder, next, count).await
		}
		async fn open(&self, id: &MailId) -> Result<Mail> {
			self.calls.lock().unwrap().push(("open", vec![id.clone()]));
			if let Some(e) = self.single_errors.lock().unwrap().pop_front() {
				return Err(e);
			}
			self.demo.open(id).await
		}
		async fn open_many(&self, ids: &[MailId]) -> Result<Vec<(MailId, Result<Mail>)>> {
			self.calls.lock().unwrap().push(("open_many", ids.to_vec()));
			if let (Some(hold), true) = (&self.hold, ids.len() > 1) {
				let _ = hold.acquire().await.map(|p| p.forget());
			}
			if let Some(e) = self.batch_errors.lock().unwrap().pop_front() {
				return Err(e);
			}
			let mut opened = self.demo.open_many(ids).await?;
			for (id, result) in &mut opened {
				if self.flaky.contains(id) {
					*result = Err(Error::Other("does not decrypt in a batch".into()));
				}
			}
			Ok(opened)
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
			self.demo.move_mails(ids, folder).await
		}
		async fn labels(&self) -> Result<Vec<tern_core::Label>> {
			self.demo.labels().await
		}
		async fn add_label(&self, ids: &[MailId], label: &FolderId) -> Result<()> {
			self.demo.add_label(ids, label).await
		}
		async fn sign_out(&self) -> Result<()> {
			Ok(())
		}
	}

	/// A cache with every demo folder's list synced, the fetcher on it, and the five newest mails.
	async fn setup(backend: Scripted, tuning: Tuning) -> (Arc<Scripted>, Arc<Cache>, Fetcher, Vec<MailId>) {
		let backend = Arc::new(backend);
		let cache = Arc::new(Cache::in_memory());
		for folder in sync::folders(backend.as_ref(), &cache).await.unwrap() {
			sync::folder(backend.as_ref(), &cache, &folder.id, 100).await.unwrap();
		}
		let newest = cache.missing_bodies(5).unwrap();
		let fetcher = Fetcher::new(backend.clone(), cache.clone(), Handle::current(), tuning);
		(backend, cache, fetcher, newest)
	}

	fn gone() -> MailId {
		MailId { list: "demo".into(), element: "gone".into() }
	}

	#[tokio::test]
	async fn the_open_ahead_batches_stores_and_skips_what_is_stored() {
		let (backend, cache, fetcher, newest) = setup(Scripted::default(), Tuning::DEFAULT).await;
		assert_eq!(newest.len(), 5);
		assert_eq!(fetcher.ahead(3).await.unwrap(), 3);
		assert_eq!(backend.calls(), [("open_many", newest[..3].to_vec())], "one request, newest first");
		assert!(newest[..3].iter().all(|id| cache.has_mail(id).unwrap()));
		assert_eq!(fetcher.ahead(5).await.unwrap(), 2);
		assert_eq!(fetcher.ahead(5).await.unwrap(), 0, "nothing is fetched twice");
		assert_eq!(backend.calls().len(), 2);
	}

	#[tokio::test]
	async fn reading_goes_ahead_of_the_background_and_nothing_is_fetched_twice() {
		let hold = Arc::new(tokio::sync::Semaphore::new(0));
		let (backend, cache, fetcher, newest) = setup(Scripted { hold: Some(hold.clone()), ..Default::default() }, Tuning::DEFAULT).await;
		// The background takes its batch and is held there.
		let ahead = tokio::spawn({
			let fetcher = fetcher.clone();
			async move { fetcher.ahead(5).await }
		});
		while backend.calls().is_empty() {
			tokio::task::yield_now().await;
		}
		// A conversation opens over it: its focused mail, held in the background's batch, comes at
		// once on the free lane; the rest of it is already on its way and is not asked for again.
		let thread: Vec<MailId> = newest.iter().rev().cloned().collect();
		let focus = newest[1].clone();
		fetcher.want(Want::Reading { thread, focus: focus.clone() });
		fetcher.want(Want::Viewing(newest.clone()));
		let mail = fetcher.mail(&focus).await.unwrap();
		assert_eq!(mail.summary.id, focus);
		assert_eq!(backend.calls(), [("open_many", newest.clone()), ("open_many", vec![focus.clone()])]);
		assert!(!cache.has_mail(&newest[0]).unwrap(), "the background batch is still held");
		hold.add_permits(10);
		assert_eq!(ahead.await.unwrap().unwrap(), 5);
		assert_eq!(backend.calls().len(), 2, "every mail was asked for once, the focus once more");
	}

	#[test]
	fn each_step_down_leaves_a_lane_for_the_more_urgent() {
		let tuning = Tuning::DEFAULT;
		assert_eq!([Priority::Background, Priority::Viewing, Priority::Reading].map(|p| tuning.limit(p)), [1, 2, 3]);
		assert_eq!(Tuning { lanes: 1, ..tuning }.limit(Priority::Background), 1);
	}

	#[tokio::test]
	async fn the_newest_conversation_opened_comes_first() {
		let hold = Arc::new(tokio::sync::Semaphore::new(0));
		// One lane, so the queue's order shows.
		let tuning = Tuning { lanes: 1, ..Tuning::DEFAULT };
		let (backend, _cache, fetcher, newest) = setup(Scripted { hold: Some(hold.clone()), ..Default::default() }, tuning).await;
		fetcher.want(Want::Viewing(newest[3..].to_vec()));
		while backend.calls().is_empty() {
			tokio::task::yield_now().await;
		}
		fetcher.want(Want::Reading { thread: newest[..2].to_vec(), focus: newest[1].clone() });
		fetcher.want(Want::Reading { thread: vec![newest[2].clone()], focus: newest[2].clone() });
		hold.add_permits(10);
		while backend.calls().len() < 4 {
			tokio::task::yield_now().await;
		}
		let order: Vec<Vec<MailId>> = backend.calls().into_iter().map(|(_, ids)| ids).collect();
		assert_eq!(order, [newest[3..].to_vec(), vec![newest[2].clone()], vec![newest[1].clone()], vec![newest[0].clone()]]);
	}

	#[tokio::test]
	async fn a_refused_batch_falls_back_to_one_mail_at_a_time() {
		let scripted = Scripted { batch_errors: Mutex::new([Error::Other("Internal Server".into())].into()), ..Default::default() };
		let (backend, cache, fetcher, newest) = setup(scripted, Tuning::DEFAULT).await;
		assert_eq!(fetcher.ahead(3).await.unwrap(), 3);
		let calls = backend.calls();
		assert_eq!(calls[0], ("open_many", newest[..3].to_vec()));
		assert_eq!(calls[1..], newest[..3].iter().map(|id| ("open", vec![id.clone()])).collect::<Vec<_>>());
		assert!(newest[..3].iter().all(|id| cache.has_mail(id).unwrap()));
	}

	#[tokio::test]
	async fn a_mail_failing_in_a_batch_is_tried_alone_before_it_is_given_up() {
		let (backend, cache, fetcher, newest) = setup(Scripted { flaky: vec![newest_flaky()], ..Default::default() }, Tuning::DEFAULT).await;
		let flaky = newest_flaky();
		let gone = gone();
		fetcher.want(Want::Reading { thread: vec![newest[0].clone(), flaky.clone(), gone.clone()], focus: newest[0].clone() });
		let (flaky_mail, gone_mail) = tokio::join!(fetcher.mail(&flaky), fetcher.mail(&gone));
		assert!(flaky_mail.is_ok(), "it opened alone");
		assert!(gone_mail.is_err());
		assert_eq!(backend.calls()[1], ("open_many", vec![gone.clone(), flaky.clone()]));
		assert_eq!(backend.calls()[2..], [("open", vec![gone.clone()]), ("open", vec![flaky.clone()])]);
		assert!(cache.has_mail(&newest[0]).unwrap() && cache.has_mail(&flaky).unwrap());
		// The one that failed alone too is not tried again in the background.
		assert!(!cache.missing_bodies(100).unwrap().contains(&gone));
	}

	/// The demo's invoice: a mail in the middle of the newest.
	fn newest_flaky() -> MailId {
		MailId { list: "demo".into(), element: "m002".into() }
	}

	#[tokio::test]
	async fn a_rate_limit_waits_and_goes_again_without_giving_up_mail() {
		let scripted = Scripted {
			batch_errors: Mutex::new([Error::RateLimited(Some(30))].into()),
			single_errors: Mutex::new([Error::RateLimited(None)].into()),
			flaky: vec![newest_flaky()],
			..Default::default()
		};
		let tuning = Tuning { max_wait: Duration::from_millis(80), default_wait: Duration::from_millis(40), ..Tuning::DEFAULT };
		let (backend, cache, fetcher, newest) = setup(scripted, tuning).await;
		let started = Instant::now();
		assert_eq!(fetcher.ahead(5).await.unwrap(), 5);
		// 30 s asked, capped to 80 ms; then the flaky mail alone is limited too and waits 40 ms more.
		assert!(started.elapsed() >= Duration::from_millis(120), "waited only {:?}", started.elapsed());
		assert!(newest.iter().all(|id| cache.has_mail(id).unwrap()), "nothing marked unopenable");
		let calls = backend.calls();
		assert_eq!(calls[0], calls[1], "the same batch, again");
	}

	#[tokio::test]
	async fn offline_stops_the_queue_and_keeps_its_work() {
		let scripted = Scripted { batch_errors: Mutex::new([Error::Offline, Error::Offline].into()), ..Default::default() };
		let (backend, cache, fetcher, newest) = setup(scripted, Tuning::DEFAULT).await;
		// Two batches, the focus and the rest, go out together and both find the network gone.
		fetcher.want(Want::Reading { thread: newest.clone(), focus: newest[0].clone() });
		assert_eq!(fetcher.mail(&newest[0]).await.unwrap_err(), Error::Offline, "the reader shows why");
		tokio::task::yield_now().await;
		assert_eq!(backend.calls().len(), 2, "nothing more is tried while offline");
		assert!(!cache.has_mail(&newest[1]).unwrap());
		// The next round picks the work up again.
		assert_eq!(fetcher.ahead(0).await.unwrap(), 0);
		while !newest.iter().all(|id| cache.has_mail(id).unwrap()) {
			tokio::task::yield_now().await;
		}
		assert_eq!(backend.calls().len(), 4, "each batch once more");
	}

	#[tokio::test]
	async fn a_card_does_not_wait_behind_the_lists_batch() {
		let hold = Arc::new(tokio::sync::Semaphore::new(0));
		let (backend, _cache, fetcher, newest) = setup(Scripted { hold: Some(hold), ..Default::default() }, Tuning::DEFAULT).await;
		fetcher.want(Want::Viewing(newest.clone()));
		// The list's batch is held; the card's mail comes alone.
		assert!(fetcher.mail(&newest[2]).await.is_ok());
		assert_eq!(backend.calls(), [("open_many", newest.clone()), ("open_many", vec![newest[2].clone()])]);
	}

	#[tokio::test]
	async fn a_stopped_fetcher_answers_waiters() {
		let hold = Arc::new(tokio::sync::Semaphore::new(0));
		let tuning = Tuning { lanes: 1, ..Tuning::DEFAULT };
		let (_backend, _cache, fetcher, newest) = setup(Scripted { hold: Some(hold), ..Default::default() }, tuning).await;
		fetcher.want(Want::Viewing(newest[..2].to_vec()));
		let waiting = tokio::spawn({
			let (fetcher, id) = (fetcher.clone(), newest[3].clone());
			async move { fetcher.mail(&id).await }
		});
		while fetcher.inner.state.lock().unwrap().waiters.is_empty() {
			tokio::task::yield_now().await;
		}
		fetcher.stop();
		assert!(waiting.await.unwrap().is_err());
		assert!(fetcher.mail(&newest[4]).await.is_err());
	}
}

//! The app itself: one task owns all the state, takes the person's actions and the results of work
//! in the background one at a time, and publishes what changed. Nothing here waits: anything slow
//! runs in a task of its own and comes back as a closure over the state, which is dropped if the
//! account it was started for has gone meanwhile.

use std::collections::{BTreeSet, HashMap, HashSet};
use std::future::Future;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use async_trait::async_trait;
use tern_cache::search::Query;
use tern_sync::Level;
use tern_cache::{Cache, ThreadRow};
use tern_sync::policy::Want;
use tern_demo::{DemoBackend, DEMO_LOGIN};
use tern_rules::{parse, term, Field, Part, Rule};
use tern_mail::{Account, Address, Attachment, Backend, Draft, Error, Folder, FolderId, FolderKind, Label, Mail, MailId, MailPage, MailSummary, NewAttachment, Result as CoreResult};
use crate::cleanup::{self, Pick, Usage};
use tern_compose::{Answer, Fields, Quote, MAX_TOTAL_BYTES};
use tern_sync::fetcher::{Fetcher, Tuning};
use tern_sync::{Event, Job, Stops};
use tern_html::Doc;
use crate::rules;
use tern_rules::Piece;
use crate::settings::{Settings, BACKGROUND_MINUTES, CLEANUP_MB};
use crate::paths::Paths;
use crate::semantic::{self, Meaning};
use crate::format;
use tokio::sync::mpsc::UnboundedSender;
use tokio::sync::Notify;

use crate::doc::{block_text, flatten, Picture};
use crate::view::*;
use crate::{EngineListener, CACHE_KEY_SECRET, SESSION_SECRET, Options, SecretError, SecretStore, SessionParts};

/// Conversations listed before scrolling asks for more.
const ROWS: usize = 100;
/// The rows whose mail is opened ahead, as likely to be read next.
const VIEWING: usize = 25;
const RESULTS: usize = 200;
const SYNC_EVERY: Duration = Duration::from_secs(5 * 60);
const RESUME_EVERY: Duration = Duration::from_secs(60);
/// Mails the rule page lists from those it catches; the rest are counted.
const PREVIEW: usize = 4;
/// Matches Move to Spam lists at most.
const SPAM_SHOWN: usize = 500;
/// Mail in these is not offered for Spam: it is already gone, or this account's own.
const SPAM_SKIP: [FolderKind; 4] = [FolderKind::Spam, FolderKind::Trash, FolderKind::Sent, FolderKind::Drafts];
/// How far the Inbox has been seen or told about: the newest received time looked at.
const NOTIFIED: &str = "notified";
/// Conversations the system is told about at once; the rest are only counted.
const MAX_NOTICES: usize = 6;
/// A background check right after a round has nothing to add.
const ROUND_FRESH_MS: i64 = 60_000;

pub type Apply = Box<dyn FnOnce(&mut App) + Send>;

pub enum Msg {
	Action(Action),
	/// The result of work started for this account.
	Done(u64, Apply),
	/// The system woke the app to check for mail; the answer goes back here.
	Background(std::sync::mpsc::Sender<BackgroundResult>),
	/// The platform wants the sync's next round now.
	SyncNow,
	Compose(Box<Fields>),
	/// The engine has been dropped.
	Stop,
}

/// What Tern is doing, from least to most important to the person waiting.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Word {
	Fetching,
	Opening,
	Downloading,
	Moving,
	Deleting,
	Attaching,
	Drafting,
	Sending,
}

const WORDS: [(Word, &str); 8] = [
	(Word::Sending, "Sending…"),
	(Word::Drafting, "Drafting…"),
	(Word::Attaching, "Attaching…"),
	(Word::Deleting, "Deleting…"),
	(Word::Moving, "Moving…"),
	(Word::Downloading, "Downloading…"),
	(Word::Opening, "Opening…"),
	(Word::Fetching, "Fetching…"),
];

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Conn {
	Fine,
	Offline,
	Busy,
	SignedOut,
}

/// Stands in until the session is back, so saved mail can be read with no connection at all.
struct NoServer {
	address: String,
}

#[async_trait]
impl Backend for NoServer {
	fn account(&self) -> Account {
		Account { address: self.address.clone(), name: String::new(), aliases: Vec::new() }
	}
	async fn folders(&self) -> CoreResult<Vec<Folder>> {
		Err(Error::Offline)
	}
	async fn list(&self, _: &FolderId, _: Option<String>, _: usize) -> CoreResult<MailPage> {
		Err(Error::Offline)
	}
	async fn open(&self, _: &MailId) -> CoreResult<Mail> {
		Err(Error::Offline)
	}
	async fn open_many(&self, _: &[MailId]) -> CoreResult<Vec<(MailId, CoreResult<Mail>)>> {
		Err(Error::Offline)
	}
	async fn attachment(&self, _: &MailId, _: &Attachment) -> CoreResult<Vec<u8>> {
		Err(Error::Offline)
	}
	async fn send(&self, _: Draft) -> CoreResult<()> {
		Err(Error::Offline)
	}
	async fn save_draft(&self, _: Draft) -> CoreResult<MailId> {
		Err(Error::Offline)
	}
	async fn contacts(&self) -> CoreResult<Vec<Address>> {
		Err(Error::Offline)
	}
	async fn set_unread(&self, _: &MailId, _: bool) -> CoreResult<()> {
		Err(Error::Offline)
	}
	async fn trash(&self, _: &MailId) -> CoreResult<()> {
		Err(Error::Offline)
	}
	async fn move_mails(&self, _: &[MailId], _: &FolderId) -> CoreResult<()> {
		Err(Error::Offline)
	}
	async fn labels(&self) -> CoreResult<Vec<Label>> {
		Err(Error::Offline)
	}
	async fn create_label(&self, _: &str) -> CoreResult<Label> {
		Err(Error::Offline)
	}
	async fn add_label(&self, _: &[MailId], _: &FolderId) -> CoreResult<()> {
		Err(Error::Offline)
	}
	async fn remove_label(&self, _: &[MailId], _: &FolderId) -> CoreResult<()> {
		Err(Error::Offline)
	}
	async fn sign_out(&self) -> CoreResult<()> {
		Err(Error::Offline)
	}
}

/// What only a signed-in account has.
struct Session {
	email: String,
	backend: Arc<dyn Backend>,
	cache: Arc<Cache>,
	fetcher: Fetcher,
	/// Kept on disk and remembered, unlike the demo's.
	persistent: bool,
	/// The session is back and the server can be asked.
	connected: bool,
	wake: Arc<Notify>,
	/// When a round of the sync last finished. Held while one runs, so two never run at once.
	last_round: Arc<tokio::sync::Mutex<i64>>,
	contacts_done: Arc<std::sync::atomic::AtomicBool>,
	/// The demo account, which has mail arrive when asked.
	demo: Option<Arc<DemoBackend>>,
	/// The rounds of the sync, which end with the session.
	sync: Option<Stops>,
}

/// One round of keeping the cache up to date, for a session.
struct Round {
	paths: Paths,
	meaning: Arc<Meaning>,
	backend: Arc<dyn Backend>,
	cache: Arc<Cache>,
	fetcher: Fetcher,
	settings: Arc<Mutex<Settings>>,
	current: Arc<Mutex<Option<Folder>>>,
	tx: UnboundedSender<Msg>,
	generation: u64,
	last: Arc<tokio::sync::Mutex<i64>>,
	contacts_done: Arc<std::sync::atomic::AtomicBool>,
}

impl Round {
	fn post(&self, apply: Apply) {
		let _ = self.tx.send(Msg::Done(self.generation, apply));
	}

	/// A round now, after any already running.
	async fn run(&self) -> CoreResult<()> {
		let mut last = self.last.lock().await;
		let result = self.go().await;
		*last = format::now_ms();
		result
	}

	/// A round, unless one finished just now, as when the same wake-up started both.
	async fn run_unless_just_done(&self) -> CoreResult<()> {
		let mut last = self.last.lock().await;
		if format::now_ms() - *last < ROUND_FRESH_MS {
			return Ok(());
		}
		let result = self.go().await;
		*last = format::now_ms();
		result
	}

	async fn go(&self) -> CoreResult<()> {
		self.post(Box::new(|app| app.start_work(Word::Fetching)));
		let settings = *self.settings.lock().unwrap_or_else(|p| p.into_inner());
		let current = self.current.clone();
		let job = Job {
			backend: self.backend.clone(),
			cache: self.cache.clone(),
			fetcher: self.fetcher.clone(),
			level: settings.offline,
			rules: rules::load(&self.paths),
			on_screen: Arc::new(move || current.lock().unwrap_or_else(|p| p.into_inner()).as_ref().map(|f| f.id.clone())),
			labels: true,
			contacts_done: self.contacts_done.clone(),
		};
		let finished = tern_sync::round(&job, &|event| {
			self.post(Box::new(move |app| match event {
				Event::Folders(folders) => {
					app.note_fine();
					app.show_folders(folders);
				},
				Event::Labels(labels) => app.labels = labels,
				Event::FolderChanged(id) => app.folder_changed(Some(id)),
				Event::RulesMoved => app.folder_changed(None),
				Event::Problem(e) => app.note_error(&e),
				Event::NoFolders(e) => app.sync_failed(e),
			}));
		})
		.await;
		let cache = &self.cache;
		if finished.listed && self.settings.lock().unwrap_or_else(|p| p.into_inner()).semantic {
			if let Err(e) = self.meaning.clone().index(cache.clone()).await {
				log::warn!("could not index mail by meaning: {e}");
			}
			// Turned off meanwhile: what was worked out goes too.
			if !self.settings.lock().unwrap_or_else(|p| p.into_inner()).semantic {
				self.meaning.forget(Some(cache));
			}
		}
		let (limit, meaning) = {
			let s = self.settings.lock().unwrap_or_else(|p| p.into_inner());
			(s.cleanup_mb, s.semantic)
		};
		let usage = cleanup::measure(cache, settings.offline, meaning, &self.meaning);
		let shown = cleanup::offer(usage.total, limit, disk(&self.paths));
		self.post(Box::new(move |app| {
			app.end_work(Word::Fetching);
			app.usage = Some(usage);
			app.cleanup_offer = shown.then_some(usage.total);
		}));
		finished.result
	}
}

/// A conversation's notification id: the same for the same conversation, and never negative.
fn notice_id(key: &str) -> i32 {
	let hash = key.bytes().fold(0x811c_9dc5u32, |h, b| (h ^ u32::from(b)).wrapping_mul(0x0100_0193));
	(hash & 0x7fff_ffff) as i32
}

struct Opened {
	mail: Mail,
	plain: String,
	blocks: Vec<BlockView>,
	notice: Option<String>,
	/// Content-IDs of the pictures drawn in the text, which are not listed as attachments.
	inline: Vec<String>,
}

/// A mail read and turned into blocks, off the app's task.
struct LoadedMail {
	mail: Mail,
	plain: String,
	blocks: Vec<BlockView>,
	pictures: Vec<(u32, Picture)>,
	notice: Option<String>,
	inline: Vec<String>,
}

struct Card {
	summary: MailSummary,
	kind: FolderKind,
	expanded: bool,
	loading: bool,
	error: Option<String>,
	opened: Option<Opened>,
	/// Asked for its body already, so showing it again does not ask twice.
	asked: bool,
}

struct Conv {
	key: String,
	folder: FolderId,
	subject: String,
	cards: Vec<Card>,
	current: MailId,
}

struct Comp {
	id: u64,
	fields: Fields,
	saved: Snapshot,
	addresses: Vec<String>,
	revision: u64,
	show_cc: bool,
	/// Attachments on their way from the mail being forwarded.
	pending: Vec<String>,
	sending: bool,
	saving: bool,
	error: Option<String>,
	tries: u32,
	bad: (bool, bool, bool),
	suggest: Option<(AddressField, Vec<Address>)>,
}

type Snapshot = (String, String, String, String, String, String, usize, usize, Quote);

fn snapshot(f: &Fields) -> Snapshot {
	(f.from.clone(), f.to.clone(), f.cc.clone(), f.bcc.clone(), f.subject.clone(), f.body.clone(), f.attachments.len(), f.kept.len(), f.quote.clone())
}

struct SearchState {
	revision: u64,
	query: String,
	rule: Option<String>,
	/// Browsing a label instead of searching: its own id, and its name.
	label: Option<(String, String)>,
	searching: bool,
	generation: u64,
	rows: Vec<ThreadRow>,
	folders: HashMap<String, FolderId>,
}

struct RuleEd {
	index: Option<usize>,
	revision: u64,
	cursor: u32,
	line: String,
	does: RuleDoes,
	folder: String,
	label: String,
	tone: PreviewTone,
	message: String,
	preview: Vec<PreviewRow>,
	more: Option<String>,
	generation: u64,
	saving: bool,
}

struct SpamSheet {
	own: Vec<MailSummary>,
	revision: u64,
	line: String,
	problem: Option<String>,
	mails: Vec<MailSummary>,
	total: usize,
	picked: HashSet<MailId>,
	keep_rule: bool,
	generation: u64,
}

struct LabelSheet {
	subject: String,
	mails: Vec<MailSummary>,
}

enum Sheet {
	Labels(LabelSheet),
	Spam(SpamSheet),
	Cleanup(Usage),
	Discard,
}

#[derive(Default)]
struct Published {
	shell: Option<ShellView>,
	signin: Option<SignInView>,
	list: Option<ListView>,
	conversation: Option<Option<ConversationView>>,
	composer: Option<Option<ComposerView>>,
	composers: Option<ComposersView>,
	search: Option<SearchView>,
	settings: Option<SettingsView>,
	rules: Option<RulesView>,
	rule_editor: Option<Option<RuleEditorView>>,
	sheet: Option<Option<SheetView>>,
}

pub struct App {
	tx: UnboundedSender<Msg>,
	listener: Arc<dyn EngineListener>,
	secrets: Arc<dyn SecretStore>,
	runtime: tokio::runtime::Handle,
	generation: u64,
	session: Option<Session>,
	stack: Vec<Screen>,
	signin: SignInView,
	settings: Arc<Mutex<Settings>>,
	folders: Vec<Folder>,
	labels: Vec<Label>,
	current: Arc<Mutex<Option<Folder>>>,
	shown: usize,
	rows: Vec<ThreadRow>,
	rows_loaded: bool,
	synced: bool,
	in_flight: Vec<FolderId>,
	fetching_older: bool,
	complete: bool,
	failed: Option<String>,
	list_generation: u64,
	selection: BTreeSet<String>,
	conv: Option<Conv>,
	/// The composer on screen, or being saved; a failed save brings it back.
	comps: Vec<Comp>,
	/// The composer actions are for.
	focus: Option<u64>,
	next_comp: u64,
	/// Links opened before an account was there to answer them from.
	waiting: Vec<Fields>,
	search: Option<SearchState>,
	rules: Vec<Rule>,
	rule_ed: Option<RuleEd>,
	sheet: Option<Sheet>,
	busy: [usize; 8],
	conn: Conn,
	snack: Option<SnackView>,
	snack_retry: bool,
	next_snack: u64,
	usage: Option<Usage>,
	cleanup_offer: Option<u64>,
	semantic_note: Option<String>,
	/// The app is on screen.
	visible: bool,
	background_waiting: Vec<std::sync::mpsc::Sender<BackgroundResult>>,
	background_running: bool,
	resuming: bool,
	/// The session to ask the server for again, for an account not yet connected.
	saved_session: Option<String>,
	/// Clean Up was asked for before anything was measured.
	cleanup_waiting: bool,
	device: &'static str,
	parts: Arc<Mutex<Option<SessionParts>>>,
	retry_hint: &'static str,
	follow_neighbour: bool,
	lone_draft_composes: bool,
	/// An account to open at the start in place of any saved one.
	start_with: Option<Arc<dyn Backend>>,
	paths: Paths,
	meaning: Arc<Meaning>,
	/// A conversation asked for, from a notification, before the account was open.
	open_when_ready: Option<(String, FolderId)>,
	open_dir: PathBuf,
	published: Published,
	revision: u64,
}

fn word_index(word: Word) -> usize {
	WORDS.iter().position(|(w, _)| *w == word).unwrap_or(0)
}

fn mail_ref(id: &MailId) -> MailRef {
	MailRef { list: id.list.clone(), element: id.element.clone() }
}

fn mail_id(r: &MailRef) -> MailId {
	MailId { list: r.list.clone(), element: r.element.clone() }
}

/// Who a conversation's row names: whom it went to, for mail this account wrote.
fn who(m: &MailSummary) -> String {
	match (&m.to, m.outgoing) {
		(Some(to), true) => format!("To: {}", to.display()),
		(None, true) => "(No recipients)".to_string(),
		_ if m.from.display().is_empty() => "(Unknown sender)".to_string(),
		_ => m.from.display().to_string(),
	}
}

fn subject_or_none(subject: &str) -> String {
	if subject.trim().is_empty() {
		"(No subject)".to_string()
	} else {
		subject.to_string()
	}
}

fn initials(name: &str) -> String {
	let mut words = name.split_whitespace().filter(|w| w.chars().next().is_some_and(char::is_alphanumeric));
	let first = words.next().and_then(|w| w.chars().next());
	let last = words.next_back().and_then(|w| w.chars().next());
	first.into_iter().chain(last).flat_map(char::to_uppercase).collect()
}

fn names(list: &[Address]) -> String {
	list.iter().map(|a| a.display().to_string()).collect::<Vec<_>>().join(", ")
}

fn recipients_line(mail: &Mail) -> Option<String> {
	if mail.to.is_empty() && mail.cc.is_empty() && mail.bcc.is_empty() {
		return None;
	}
	let mut s = format!("To {}", names(&mail.to));
	if !mail.cc.is_empty() {
		s.push_str(&format!(" · Cc {}", names(&mail.cc)));
	}
	if !mail.bcc.is_empty() {
		s.push_str(&format!(" · Bcc {}", names(&mail.bcc)));
	}
	Some(s)
}

fn level_label(level: Level) -> String {
	let name = match level {
		Level::Low => "Low",
		Level::Medium => "Medium",
		Level::High => "High",
	};
	format!("{name}: {} per folder, {} opened ahead", level.headers(), level.bodies())
}

fn limit_label(mb: u32) -> String {
	if mb == 0 {
		"Never".to_string()
	} else {
		format::size(u64::from(mb) * 1_000_000)
	}
}

fn storage_line(usage: Usage, device: &str) -> String {
	let can_go = usage.older + usage.vectors + usage.model;
	format!("{} on this {device} · {} can go", format::size(usage.total), format::size(can_go))
}

/// The background checks' pace when turning on notifications turns them on.
const DEFAULT_BACKGROUND: u32 = 30;

fn background_label(minutes: u32) -> String {
	match minutes {
		0 => "Off".to_string(),
		60 => "Every hour".to_string(),
		m => format!("Every {m} minutes"),
	}
}

fn semantic_about(device: &str) -> String {
	format!("Also finds mail about what you type, not only mail with its words. Downloads a {} language model once; your mail never leaves this {device}.", semantic::SIZE)
}

/// The word being typed in an address field: what follows the last separator.
fn current_word(text: &str) -> (usize, &str) {
	let start = text.rfind([',', ';']).map_or(0, |i| i + 1);
	(start, text[start..].trim_start())
}

/// Free and total bytes of the disk the cache is on.
fn disk(paths: &Paths) -> Option<(u64, u64)> {
	let path = paths.mail_cache("x");
	let dir = path.ancestors().find(|p| p.exists())?;
	let dir = std::ffi::CString::new(dir.to_str()?).ok()?;
	let mut info: libc::statvfs = unsafe { std::mem::zeroed() };
	// SAFETY: dir is a valid C string and info lives for the call.
	if unsafe { libc::statvfs(dir.as_ptr(), &mut info) } != 0 {
		return None;
	}
	let block = info.f_frsize as u64;
	Some((info.f_bavail as u64 * block, info.f_blocks as u64 * block))
}

/// A fresh private file for one opened attachment or picture.
fn private_file(dir: &std::path::Path, name: &str, data: &[u8]) -> std::io::Result<PathBuf> {
	use std::os::unix::fs::DirBuilderExt;
	use std::sync::atomic::{AtomicU64, Ordering};
	static NEXT: AtomicU64 = AtomicU64::new(0);
	std::fs::DirBuilder::new().recursive(true).mode(0o700).create(dir)?;
	let one = dir.join(format!("{:x}-{:x}", format::now_ms(), NEXT.fetch_add(1, Ordering::Relaxed)));
	std::fs::DirBuilder::new().mode(0o700).create(&one)?;
	let path = one.join(tern_compose::safe_file_name(name));
	std::fs::write(&path, data)?;
	Ok(path)
}

/// The saved session and its address. `None` only when there really is none.
fn saved_session(secrets: &dyn SecretStore) -> Result<Option<(String, String)>, SecretError> {
	Ok(secrets.get(SESSION_SECRET.into())?.and_then(|raw| serde_json::from_slice(&raw).ok()))
}

/// The key of `email`'s mail cache, made and stored on first use. `None` means this session keeps
/// no cache on disk: a store that cannot answer, or holds something unusable, must not lose the
/// key by having it replaced, and one that cannot save a new key would lose the cache next start.
fn cache_key(secrets: &dyn SecretStore, email: &str) -> Option<[u8; 32]> {
	let name = format!("{CACHE_KEY_SECRET}{email}");
	match secrets.get(name.clone()) {
		Ok(Some(key)) => return <[u8; 32]>::try_from(key.as_slice()).ok(),
		Ok(None) => {},
		Err(_) => return None,
	}
	let mut key = [0u8; 32];
	std::fs::File::open("/dev/urandom").and_then(|mut f| std::io::Read::read_exact(&mut f, &mut key)).ok()?;
	secrets.put(name, key.to_vec()).ok()?;
	Some(key)
}

impl App {
	pub fn new(tx: UnboundedSender<Msg>, listener: Arc<dyn EngineListener>, secrets: Arc<dyn SecretStore>, runtime: tokio::runtime::Handle, paths: Paths, options: Options) -> App {
		App {
			device: options.device,
			parts: Arc::default(),
			retry_hint: options.retry_hint,
			follow_neighbour: options.follow_neighbour,
			lone_draft_composes: options.lone_draft_composes,
			start_with: options.signed_in,
			settings: Arc::new(Mutex::new(Settings::load(&paths))),
			meaning: options.meaning.unwrap_or_else(|| Meaning::new(&paths)),
			open_dir: paths.opened_dir(),
			paths,
			tx,
			listener,
			secrets,
			runtime,
			generation: 0,
			session: None,
			stack: vec![Screen::Starting],
			signin: SignInView { email: String::new(), busy: false, error: None },
			folders: Vec::new(),
			labels: Vec::new(),
			current: Arc::default(),
			shown: ROWS,
			rows: Vec::new(),
			rows_loaded: false,
			synced: false,
			in_flight: Vec::new(),
			fetching_older: false,
			complete: true,
			failed: None,
			list_generation: 0,
			selection: BTreeSet::new(),
			conv: None,
			comps: Vec::new(),
			focus: None,
			next_comp: 1,
			waiting: Vec::new(),
			search: None,
			rules: Vec::new(),
			rule_ed: None,
			sheet: None,
			busy: [0; 8],
			conn: Conn::Fine,
			snack: None,
			snack_retry: false,
			next_snack: 1,
			usage: None,
			cleanup_offer: None,
			semantic_note: None,
			visible: false,
			background_waiting: Vec::new(),
			background_running: false,
			resuming: false,
			saved_session: None,
			cleanup_waiting: false,
			open_when_ready: None,
			published: Published::default(),
			revision: 1,
		}
	}

	pub fn parts_handle(&self) -> Arc<Mutex<Option<SessionParts>>> {
		self.parts.clone()
	}

	pub fn settings_handle(&self) -> Arc<Mutex<Settings>> {
		self.settings.clone()
	}

	/// Ends everything under way, leaving what is saved as it is.
	pub fn stop(&mut self) {
		self.reset_account();
	}

	pub fn sync_now(&self) {
		if let Some(session) = &self.session {
			session.wake.notify_one();
		}
	}

	/// Brings the folders of these kinds up to date, as after sending or saving a draft.
	pub fn sync_kinds(&mut self, kinds: &[FolderKind]) {
		self.folder_changed(None);
		let ids: Vec<FolderId> = self.folders.iter().filter(|f| kinds.contains(&f.kind)).map(|f| f.id.clone()).collect();
		for id in ids {
			self.sync_folder(&id, false);
		}
	}

	/// The open conversation has left the list on show: its neighbour opens, where the two are on
	/// screen together. `before` is the list as it was.
	fn follow_on(&mut self, before: &[String]) {
		if !self.follow_neighbour {
			return;
		}
		let Some(key) = self.conv.as_ref().map(|c| c.key.clone()) else { return };
		let rows = self.search.as_ref().map_or(&self.rows, |s| &s.rows);
		if rows.iter().any(|r| r.key == key) {
			return;
		}
		let Some(at) = before.iter().position(|k| *k == key) else { return };
		let Some(next) = rows.get(at.min(rows.len().saturating_sub(1))).map(|r| r.key.clone()) else {
			self.conv = None;
			return self.remove_screen(Screen::Conversation);
		};
		let found = self.search.as_ref().and_then(|s| s.folders.get(&next).cloned());
		let here = self.current.lock().unwrap_or_else(|p| p.into_inner()).as_ref().map(|f| f.id.clone());
		if let Some(folder) = found.or(here) {
			self.open_thread(next, folder);
		}
	}

	/// Keeps what tests look behind the views at up to date.
	fn share_session(&self) {
		*self.parts.lock().unwrap_or_else(|p| p.into_inner()) = self.session.as_ref().map(|s| SessionParts { backend: s.backend.clone(), cache: s.cache.clone() });
	}

	fn settings(&self) -> Settings {
		*self.settings.lock().unwrap_or_else(|p| p.into_inner())
	}

	fn change_settings(&mut self, change: impl FnOnce(&mut Settings)) {
		let mut settings = self.settings();
		change(&mut settings);
		*self.settings.lock().unwrap_or_else(|p| p.into_inner()) = settings;
		settings.save(&self.paths);
		if let Some(session) = &self.session {
			session.wake.notify_one();
		}
	}

	fn next_revision(&mut self) -> u64 {
		self.revision += 1;
		self.revision
	}

	/// Runs `work` off this task; what it returns is applied here, unless the account has gone.
	fn task<A: FnOnce(&mut App) + Send + 'static>(&self, work: impl Future<Output = A> + Send + 'static) {
		let (tx, generation) = (self.tx.clone(), self.generation);
		self.runtime.spawn(async move {
			let apply = work.await;
			let _ = tx.send(Msg::Done(generation, Box::new(apply)));
		});
	}

	fn start_work(&mut self, word: Word) {
		self.busy[word_index(word)] += 1;
	}

	fn end_work(&mut self, word: Word) {
		let at = word_index(word);
		self.busy[at] = self.busy[at].saturating_sub(1);
	}

	fn say(&mut self, text: impl Into<String>) {
		self.snack = Some(SnackView { id: self.next_snack, text: text.into(), action: None });
		self.next_snack += 1;
		self.snack_retry = false;
	}

	fn say_retry(&mut self, text: impl Into<String>) {
		self.snack = Some(SnackView { id: self.next_snack, text: text.into(), action: Some("Retry".into()) });
		self.next_snack += 1;
		self.snack_retry = true;
	}

	/// Takes what a failed call says about the connection, and tells the person.
	fn failed_call(&mut self, e: &Error) {
		self.note_error(e);
		match e {
			Error::Offline => self.say_retry(e.to_string()),
			Error::SessionExpired => {},
			_ => self.say(e.to_string()),
		}
	}

	fn note_error(&mut self, e: &Error) {
		match e {
			Error::Offline => self.conn = Conn::Offline,
			Error::RateLimited(_) => self.conn = Conn::Busy,
			Error::SessionExpired => self.conn = Conn::SignedOut,
			_ => {},
		}
	}

	fn note_fine(&mut self) {
		if self.conn != Conn::SignedOut {
			self.conn = Conn::Fine;
		}
	}

	fn top(&self) -> Screen {
		self.stack.last().copied().unwrap_or(Screen::Starting)
	}

	fn push(&mut self, screen: Screen) {
		if self.top() != screen {
			self.stack.push(screen);
		}
	}

	fn remove_screen(&mut self, screen: Screen) {
		self.stack.retain(|s| *s != screen);
		if self.stack.is_empty() {
			self.stack.push(if self.session.is_some() { Screen::Inbox } else { Screen::SignIn });
		}
	}

	// ---- Starting and accounts

	pub fn start(&mut self) {
		if let Err(e) = std::fs::remove_dir_all(&self.open_dir) {
			if e.kind() != std::io::ErrorKind::NotFound {
				log::warn!("could not clear opened files: {e}");
			}
		}
		if let Some(backend) = self.start_with.take() {
			let email = backend.account().address;
			self.enter(email, backend, Cache::in_memory(), false, true);
			return self.publish();
		}
		let (secrets, paths) = (self.secrets.clone(), self.paths.clone());
		self.task(async move {
			// The platform may take its time over secrets, and the cache over its first read.
			let found = tokio::task::spawn_blocking(move || {
				let saved = saved_session(secrets.as_ref())?;
				Ok::<_, SecretError>(saved.map(|(email, session)| {
					let cache = cache_key(secrets.as_ref(), &email).and_then(|key| Cache::open(&paths.mail_cache(&email), &key).ok());
					(email, session, cache)
				}))
			})
			.await
			.unwrap_or(Err(SecretError::Unavailable));
			move |app: &mut App| match found {
				Ok(Some((email, session, cache))) => {
					app.signin.email = email.clone();
					app.saved_session = Some(session.clone());
					let backend: Arc<dyn Backend> = Arc::new(NoServer { address: email.clone() });
					app.enter(email, backend, cache.unwrap_or_else(Cache::in_memory), true, false);
					app.resume(session);
				},
				Ok(None) => {
					app.stack = vec![Screen::SignIn];
					app.run_background();
				},
				// Still saved, as far as anyone knows: nothing is forgotten over this.
				Err(_) => {
					app.signin.error = Some("Could not read the saved sign-in. Try again, or sign in.".into());
					app.stack = vec![Screen::SignIn];
					app.run_background();
				},
			}
		});
		self.publish();
	}

	/// Asks the server to take the saved session back.
	fn resume(&mut self, saved: String) {
		let again = saved.clone();
		self.resuming = true;
		let sdk_dir = self.paths.sdk_dir();
		self.task(async move {
			let result = tern_tuta::resume(&saved, sdk_dir).await;
			move |app: &mut App| match result {
				Ok(backend) => {
					app.resuming = false;
					app.connected(backend);
				},
				Err(Error::SessionExpired) => {
					app.resuming = false;
					app.conn = Conn::SignedOut;
					app.run_background();
				},
				Err(e) => {
					app.resuming = false;
					app.note_error(&e);
					// A check that woke the app for this tries again on its next turn.
					let retry = BackgroundResult { notices: Vec::new(), summary: None, retry: true };
					app.answer_background(retry);
					// Not here yet: asked again in a while, or as soon as the network returns.
					app.task(async move {
						tokio::time::sleep(RESUME_EVERY).await;
						move |app: &mut App| app.resume_again(again)
					});
				},
			}
		});
	}

	fn resume_again(&mut self, saved: String) {
		if self.session.as_ref().is_some_and(|s| !s.connected) {
			self.resume(saved);
		}
	}

	/// Asks at once for what a saved session could not reach before.
	fn resume_now(&mut self) {
		if !self.resuming && self.session.as_ref().is_some_and(|s| !s.connected) {
			if let Some(session) = self.saved_session.clone() {
				self.resume(session);
			}
		}
	}

	/// The server is reachable for this session: real work can start.
	fn connected(&mut self, backend: Arc<dyn Backend>) {
		let Some(session) = self.session.as_mut() else { return };
		if session.connected {
			return;
		}
		session.fetcher.stop();
		session.backend = backend.clone();
		session.fetcher = Fetcher::new(backend, session.cache.clone(), self.runtime.clone(), Tuning::DEFAULT);
		session.connected = true;
		self.note_fine();
		self.share_session();
		self.start_sync();
		self.run_background();
	}

	/// Opens the mailbox for an account. The lists show from the cache straight away.
	fn enter(&mut self, email: String, backend: Arc<dyn Backend>, cache: Cache, persistent: bool, connected: bool) {
		self.enter_with(email, backend, cache, persistent, connected, None);
	}

	fn enter_with(&mut self, email: String, backend: Arc<dyn Backend>, cache: Cache, persistent: bool, connected: bool, demo: Option<Arc<DemoBackend>>) {
		let cache = Arc::new(cache);
		let fetcher = Fetcher::new(backend.clone(), cache.clone(), self.runtime.clone(), Tuning::DEFAULT);
		self.session = Some(Session {
			email,
			backend,
			cache: cache.clone(),
			fetcher,
			persistent,
			connected,
			wake: Arc::default(),
			last_round: Arc::default(),
			contacts_done: Arc::default(),
			demo,
			sync: None,
		});
		self.stack = vec![Screen::Inbox];
		self.signin.busy = false;
		self.signin.error = None;
		self.rules = rules::load(&self.paths);
		self.share_session();
		match cache.folders() {
			Ok(folders) => self.show_folders(folders),
			Err(e) => log::warn!("could not read folders: {e}"),
		}
		if connected {
			self.start_sync();
		}
		for fields in std::mem::take(&mut self.waiting) {
			self.open_composer(fields);
		}
		if let Some((key, folder)) = self.open_when_ready.take() {
			self.open_thread(key, folder);
		}
		self.run_background();
	}

	fn sign_in(&mut self, email: String, password: String) {
		let email = email.trim().to_string();
		self.signin.error = None;
		self.signin.email = email.clone();
		if email.eq_ignore_ascii_case(DEMO_LOGIN) {
			self.reset_account();
			let demo = Arc::new(DemoBackend::new());
			self.enter_with(email, demo.clone(), Cache::in_memory(), false, true, Some(demo));
			return;
		}
		if email.is_empty() || password.is_empty() {
			self.signin.error = Some("Enter your email address and password".into());
			return;
		}
		self.signin.busy = true;
		let (secrets, paths) = (self.secrets.clone(), self.paths.clone());
		self.task(async move {
			let result = tern_tuta::login(&email, &password, paths.sdk_dir()).await;
			let opened = match &result {
				Ok((_, session)) => {
					let (email, session, paths) = (email.clone(), session.clone(), paths.clone());
					tokio::task::spawn_blocking(move || {
						if let Ok(raw) = serde_json::to_vec(&(&email, &session)) {
							// Not remembered is not fatal: the next start asks again.
							let _ = secrets.put(SESSION_SECRET.into(), raw);
						}
						cache_key(secrets.as_ref(), &email).and_then(|key| Cache::open(&paths.mail_cache(&email), &key).ok())
					})
					.await
					.unwrap_or(None)
				},
				Err(_) => None,
			};
			move |app: &mut App| match result {
				Ok((backend, session)) => {
					app.reset_account();
					app.saved_session = Some(session);
					app.enter(email, backend, opened.unwrap_or_else(Cache::in_memory), true, true);
				},
				Err(e) => {
					app.signin.busy = false;
					app.signin.error = Some(e.to_string());
					app.note_error(&e);
				},
			}
		});
	}

	/// Forgets everything of the account on show; later results of its work are dropped.
	fn reset_account(&mut self) {
		self.generation += 1;
		if let Some(session) = self.session.take() {
			session.fetcher.stop();
			self.share_session();
		}
		self.folders.clear();
		self.labels.clear();
		*self.current.lock().unwrap_or_else(|p| p.into_inner()) = None;
		self.shown = ROWS;
		self.rows.clear();
		self.rows_loaded = false;
		self.synced = false;
		self.in_flight.clear();
		self.fetching_older = false;
		self.complete = true;
		self.failed = None;
		self.selection.clear();
		self.conv = None;
		self.comps.clear();
		self.focus = None;
		self.search = None;
		self.rule_ed = None;
		self.sheet = None;
		self.busy = [0; 8];
		self.conn = Conn::Fine;
		self.snack = None;
		self.usage = None;
		self.cleanup_offer = None;
		self.background_running = false;
		self.resuming = false;
		self.saved_session = None;
		self.answer_background(BackgroundResult { notices: Vec::new(), summary: None, retry: false });
	}

	fn sign_out(&mut self) {
		let Some(session) = self.session.as_ref() else { return };
		let (backend, email, persistent) = (session.backend.clone(), session.email.clone(), session.persistent);
		let forget = self.settings().forget_on_sign_out;
		self.signin.email = email.clone();
		self.reset_account();
		self.stack = vec![Screen::SignIn];
		let (secrets, paths) = (self.secrets.clone(), self.paths.clone());
		self.runtime.spawn(async move {
			let _ = tokio::task::spawn_blocking(move || {
				secrets.remove(SESSION_SECRET.into());
				if persistent && forget {
					secrets.remove(format!("{CACHE_KEY_SECRET}{email}"));
					let path = paths.mail_cache(&email);
					for suffix in ["", "-wal", "-shm"] {
						let _ = std::fs::remove_file(format!("{}{suffix}", path.display()));
					}
				}
			})
			.await;
			let _ = backend.sign_out().await;
		});
	}

	// ---- Keeping the cache up to date

	fn round(&self) -> Option<Round> {
		let session = self.session.as_ref()?;
		Some(Round {
			paths: self.paths.clone(),
			meaning: self.meaning.clone(),
			backend: session.backend.clone(),
			cache: session.cache.clone(),
			fetcher: session.fetcher.clone(),
			settings: self.settings.clone(),
			current: self.current.clone(),
			tx: self.tx.clone(),
			generation: self.generation,
			last: session.last_round.clone(),
			contacts_done: session.contacts_done.clone(),
		})
	}

	/// The sync's rounds for this session, at once and then every few minutes or when woken.
	fn start_sync(&mut self) {
		let Some(round) = self.round() else { return };
		let Some(wake) = self.session.as_ref().map(|s| s.wake.clone()) else { return };
		let task = self.runtime.spawn(async move {
			loop {
				let _ = round.run().await;
				tokio::select! {
					_ = tokio::time::sleep(SYNC_EVERY) => {},
					_ = wake.notified() => {},
				}
			}
		});
		if let Some(session) = self.session.as_mut() {
			session.sync = Some(Stops(task.abort_handle()));
		}
	}

	// ---- Background checks and what the system says about new mail

	/// The system woke the app to check for mail. Answered once a round has run, or at once if
	/// there is no account to check. A saved session is resumed first, as the process may be new.
	pub fn background_check(&mut self, reply: std::sync::mpsc::Sender<BackgroundResult>) {
		self.background_waiting.push(reply);
		self.run_background();
	}

	fn answer_background(&mut self, result: BackgroundResult) {
		for reply in self.background_waiting.drain(..) {
			let _ = reply.send(result.clone());
		}
	}

	fn run_background(&mut self) {
		if self.background_waiting.is_empty() {
			return;
		}
		let none = BackgroundResult { notices: Vec::new(), summary: None, retry: false };
		let Some(session) = self.session.as_ref() else {
			// Still finding out whether there is an account.
			if self.top() != Screen::Starting {
				self.answer_background(none);
			}
			return;
		};
		if !session.connected {
			if self.conn == Conn::SignedOut {
				return self.answer_background(none);
			}
			if !self.resuming {
				self.resume_now();
			}
			// Answered when the resume comes back: `connected` or `resume_failed`.
			return;
		}
		if self.background_running {
			return;
		}
		self.background_running = true;
		let demo = session.demo.is_some();
		if let Some(demo) = &session.demo {
			// The demo account has no server to bring news, so its mail arrives here.
			demo.deliver(("Sam Okafor", "sam@example.net"), "Table booked for 12:30", "<p>All set, see you there.</p>");
		}
		let Some(round) = self.round() else { return };
		self.task(async move {
			let result = if demo { round.run().await } else { round.run_unless_just_done().await };
			move |app: &mut App| {
				app.background_running = false;
				let retry = result.is_err();
				let (notices, summary) = app.new_mail_notices();
				app.folder_changed(None);
				app.answer_background(BackgroundResult { notices, summary, retry });
			}
		});
	}

	fn inbox_id(&self) -> Option<FolderId> {
		let cache = &self.session.as_ref()?.cache;
		cache.folders().ok()?.into_iter().find(|f| f.kind == FolderKind::Inbox).map(|f| f.id)
	}

	/// Moves what counts as new past everything in the Inbox now: it has been seen, or nobody asked
	/// to hear about it.
	fn mark_inbox_seen(&mut self) {
		let (Some(session), Some(inbox)) = (self.session.as_ref(), self.inbox_id()) else { return };
		if let Some(newest) = session.cache.list(&inbox, 0, 1).ok().and_then(|mails| mails.first().map(|m| m.received_ms)) {
			let _ = session.cache.set_mark(NOTIFIED, newest);
		}
	}

	/// The conversations with mail new since the last look, when the system should say so. The
	/// first look says nothing: the mail already there is not news.
	fn new_mail_notices(&mut self) -> (Vec<MailNotice>, Option<String>) {
		let (Some(session), Some(inbox)) = (self.session.as_ref(), self.inbox_id()) else { return (Vec::new(), None) };
		let Some(after) = session.cache.mark(NOTIFIED).ok().flatten() else {
			self.mark_inbox_seen();
			return (Vec::new(), None);
		};
		if self.visible || !self.settings().notifications {
			self.mark_inbox_seen();
			return (Vec::new(), None);
		}
		let new: Vec<MailSummary> = session.cache.received_after(&inbox, after).unwrap_or_default();
		let newest = new.iter().map(|m| m.received_ms).max();
		// Oldest first as they came, so the latest of a conversation is what stands for it.
		let mut by_conversation: Vec<(String, MailSummary)> = Vec::new();
		for mail in new.into_iter().filter(|m| m.unread && !m.outgoing && !m.draft) {
			let key = session.cache.thread_key_of(&mail.id).ok().flatten().unwrap_or_else(|| mail.id.element.clone());
			by_conversation.retain(|(k, _)| k != &key);
			by_conversation.push((key, mail));
		}
		if let Some(newest) = newest {
			let _ = session.cache.set_mark(NOTIFIED, newest);
		}
		let total = by_conversation.len();
		let notices = by_conversation
			.into_iter()
			.rev()
			.take(MAX_NOTICES)
			.map(|(key, mail)| MailNotice { id: notice_id(&key), key, folder_id: inbox.0.clone(), who: who(&mail), subject: subject_or_none(&mail.subject), received_ms: mail.received_ms })
			.collect();
		let summary = (total > 1).then(|| format!("{total} new conversations"));
		(notices, summary)
	}

	fn sync_failed(&mut self, e: Error) {
		self.note_error(&e);
		if self.folders.is_empty() {
			self.failed = Some(e.to_string());
		}
	}

	fn show_folders(&mut self, folders: Vec<Folder>) {
		self.folders = folders;
		let current = self.current.lock().unwrap_or_else(|p| p.into_inner()).clone();
		let again = current.as_ref().and_then(|c| self.folders.iter().find(|f| f.id == c.id).cloned());
		match again {
			Some(folder) => {
				*self.current.lock().unwrap_or_else(|p| p.into_inner()) = Some(folder);
				self.reload_list();
			},
			None => {
				let inbox = self.folders.iter().find(|f| f.kind == FolderKind::Inbox).or_else(|| self.folders.first()).cloned();
				if let Some(folder) = inbox {
					self.open_folder(folder);
				}
			},
		}
		if !self.folders.is_empty() {
			self.failed = None;
		}
	}

	fn open_folder(&mut self, folder: Folder) {
		let same = self.current.lock().unwrap_or_else(|p| p.into_inner()).as_ref().is_some_and(|c| c.id == folder.id);
		let id = folder.id.clone();
		*self.current.lock().unwrap_or_else(|p| p.into_inner()) = Some(folder);
		if !same {
			self.shown = ROWS;
			self.rows.clear();
			self.rows_loaded = false;
			self.selection.clear();
		}
		self.reload_list();
		self.sync_folder(&id, false);
	}

	/// Mail in `folder` changed, or anywhere when None: shows what is there now.
	fn folder_changed(&mut self, folder: Option<FolderId>) {
		if let Some(session) = &self.session {
			if let Ok(folders) = session.cache.folders() {
				self.folders = folders;
				let current = self.current.lock().unwrap_or_else(|p| p.into_inner()).as_ref().map(|c| c.id.clone());
				let found = current.and_then(|id| self.folders.iter().find(|f| f.id == id).cloned());
				if let Some(found) = found {
					*self.current.lock().unwrap_or_else(|p| p.into_inner()) = Some(found);
				}
			}
		}
		let on_show = self.current.lock().unwrap_or_else(|p| p.into_inner()).as_ref().map(|f| f.id.clone());
		if folder.is_none() || folder == on_show {
			self.reload_list();
		}
		if self.search.is_some() {
			self.run_search();
		}
		self.refresh_conversation();
	}

	fn reload_list(&mut self) {
		let (Some(session), Some(folder)) = (self.session.as_ref(), self.current.lock().unwrap_or_else(|p| p.into_inner()).clone()) else { return };
		self.list_generation += 1;
		let generation = self.list_generation;
		let (cache, shown) = (session.cache.clone(), self.shown);
		let id = folder.id;
		self.task(async move {
			let rows = cache.threads(&id, shown).unwrap_or_default();
			let state = cache.folder_state(&id).unwrap_or_default();
			move |app: &mut App| {
				if app.list_generation != generation {
					return;
				}
				let before: Vec<String> = app.rows.iter().map(|r| r.key.clone()).collect();
				app.rows = rows;
				app.rows_loaded = true;
				app.synced = state.synced_ms > 0;
				app.complete = state.complete;
				let keys: HashSet<&String> = app.rows.iter().map(|r| &r.key).collect();
				app.selection.retain(|k| keys.contains(k));
				if let Some(session) = &app.session {
					session.fetcher.want(Want::Viewing(app.rows.iter().take(VIEWING).map(|r| r.latest.id.clone()).collect()));
				}
				if app.search.is_none() {
					app.follow_on(&before);
				}
			}
		});
	}

	/// Brings `folder` up to date now, ahead of the sync. `told` says the person asked and waits.
	fn sync_folder(&mut self, folder: &FolderId, told: bool) {
		let Some(session) = self.session.as_ref() else { return };
		if !session.connected {
			if told {
				self.resume_now();
				let e = Error::Offline;
				self.failed_call(&e);
			}
			return;
		}
		if self.in_flight.contains(folder) {
			return;
		}
		let (backend, cache) = (session.backend.clone(), session.cache.clone());
		let keep = self.settings().offline.headers();
		self.in_flight.push(folder.clone());
		self.start_work(Word::Fetching);
		let id = folder.clone();
		self.task(async move {
			let result = tern_sync::folder(backend.as_ref(), &cache, &id, keep).await;
			move |app: &mut App| {
				app.end_work(Word::Fetching);
				app.in_flight.retain(|f| f != &id);
				match result {
					Ok(_) => {
						app.note_fine();
						app.failed = None;
						app.folder_changed(Some(id));
					},
					Err(e) => {
						let on_show = app.current.lock().unwrap_or_else(|p| p.into_inner()).as_ref().map(|f| f.id.clone()) == Some(id);
						if told {
							app.failed_call(&e);
						} else {
							app.note_error(&e);
						}
						if on_show && app.rows.is_empty() {
							app.failed = Some(e.to_string());
						}
					},
				}
			}
		});
	}

	fn refresh(&mut self) {
		if let Some(session) = &self.session {
			session.wake.notify_one();
		}
		self.resume_now();
		let current = self.current.lock().unwrap_or_else(|p| p.into_inner()).as_ref().map(|f| f.id.clone());
		if let Some(id) = current {
			self.sync_folder(&id, true);
		}
	}

	fn load_more(&mut self) {
		let (Some(session), Some(folder)) = (self.session.as_ref(), self.current.lock().unwrap_or_else(|p| p.into_inner()).clone()) else { return };
		if self.rows.len() >= self.shown {
			self.shown += ROWS;
			return self.reload_list();
		}
		if self.fetching_older || self.complete || !session.connected {
			return;
		}
		let (backend, cache) = (session.backend.clone(), session.cache.clone());
		self.fetching_older = true;
		self.start_work(Word::Fetching);
		let id = folder.id;
		self.task(async move {
			let result = tern_sync::older(backend.as_ref(), &cache, &id).await;
			move |app: &mut App| {
				app.fetching_older = false;
				app.end_work(Word::Fetching);
				match result {
					Ok(_) => {
						app.shown += ROWS;
						app.folder_changed(Some(id));
					},
					Err(e) => app.failed_call(&e),
				}
			}
		});
	}

	// ---- Reading

	fn folder_kind(&self, id: &FolderId) -> FolderKind {
		self.folders.iter().find(|f| &f.id == id).map_or(FolderKind::Custom, |f| f.kind)
	}

	fn open_thread(&mut self, key: String, folder: FolderId) {
		let Some(session) = self.session.as_ref() else { return };
		let row = self.rows.iter().find(|r| r.key == key).cloned().or_else(|| self.search.as_ref().and_then(|s| s.rows.iter().find(|r| r.key == key).cloned()));
		let mut mails = session.cache.thread(&key, &folder).unwrap_or_default();
		if mails.is_empty() {
			let Some(row) = &row else { return };
			mails.push(tern_cache::ThreadMail { summary: row.latest.clone(), folder: self.folder_kind(&folder), folder_id: folder.clone() });
		}
		// A lone draft goes straight back to being written.
		if self.lone_draft_composes && mails.len() == 1 && mails[0].summary.draft {
			return self.edit_draft(mails[0].summary.id.clone());
		}
		mails.reverse();
		let focus = row.map_or_else(|| mails[0].summary.id.clone(), |r| r.latest.id);
		let subject = subject_or_none(&mails[0].summary.subject);
		session.fetcher.want(Want::Reading { thread: mails.iter().map(|m| m.summary.id.clone()).collect(), focus: focus.clone() });
		let cards = mails
			.into_iter()
			.map(|m| {
				let expanded = m.summary.id == focus || m.summary.unread;
				Card { summary: m.summary, kind: m.folder, expanded, loading: expanded, error: None, opened: None, asked: false }
			})
			.collect();
		self.conv = Some(Conv { key, folder, subject, cards, current: focus });
		self.selection.clear();
		self.push(Screen::Conversation);
		self.load_expanded();
	}

	fn load_expanded(&mut self) {
		let Some(conv) = self.conv.as_mut() else { return };
		let ids: Vec<MailId> = conv
			.cards
			.iter_mut()
			.filter(|c| c.expanded && !c.asked)
			.map(|c| {
				c.asked = true;
				c.summary.id.clone()
			})
			.collect();
		for id in ids {
			self.load_card(id);
		}
	}

	fn load_card(&mut self, id: MailId) {
		let Some(session) = self.session.as_ref() else { return };
		let fetcher = session.fetcher.clone();
		// Mail from the cache opens at once; only a wait on the server says so.
		self.start_work(Word::Opening);
		self.task(async move {
			let result = fetcher.mail(&id).await;
			let opened = result.map(|mail| {
				let doc = Doc::parse(&mail.body_html);
				let notice = doc.is_complex().then(|| "Some of this mail only shows in Tuta’s web app.".to_string());
				let (blocks, pictures) = flatten(&doc);
				LoadedMail { plain: doc.to_plain_text(), inline: doc.cids(), mail, blocks, pictures, notice }
			});
			move |app: &mut App| {
				app.end_work(Word::Opening);
				app.card_loaded(id, opened);
			}
		});
	}

	fn card_loaded(&mut self, id: MailId, result: CoreResult<LoadedMail>) {
		let mut mark_read = None;
		let mut pictures_to_get = Vec::new();
		let mut failed = None;
		if let Some(card) = self.conv.as_mut().and_then(|c| c.cards.iter_mut().find(|c| c.summary.id == id)) {
			card.loading = false;
			match result {
				Ok(loaded) => {
					let attachments = loaded.mail.attachments.clone();
					card.opened = Some(Opened { mail: loaded.mail, plain: loaded.plain, blocks: loaded.blocks, notice: loaded.notice, inline: loaded.inline });
					card.error = None;
					if card.summary.unread {
						mark_read = Some(card.summary.clone());
						card.summary.unread = false;
					}
					for (block, picture) in loaded.pictures {
						match picture {
							Picture::Data(data) => pictures_to_get.push((block, None, Some(data))),
							Picture::Cid(cid) => {
								if let Some(att) = attachments.iter().find(|a| a.cid.as_deref().map(|c| c.trim_matches(['<', '>'])) == Some(cid.as_str())) {
									pictures_to_get.push((block, Some(att.clone()), None));
								}
							},
						}
					}
				},
				Err(e) => {
					card.error = Some(e.to_string());
					card.asked = false;
					failed = Some(e);
				},
			}
		}
		if let Some(e) = failed {
			self.note_error(&e);
		}
		for (block, att, data) in pictures_to_get {
			self.fetch_picture(id.clone(), block, att, data);
		}
		if let Some(summary) = mark_read {
			self.set_unread(&summary, false);
		}
	}

	fn fetch_picture(&mut self, mail: MailId, block: u32, att: Option<Attachment>, data: Option<Vec<u8>>) {
		let Some(session) = self.session.as_ref() else { return };
		let (backend, dir) = (session.backend.clone(), self.open_dir.clone());
		self.task(async move {
			let bytes = match (data, &att) {
				(Some(data), _) => Some(data),
				(None, Some(att)) => backend.attachment(&mail, att).await.ok(),
				_ => None,
			};
			let name = att.as_ref().map_or("image", |a| a.name.as_str()).to_string();
			let path = bytes.and_then(|bytes| private_file(&dir, &name, &bytes).ok());
			move |app: &mut App| {
				let Some(path) = path else { return };
				let card = app.conv.as_mut().and_then(|c| c.cards.iter_mut().find(|c| c.summary.id == mail));
				let Some(opened) = card.and_then(|c| c.opened.as_mut()) else { return };
				if let Some(BlockView { kind: BlockKind::Image { path: slot, .. }, .. }) = opened.blocks.get_mut(block as usize) {
					*slot = Some(path.display().to_string());
				}
			}
		});
	}

	/// The conversation on show may have changed: new mail, or mail gone from it.
	fn refresh_conversation(&mut self) {
		let (Some(session), Some(conv)) = (self.session.as_ref(), self.conv.as_mut()) else { return };
		let mut mails = session.cache.thread(&conv.key, &conv.folder).unwrap_or_default();
		mails.reverse();
		if mails.is_empty() {
			// Left to the list changing, which says where to go next.
			if self.follow_neighbour {
				return;
			}
			self.conv = None;
			return self.remove_screen(Screen::Conversation);
		}
		let same = mails.len() == conv.cards.len() && mails.iter().zip(&conv.cards).all(|(a, b)| a.summary == b.summary);
		if same {
			return;
		}
		let mut before: HashMap<MailId, Card> = conv.cards.drain(..).map(|c| (c.summary.id.clone(), c)).collect();
		conv.cards = mails
			.into_iter()
			.map(|m| match before.remove(&m.summary.id) {
				Some(mut card) => {
					card.summary = m.summary;
					card.kind = m.folder;
					card
				},
				None => {
					let expanded = m.summary.unread;
					Card { summary: m.summary, kind: m.folder, expanded, loading: expanded, error: None, opened: None, asked: false }
				},
			})
			.collect();
		self.load_expanded();
	}

	fn toggle_card(&mut self, id: MailId) {
		let Some(conv) = self.conv.as_mut() else { return };
		let Some(card) = conv.cards.iter_mut().find(|c| c.summary.id == id) else { return };
		card.expanded = !card.expanded;
		if card.expanded {
			conv.current = id;
			if card.opened.is_none() {
				card.loading = true;
			}
		}
		self.load_expanded();
	}

	fn toggle_pick(&mut self, id: MailId, block: u32) {
		let Some(conv) = self.conv.as_mut() else { return };
		conv.current = id.clone();
		let card = conv.cards.iter_mut().find(|c| c.summary.id == id);
		if let Some(b) = card.and_then(|c| c.opened.as_mut()).and_then(|o| o.blocks.get_mut(block as usize)) {
			b.picked = !b.picked;
		}
	}

	fn set_unread(&mut self, summary: &MailSummary, unread: bool) {
		let Some(session) = self.session.as_ref() else { return };
		let (backend, cache, id) = (session.backend.clone(), session.cache.clone(), summary.id.clone());
		let _ = cache.set_unread(&id, unread);
		self.folder_changed(None);
		self.task(async move {
			let result = backend.set_unread(&id, unread).await;
			if result.is_err() {
				let _ = cache.set_unread(&id, !unread);
			}
			move |app: &mut App| {
				if let Err(e) = result {
					app.note_error(&e);
					app.folder_changed(None);
				}
			}
		});
	}

	/// The mails of conversation `key` in `folder`, oldest first.
	fn thread_here(&self, key: &str, folder: &FolderId) -> Vec<MailSummary> {
		let Some(session) = self.session.as_ref() else { return Vec::new() };
		session.cache.thread(key, folder).unwrap_or_default().into_iter().filter(|m| &m.folder_id == folder).map(|m| m.summary).collect()
	}

	fn current_folder(&self) -> Option<Folder> {
		self.current.lock().unwrap_or_else(|p| p.into_inner()).clone()
	}

	/// Moves `ids` to a folder at once here, and on the server behind it.
	fn move_away(&mut self, ids: Vec<MailId>, kind: FolderKind, message: String) {
		let Some(session) = self.session.as_ref() else { return };
		if ids.is_empty() {
			return;
		}
		let target = self.folders.iter().find(|f| f.kind == kind).map(|f| f.id.clone());
		let (backend, cache) = (session.backend.clone(), session.cache.clone());
		for id in &ids {
			let _ = match &target {
				Some(t) => cache.move_to(id, t),
				None => cache.remove(id),
			};
		}
		let word = if kind == FolderKind::Trash { Word::Deleting } else { Word::Moving };
		self.start_work(word);
		self.folder_changed(None);
		self.task(async move {
			let result = match (kind, &target) {
				(FolderKind::Trash, _) => {
					let mut result = Ok(());
					for id in &ids {
						result = backend.trash(id).await;
						if result.is_err() {
							break;
						}
					}
					result
				},
				(_, Some(target)) => backend.move_mails(&ids, target).await,
				(_, None) => Err(Error::Other("This account has no such folder".into())),
			};
			move |app: &mut App| {
				app.end_work(word);
				match result {
					Ok(()) => {
						app.note_fine();
						app.say(message);
					},
					Err(e) => {
						app.failed_call(&e);
						app.refresh();
					},
				}
			}
		});
	}

	fn conversation_away(&mut self, kind: FolderKind, one: &str, many: &str) {
		let Some(conv) = self.conv.as_ref() else { return };
		let (key, folder) = (conv.key.clone(), conv.folder.clone());
		let ids: Vec<MailId> = self.thread_here(&key, &folder).into_iter().map(|m| m.id).collect();
		let message = if ids.len() > 1 { many } else { one }.to_string();
		if !self.follow_neighbour {
			self.conv = None;
			self.remove_screen(Screen::Conversation);
		}
		self.move_away(ids, kind, message);
	}

	fn conversation_unread(&mut self) {
		let Some(conv) = self.conv.as_ref() else { return };
		let current = conv.cards.iter().find(|c| c.summary.id == conv.current).map(|c| c.summary.clone());
		self.conv = None;
		self.remove_screen(Screen::Conversation);
		if let Some(summary) = current {
			self.set_unread(&summary, true);
		}
	}

	fn set_pinned(&mut self, keys: &[String], pinned: bool) {
		let Some(session) = self.session.as_ref() else { return };
		for key in keys {
			if let Err(e) = session.cache.set_pinned(key, pinned) {
				self.say(e.to_string());
				return;
			}
		}
		self.reload_list();
	}

	fn thread_unread(&mut self, key: &str, folder: &FolderId, unread: bool) {
		let mut here = self.thread_here(key, folder);
		if unread {
			here = here.pop().into_iter().collect();
		}
		for summary in here.iter().filter(|m| m.unread != unread) {
			self.set_unread(summary, unread);
		}
	}

	// ---- Selecting

	fn selected_rows(&self) -> Vec<ThreadRow> {
		self.rows.iter().filter(|r| self.selection.contains(&r.key)).cloned().collect()
	}

	fn selection_ids(&self) -> Vec<MailId> {
		let Some(folder) = self.current_folder() else { return Vec::new() };
		self.selected_rows().iter().flat_map(|r| self.thread_here(&r.key, &folder.id)).map(|m| m.id).collect()
	}

	fn selection_action(&mut self, action: &Action) {
		let rows = self.selected_rows();
		let Some(folder) = self.current_folder() else { return };
		match action {
			Action::SelectionPin => {
				let pin = rows.iter().any(|r| !r.pinned);
				let keys: Vec<String> = rows.iter().map(|r| r.key.clone()).collect();
				self.set_pinned(&keys, pin);
				self.selection.clear();
			},
			Action::SelectionRead => {
				let mark_read = rows.iter().any(|r| r.unread);
				for row in &rows {
					self.thread_unread(&row.key, &folder.id, !mark_read);
				}
				self.selection.clear();
			},
			Action::SelectionTrash => {
				let ids = self.selection_ids();
				let message = if rows.len() > 1 { format!("{} conversations moved to Trash", rows.len()) } else { "Moved to Trash".to_string() };
				self.selection.clear();
				self.move_away(ids, FolderKind::Trash, message);
			},
			Action::SelectionSpam => {
				let own: Vec<MailSummary> = rows.iter().flat_map(|r| self.thread_here(&r.key, &folder.id)).filter(|m| !m.outgoing).collect();
				self.open_spam(own);
			},
			Action::SelectionLabels => {
				let mails: Vec<MailSummary> = rows.iter().flat_map(|r| self.thread_here(&r.key, &folder.id)).collect();
				let subject = if rows.len() == 1 { subject_or_none(&rows[0].latest.subject) } else { format!("{} conversations", rows.len()) };
				self.open_labels(subject, mails);
			},
			_ => {},
		}
	}

	// ---- Labels

	fn open_labels(&mut self, subject: String, mails: Vec<MailSummary>) {
		self.sheet = Some(Sheet::Labels(LabelSheet { subject, mails }));
		let Some(session) = self.session.as_ref() else { return };
		let backend = session.backend.clone();
		self.task(async move {
			let labels = backend.labels().await;
			move |app: &mut App| {
				if let Ok(labels) = labels {
					app.labels = labels;
				}
			}
		});
	}

	fn toggle_label(&mut self, id: String) {
		let Some(label) = self.labels.iter().find(|l| l.id.0 == id).cloned() else { return };
		let Some(Sheet::Labels(sheet)) = self.sheet.as_mut() else { return };
		let add = !sheet.mails.iter().all(|m| label.on(m));
		for mail in &mut sheet.mails {
			mail.sets.retain(|s| s != label.own_id());
			if add {
				mail.sets.push(label.own_id().to_string());
			}
		}
		let changed: Vec<(MailId, Vec<String>)> = sheet.mails.iter().map(|m| (m.id.clone(), m.sets.clone())).collect();
		self.apply_label(label, add, changed);
	}

	fn apply_label(&mut self, label: Label, add: bool, changed: Vec<(MailId, Vec<String>)>) {
		let Some(session) = self.session.as_ref() else { return };
		let (backend, cache) = (session.backend.clone(), session.cache.clone());
		for (id, sets) in &changed {
			let _ = cache.set_mail_sets(id, sets);
		}
		// The open conversation's cards, and a label's or a search's list, change with it.
		self.folder_changed(None);
		let ids: Vec<MailId> = changed.into_iter().map(|(id, _)| id).collect();
		self.start_work(Word::Moving);
		self.task(async move {
			let result = if add { backend.add_label(&ids, &label.id).await } else { backend.remove_label(&ids, &label.id).await };
			move |app: &mut App| {
				app.end_work(Word::Moving);
				if let Err(e) = result {
					app.failed_call(&e);
					app.refresh();
				}
			}
		});
	}

	fn create_label(&mut self, name: String) {
		let name = name.trim().to_string();
		let Some(session) = self.session.as_ref() else { return };
		if name.is_empty() {
			return;
		}
		let backend = session.backend.clone();
		self.task(async move {
			let result = backend.create_label(&name).await;
			move |app: &mut App| match result {
				Ok(label) => {
					let id = label.id.0.clone();
					app.labels.retain(|l| l.id != label.id);
					app.labels.push(label);
					app.labels.sort_by_key(|l| l.name.to_lowercase());
					app.toggle_label(id);
				},
				Err(e) => app.failed_call(&e),
			}
		});
	}

	// ---- Move to Spam

	fn open_spam(&mut self, own: Vec<MailSummary>) {
		if !self.folders.iter().any(|f| f.kind == FolderKind::Spam) {
			return self.say("This account has no Spam folder");
		}
		let Some(first) = own.last().cloned() else {
			return self.say("Only mail from others can go to Spam");
		};
		let revision = self.next_revision();
		let picked = own.iter().map(|m| m.id.clone()).collect();
		let line = term("from", &first.from.address);
		self.sheet = Some(Sheet::Spam(SpamSheet { own, revision, line, problem: None, mails: Vec::new(), total: 0, picked, keep_rule: false, generation: 0 }));
		self.refresh_spam();
	}

	fn refresh_spam(&mut self) {
		let Some(session) = self.session.as_ref() else { return };
		let cache = session.cache.clone();
		let Some(Sheet::Spam(sheet)) = self.sheet.as_mut() else { return };
		sheet.generation += 1;
		let generation = sheet.generation;
		let rule = match parse(&sheet.line) {
			Ok(rule) => {
				sheet.problem = None;
				Some(rule)
			},
			Err(e) => {
				sheet.problem = Some(e.message);
				None
			},
		};
		self.task(async move {
			let found = rule.and_then(|rule| cache.matching(&rule, &SPAM_SKIP, SPAM_SHOWN).ok()).unwrap_or_default();
			move |app: &mut App| {
				let Some(Sheet::Spam(sheet)) = app.sheet.as_mut() else { return };
				if sheet.generation != generation {
					return;
				}
				let (found, total) = found;
				let own: HashSet<MailId> = sheet.own.iter().map(|m| m.id.clone()).collect();
				sheet.mails = sheet.own.iter().cloned().chain(found.into_iter().map(|(m, _)| m).filter(|m| !own.contains(&m.id))).collect();
				sheet.total = total;
			}
		});
	}

	fn spam_confirm(&mut self) {
		let Some(Sheet::Spam(sheet)) = self.sheet.take() else { return };
		let ruled = sheet.keep_rule && parse(sheet.line.trim()).is_ok();
		if ruled {
			self.rules = rules::load(&self.paths);
			self.rules.push(Rule { when: sheet.line.trim().to_string(), action: tern_rules::Action::Spam });
			rules::save(&self.paths, &self.rules);
		}
		let ids: Vec<MailId> = sheet.mails.iter().map(|m| m.id.clone()).filter(|id| sheet.picked.contains(id)).collect();
		self.selection.clear();
		if ids.is_empty() {
			return self.say("Rule added");
		}
		let n = ids.len();
		if let Some(conv) = self.conv.as_ref().filter(|_| !self.follow_neighbour) {
			if conv.cards.iter().all(|c| ids.contains(&c.summary.id)) {
				self.conv = None;
				self.remove_screen(Screen::Conversation);
			}
		}
		let moved = if n == 1 { "Moved to Spam".to_string() } else { format!("Moved {n} mails to Spam") };
		let message = if ruled { format!("{moved}, and added a rule") } else { moved };
		self.move_away(ids, FolderKind::Spam, message);
	}

	// ---- Writing

	fn open_composer(&mut self, fields: Fields) {
		let Some(session) = self.session.as_ref() else {
			self.waiting.push(fields);
			return;
		};
		let addresses = session.backend.account().addresses();
		let id = self.next_comp;
		self.next_comp += 1;
		let revision = self.next_revision();
		let show_cc = !fields.cc.is_empty() || !fields.bcc.is_empty();
		let saved = snapshot(&fields);
		self.focus = Some(id);
		self.comps.push(Comp { id, fields, saved, addresses, revision, show_cc, pending: Vec::new(), sending: false, saving: false, error: None, tries: 0, bad: (false, false, false), suggest: None });
		self.push(Screen::Composer);
	}

	/// Opens a composer with these fields, now or once there is an account.
	pub fn compose(&mut self, fields: Fields) {
		self.open_composer(fields);
	}

	fn remove_composer(&mut self, id: u64) {
		self.comps.retain(|c| c.id != id);
		if self.focus == Some(id) {
			self.focus = self.comps.last().map(|c| c.id);
		}
		if self.comps.is_empty() {
			self.remove_screen(Screen::Composer);
		}
	}

	fn compose_answer(&mut self, mail: MailId, kind: AnswerKind, chosen: Option<String>) {
		let Some(session) = self.session.as_ref() else { return };
		let me = session.backend.account();
		let Some(card) = self.conv.as_ref().and_then(|c| c.cards.iter().find(|c| c.summary.id == mail)) else { return };
		let Some(opened) = &card.opened else { return };
		let answer = match kind {
			AnswerKind::Reply => Answer::Reply,
			AnswerKind::ReplyAll => Answer::ReplyAll,
			AnswerKind::Forward => Answer::Forward,
		};
		let date = format::long_date(opened.mail.summary.received_ms);
		// Blocks picked in the mail are what a reply answers.
		let picked = opened.blocks.iter().filter(|b| b.picked).map(block_text).collect::<Vec<_>>().join("\n");
		let picked = chosen.or((!picked.trim().is_empty()).then_some(picked));
		let fields = tern_compose::answer_fields(&opened.mail, &opened.plain, &date, answer, &me, picked.as_deref());
		let forwarded = if kind == AnswerKind::Forward { opened.mail.attachments.clone() } else { Vec::new() };
		self.open_composer(fields);
		let comp_id = self.focus;
		for att in forwarded {
			self.forward_attachment(comp_id, mail.clone(), att);
		}
	}

	fn forward_attachment(&mut self, comp_id: Option<u64>, mail: MailId, att: Attachment) {
		let Some(session) = self.session.as_ref() else { return };
		let backend = session.backend.clone();
		if let Some(comp) = self.comps.iter_mut().find(|c| Some(c.id) == comp_id) {
			comp.pending.push(att.name.clone());
		}
		self.start_work(Word::Attaching);
		self.task(async move {
			let result = backend.attachment(&mail, &att).await;
			move |app: &mut App| {
				app.end_work(Word::Attaching);
				let Some(comp) = app.comps.iter_mut().find(|c| Some(c.id) == comp_id) else { return };
				if let Some(at) = comp.pending.iter().position(|n| n == &att.name) {
					comp.pending.remove(at);
				}
				match result {
					Ok(data) => {
						comp.fields.attachments.push(NewAttachment { name: att.name, mime: att.mime, data });
						comp.saved = snapshot(&comp.fields);
					},
					Err(e) => comp.error = Some(format!("Could not attach {}: {e}", att.name)),
				}
			}
		});
	}

	fn edit_draft(&mut self, id: MailId) {
		let Some(session) = self.session.as_ref() else { return };
		let (fetcher, cache) = (session.fetcher.clone(), session.cache.clone());
		self.start_work(Word::Opening);
		self.task(async move {
			let result = fetcher.mail(&id).await;
			let kept: Option<Quote> = cache.draft_quote(&id).ok().flatten().and_then(|json| serde_json::from_str(&json).ok());
			let fields = result.map(|mail| {
				let text = Doc::parse(&mail.body_html).to_plain_text();
				tern_compose::draft_fields(&mail, &text, kept)
			});
			move |app: &mut App| {
				app.end_work(Word::Opening);
				match fields {
					Ok(fields) => app.open_composer(fields),
					Err(e) => app.failed_call(&e),
				}
			}
		});
	}

	fn take_text(comp: &mut Comp, text: ComposeText) {
		comp.fields.to = text.to;
		comp.fields.cc = text.cc;
		comp.fields.bcc = text.bcc;
		comp.fields.subject = text.subject;
		comp.fields.body = text.body;
		comp.fields.from = comp.addresses.get(text.from_index as usize).filter(|_| text.from_index > 0).cloned().unwrap_or_default();
	}

	fn compose_send(&mut self, text: ComposeText) {
		let Some(comp) = self.comps.iter_mut().find(|c| Some(c.id) == self.focus) else { return };
		comp.tries += 1;
		if comp.sending || !comp.pending.is_empty() {
			if !comp.pending.is_empty() {
				comp.error = Some("Attachments are still downloading".into());
			}
			return;
		}
		Self::take_text(comp, text);
		let draft = match comp.fields.build_draft() {
			Ok(draft) => draft,
			Err(invalid) => {
				comp.bad = (!invalid.to.is_empty() || invalid.no_recipients, !invalid.cc.is_empty(), !invalid.bcc.is_empty());
				if comp.bad.1 || comp.bad.2 {
					comp.show_cc = true;
				}
				comp.error = Some(invalid.message());
				return;
			},
		};
		comp.sending = true;
		comp.error = None;
		comp.bad = (false, false, false);
		comp.suggest = None;
		let id = comp.id;
		let existing = comp.fields.existing.clone();
		let subject = comp.fields.subject.clone();
		let Some(session) = self.session.as_ref() else { return };
		let (backend, cache) = (session.backend.clone(), session.cache.clone());
		self.start_work(Word::Sending);
		self.task(async move {
			let result = backend.send(draft).await;
			move |app: &mut App| {
				app.end_work(Word::Sending);
				match result {
					Ok(()) => {
						if let Some(old) = existing {
							let _ = cache.remove(&old);
						}
						app.remove_composer(id);
						app.note_fine();
						app.say(format::sent_toast(&subject));
						app.sync_kinds(&[FolderKind::Sent, FolderKind::Drafts]);
					},
					Err(e) => {
						if let Some(comp) = app.comps.iter_mut().find(|c| c.id == id) {
							comp.sending = false;
							comp.error = Some(e.to_string());
						}
						app.note_error(&e);
					},
				}
			}
		});
	}

	/// Closing keeps the message: unless nothing changed it goes to Drafts behind the screen. If
	/// saving fails the composer comes back with the reason.
	fn compose_close(&mut self, text: ComposeText) {
		let Some(comp) = self.comps.iter_mut().find(|c| Some(c.id) == self.focus) else { return };
		if comp.sending {
			return;
		}
		Self::take_text(comp, text);
		let unchanged = snapshot(&comp.fields) == comp.saved;
		if (comp.fields.is_empty() && comp.fields.existing.is_none()) || unchanged {
			let id = comp.id;
			return self.remove_composer(id);
		}
		if !comp.pending.is_empty() {
			comp.error = Some("Attachments are still downloading".into());
			return;
		}
		comp.saving = true;
		comp.error = None;
		let draft = comp.fields.saved_draft();
		let replaced = draft.existing.clone();
		let quote = (comp.fields.quote != Quote::default()).then(|| serde_json::to_string(&comp.fields.quote).ok()).flatten();
		let id = comp.id;
		self.remove_screen(Screen::Composer);
		let Some(session) = self.session.as_ref() else { return };
		let (backend, cache) = (session.backend.clone(), session.cache.clone());
		self.start_work(Word::Drafting);
		self.task(async move {
			let result = backend.save_draft(draft).await;
			if let Ok(new) = &result {
				if let Some(old) = replaced.filter(|old| old != new) {
					let _ = cache.remove(&old);
				}
				// Kept here so reopening shows the quotes as they were, not merged into the text.
				let _ = cache.set_draft_quote(new, quote.as_deref());
			}
			move |app: &mut App| {
				app.end_work(Word::Drafting);
				match result {
					Ok(_) => {
						app.remove_composer(id);
						app.note_fine();
						app.say("Saved to Drafts");
						app.sync_kinds(&[FolderKind::Drafts]);
					},
					Err(e) => {
						app.note_error(&e);
						if let Some(comp) = app.comps.iter_mut().find(|c| c.id == id) {
							comp.saving = false;
							comp.error = Some(format!("Could not save the draft: {e}"));
							comp.revision += 1;
							app.push(Screen::Composer);
						}
					},
				}
			}
		});
	}

	fn compose_discard(&mut self, text: ComposeText) {
		let Some(comp) = self.comps.iter_mut().find(|c| Some(c.id) == self.focus) else { return };
		Self::take_text(comp, text);
		if comp.fields.is_empty() && comp.fields.existing.is_none() {
			let id = comp.id;
			return self.remove_composer(id);
		}
		self.sheet = Some(Sheet::Discard);
	}

	fn compose_discard_confirmed(&mut self) {
		self.sheet = None;
		let Some(id) = self.focus else { return };
		let Some(at) = self.comps.iter().position(|c| c.id == id) else { return };
		let comp = self.comps.remove(at);
		self.remove_composer(id);
		if let (Some(id), Some(session)) = (comp.fields.existing, self.session.as_ref()) {
			let (backend, cache) = (session.backend.clone(), session.cache.clone());
			let _ = cache.remove(&id);
			self.start_work(Word::Deleting);
			self.folder_changed(None);
			self.task(async move {
				let result = backend.trash(&id).await;
				move |app: &mut App| {
					app.end_work(Word::Deleting);
					match result {
						Ok(()) => app.sync_kinds(&[FolderKind::Drafts, FolderKind::Trash]),
						Err(e) => app.failed_call(&e),
					}
				}
			});
		}
	}

	fn compose_attach(&mut self, name: String, mime: String, data: Vec<u8>) {
		let Some(comp) = self.comps.iter_mut().find(|c| Some(c.id) == self.focus) else { return };
		if comp.fields.total_size() + data.len() as u64 > MAX_TOTAL_BYTES {
			comp.error = Some("Attachments can be 25 MB in all".into());
			return;
		}
		let mime = if mime.is_empty() { tern_compose::mime_for(&name).to_string() } else { mime };
		comp.fields.attachments.push(NewAttachment { name: tern_compose::safe_file_name(&name), mime, data });
		comp.error = None;
	}

	fn compose_suggest(&mut self, field: AddressField, text: String) {
		let Some(session) = self.session.as_ref() else { return };
		let cache = session.cache.clone();
		let (start, word) = current_word(&text);
		if word.is_empty() {
			if let Some(comp) = self.comps.iter_mut().find(|c| Some(c.id) == self.focus) {
				comp.suggest = None;
			}
			return;
		}
		let (earlier, word) = (text[..start].to_lowercase(), word.to_string());
		let id = self.focus;
		self.task(async move {
			let found: Vec<Address> = cache.suggest(&word, 8).unwrap_or_default().into_iter().filter(|a| !earlier.contains(&a.address.to_lowercase())).take(4).collect();
			move |app: &mut App| {
				if let Some(comp) = app.comps.iter_mut().find(|c| Some(c.id) == id) {
					comp.suggest = (!found.is_empty()).then_some((field, found));
				}
			}
		});
	}

	fn compose_take(&mut self, text: ComposeText, field: AddressField, index: u32) {
		let revision = self.next_revision();
		let Some(comp) = self.comps.iter_mut().find(|c| Some(c.id) == self.focus) else { return };
		Self::take_text(comp, text);
		let Some(address) = comp.suggest.as_ref().and_then(|(_, found)| found.get(index as usize)).cloned() else { return };
		let slot = match field {
			AddressField::To => &mut comp.fields.to,
			AddressField::Cc => &mut comp.fields.cc,
			AddressField::Bcc => &mut comp.fields.bcc,
		};
		let (start, _) = current_word(slot);
		let before = slot[..start].trim_end();
		let joined = tern_compose::format_address(&address);
		*slot = if before.is_empty() { format!("{joined}, ") } else { format!("{before} {joined}, ") };
		comp.suggest = None;
		comp.revision = revision;
	}

	// ---- Search

	fn run_search(&mut self) {
		let Some(session) = self.session.as_ref() else { return };
		let cache = session.cache.clone();
		let meaning = self.settings().semantic;
		let Some(search) = self.search.as_mut() else { return };
		search.generation += 1;
		let generation = search.generation;
		let rule = search.rule.as_deref().and_then(|line| parse(line).ok());
		let label = search.label.as_ref().map(|(own, _)| own.clone());
		let query = Query::parse(&search.query);
		if query.is_empty() && rule.is_none() && label.is_none() {
			search.rows.clear();
			search.folders.clear();
			search.searching = false;
			return;
		}
		search.searching = true;
		let model = self.meaning.clone();
		self.task(async move {
			let hits = match (rule, label) {
				(_, Some(label)) => cache.label_hits(&label, RESULTS).unwrap_or_default(),
				(Some(rule), None) => cache.rule_hits(&rule, RESULTS).unwrap_or_default(),
				(None, None) => {
					let model = if meaning { model.loaded() } else { None };
					let vector = model.map(|m| m.embed(&query.text()));
					cache.search(&query, vector.as_deref().map(|v| (semantic::NAME, v)), RESULTS).unwrap_or_default()
				},
			};
			move |app: &mut App| {
				let Some(search) = app.search.as_mut() else { return };
				if search.generation != generation {
					return;
				}
				search.searching = false;
				let before: Vec<String> = search.rows.iter().map(|r| r.key.clone()).collect();
				search.folders = hits.iter().map(|h| (h.row.key.clone(), h.folder.clone())).collect();
				search.rows = hits.into_iter().map(|h| h.row).collect();
				app.follow_on(&before);
			}
		});
	}

	// ---- Rules

	fn open_rule(&mut self, index: Option<usize>) {
		self.rules = rules::load(&self.paths);
		let rule = index.and_then(|i| self.rules.get(i).cloned());
		let folder_names = self.rule_folders();
		let first_folder = folder_names.first().cloned().unwrap_or_default();
		let first_label = self.labels.first().map(|l| l.name.clone()).unwrap_or_default();
		let revision = self.next_revision();
		let (line, does, folder, label) = match rule {
			Some(rule) => {
				let line = rule.when;
				match rule.action {
					tern_rules::Action::Spam => (line, RuleDoes::Spam, first_folder, first_label),
					tern_rules::Action::Move(folder) => (line, RuleDoes::Folder, folder, first_label),
					tern_rules::Action::Label(label) => (line, RuleDoes::Label, first_folder, label),
				}
			},
			None => (String::new(), RuleDoes::Spam, first_folder, first_label),
		};
		let cursor = line.chars().count() as u32;
		self.rule_ed = Some(RuleEd { index, revision, cursor, line, does, folder, label, tone: PreviewTone::Neutral, message: String::new(), preview: Vec::new(), more: None, generation: 0, saving: false });
		self.push(Screen::RuleEditor);
		self.preview_rule();
	}

	fn rule_folders(&self) -> Vec<String> {
		self.folders.iter().filter(|f| !matches!(f.kind, FolderKind::Drafts | FolderKind::Sent | FolderKind::Spam | FolderKind::Trash)).map(|f| f.name.clone()).collect()
	}

	fn preview_rule(&mut self) {
		let Some(session) = self.session.as_ref() else { return };
		let cache = session.cache.clone();
		let Some(ed) = self.rule_ed.as_mut() else { return };
		ed.generation += 1;
		let generation = ed.generation;
		ed.preview.clear();
		ed.more = None;
		if ed.line.trim().is_empty() {
			ed.tone = PreviewTone::Neutral;
			ed.message = "Say which mail this rule catches, with the buttons or by typing.".into();
			return;
		}
		let rule = match parse(&ed.line) {
			Ok(rule) => rule,
			Err(e) => {
				ed.tone = PreviewTone::Bad;
				ed.message = e.message;
				return;
			},
		};
		self.task(async move {
			// Rules look at new mail in the Inbox, so what is already gone says nothing about them.
			let found = cache.matching(&rule, &SPAM_SKIP, PREVIEW).unwrap_or_default();
			move |app: &mut App| {
				let Some(ed) = app.rule_ed.as_mut() else { return };
				if ed.generation != generation {
					return;
				}
				let (found, total) = found;
				ed.tone = if total == 0 { PreviewTone::Neutral } else { PreviewTone::Good };
				ed.message = match total {
					0 => "No mail here matches yet. The rule will sort new mail that does.".to_string(),
					1 => "Catches 1 mail here already. Rules only sort new mail.".to_string(),
					n => format!("Catches {n} mails here already. Rules only sort new mail."),
				};
				ed.preview = found
					.iter()
					.map(|(m, _)| PreviewRow { who: if m.from.display().is_empty() { "(Unknown sender)".into() } else { m.from.display().to_string() }, subject: subject_or_none(&m.subject) })
					.collect();
				ed.more = (total > found.len()).then(|| format!("and {} more, older", total - found.len()));
			}
		});
	}

	fn rule_put(&mut self, piece: RulePiece, cursor: u32, selection_end: u32) {
		let revision = self.next_revision();
		let Some(ed) = self.rule_ed.as_mut() else { return };
		let piece = match piece {
			RulePiece::From => Piece::Field("from"),
			RulePiece::To => Piece::Field("to"),
			RulePiece::Subject => Piece::Field("subject"),
			RulePiece::And => Piece::Word("and"),
			RulePiece::Or => Piece::Word("or"),
			RulePiece::Not => Piece::Word("not"),
			RulePiece::Brackets => Piece::Brackets,
		};
		let selected = (selection_end > cursor).then_some((cursor as usize, selection_end as usize));
		let (line, at) = tern_rules::insert(&ed.line, cursor as usize, selected, piece);
		ed.line = line;
		ed.cursor = at as u32;
		ed.revision = revision;
		self.preview_rule();
	}

	fn rule_action(ed: &RuleEd) -> tern_rules::Action {
		match ed.does {
			RuleDoes::Spam => tern_rules::Action::Spam,
			RuleDoes::Folder => tern_rules::Action::Move(ed.folder.clone()),
			RuleDoes::Label => tern_rules::Action::Label(ed.label.trim().to_string()),
		}
	}

	fn rule_save(&mut self) {
		let Some(ed) = self.rule_ed.as_mut() else { return };
		if parse(&ed.line).is_err() || ed.saving {
			return;
		}
		// A label that is not there yet is made first: rules name labels that exist.
		if ed.does == RuleDoes::Label && !self.labels.iter().any(|l| l.name.eq_ignore_ascii_case(ed.label.trim())) {
			let name = ed.label.trim().to_string();
			if name.is_empty() {
				return;
			}
			ed.saving = true;
			let Some(session) = self.session.as_ref() else { return };
			let backend = session.backend.clone();
			return self.task(async move {
				let result = backend.create_label(&name).await;
				move |app: &mut App| match result {
					Ok(label) => {
						app.labels.push(label);
						app.labels.sort_by_key(|l| l.name.to_lowercase());
						if let Some(ed) = app.rule_ed.as_mut() {
							ed.saving = false;
						}
						app.rule_save();
					},
					Err(e) => {
						if let Some(ed) = app.rule_ed.as_mut() {
							ed.saving = false;
						}
						app.failed_call(&e);
					},
				}
			});
		}
		let Some(ed) = self.rule_ed.as_ref() else { return };
		let rule = Rule { when: ed.line.trim().to_string(), action: Self::rule_action(ed) };
		let index = ed.index;
		match index.filter(|i| *i < self.rules.len()) {
			Some(i) => self.rules[i] = rule,
			None => self.rules.push(rule),
		}
		rules::save(&self.paths, &self.rules);
		self.rule_ed = None;
		self.remove_screen(Screen::RuleEditor);
	}

	fn rule_delete(&mut self) {
		let Some(ed) = self.rule_ed.take() else { return };
		if let Some(i) = ed.index.filter(|i| *i < self.rules.len()) {
			self.rules.remove(i);
			rules::save(&self.paths, &self.rules);
		}
		self.remove_screen(Screen::RuleEditor);
	}

	// ---- Attachments and links

	fn attachment(&mut self, mail: MailRef, attachment: String, save: bool) {
		let Some(session) = self.session.as_ref() else { return };
		let id = mail_id(&mail);
		let found = self.conv.as_ref().and_then(|c| c.cards.iter().find(|c| c.summary.id == id)).and_then(|c| c.opened.as_ref()).and_then(|o| o.mail.attachments.iter().find(|a| a.id == attachment)).cloned();
		let Some(att) = found else { return };
		let (backend, dir) = (session.backend.clone(), self.open_dir.clone());
		self.start_work(Word::Downloading);
		self.task(async move {
			let result = backend.attachment(&id, &att).await;
			let written = result.map(|data| private_file(&dir, &att.name, &data).map_err(|e| Error::Other(e.to_string()))).and_then(|r| r);
			move |app: &mut App| {
				app.end_work(Word::Downloading);
				match written {
					Ok(path) => {
						let path = path.display().to_string();
						app.listener.on_effect(if save { Effect::SaveFile { path, name: att.name, mime: att.mime } } else { Effect::OpenFile { path, mime: att.mime } });
					},
					Err(e) => app.failed_call(&e),
				}
			}
		});
	}

	fn open_link(&mut self, url: String) {
		if let Some(fields) = tern_compose::mailto_fields(&url) {
			return self.open_composer(fields);
		}
		let lower = url.to_lowercase();
		if lower.starts_with("https://") || lower.starts_with("http://") {
			self.listener.on_effect(Effect::OpenUrl { url });
		}
	}


	// ---- Actions

	pub fn handle(&mut self, action: Action) {
		match action {
			Action::SignIn { email, password } => self.sign_in(email, password),
			Action::SignInAgain => self.push(Screen::SignIn),
			Action::SignOut => self.sign_out(),
			Action::Back => self.back(),
			Action::Foreground => {
				self.visible = true;
				self.listener.on_effect(Effect::ClearNotifications);
				if let Some(session) = &self.session {
					session.wake.notify_one();
				}
				self.resume_now();
			},
			Action::Hidden => {
				self.visible = false;
				self.mark_inbox_seen();
			},
			Action::NetworkChanged { online } => self.network_changed(online),
			Action::SnackAction => {
				if std::mem::take(&mut self.snack_retry) {
					self.snack = None;
					self.refresh();
				}
			},
			Action::SnackGone { id } => {
				if self.snack.as_ref().is_some_and(|s| s.id == id) {
					self.snack = None;
				}
			},
			Action::OpenLabel { id } => {
				if let Some(label) = self.labels.iter().find(|l| l.id.0 == id).cloned() {
					let revision = self.next_revision();
					self.conv = None;
					self.remove_screen(Screen::Conversation);
					self.search = Some(SearchState { revision, query: String::new(), rule: None, label: Some((label.own_id().to_string(), label.name)), searching: false, generation: 0, rows: Vec::new(), folders: HashMap::new() });
					self.push(Screen::Search);
					self.run_search();
				}
			},
			Action::OpenFolder { id } => {
				if let Some(folder) = self.folders.iter().find(|f| f.id.0 == id).cloned() {
					self.conv = None;
					self.search = None;
					self.remove_screen(Screen::Conversation);
					self.remove_screen(Screen::Search);
					self.open_folder(folder);
				}
			},
			Action::Refresh => self.refresh(),
			Action::LoadMore => self.load_more(),
			Action::OpenThread { key, folder_id } => {
				if self.session.is_some() {
					self.open_thread(key, FolderId(folder_id));
				} else {
					self.open_when_ready = Some((key, FolderId(folder_id)));
				}
			},
			Action::RowPin { key } => {
				let pinned = self.rows.iter().chain(self.search.iter().flat_map(|s| s.rows.iter())).find(|r| r.key == key).is_some_and(|r| r.pinned);
				self.set_pinned(&[key], !pinned);
			},
			Action::RowRead { key, folder_id, read } => {
				// Kept unread: left, or showing it marks it read again.
				if !read && self.conv.as_ref().is_some_and(|c| c.key == key) {
					self.conv = None;
					self.remove_screen(Screen::Conversation);
				}
				self.thread_unread(&key, &FolderId(folder_id), !read);
			},
			Action::RowSpam { key, folder_id } => {
				let own = self.thread_here(&key, &FolderId(folder_id)).into_iter().filter(|m| !m.outgoing).collect();
				self.open_spam(own);
			},
			Action::RowLabels { key, folder_id } => {
				let mails = self.thread_here(&key, &FolderId(folder_id));
				let subject = mails.last().map(|m| subject_or_none(&m.subject)).unwrap_or_default();
				self.open_labels(subject, mails);
			},
			Action::RowTrash { key, folder_id } => {
				let ids: Vec<MailId> = self.thread_here(&key, &FolderId(folder_id)).into_iter().map(|m| m.id).collect();
				let message = if ids.len() > 1 { "Conversation moved to Trash" } else { "Moved to Trash" }.to_string();
				self.move_away(ids, FolderKind::Trash, message);
			},
			Action::ToggleSelect { key } => {
				if !self.selection.remove(&key) {
					self.selection.insert(key);
				}
			},
			Action::SelectAll => self.selection = self.rows.iter().map(|r| r.key.clone()).collect(),
			Action::ClearSelection => self.selection.clear(),
			action @ (Action::SelectionPin | Action::SelectionRead | Action::SelectionTrash | Action::SelectionSpam | Action::SelectionLabels) => self.selection_action(&action),
			Action::ToggleCard { id } => self.toggle_card(mail_id(&id)),
			Action::TogglePick { id, block } => self.toggle_pick(mail_id(&id), block),
			Action::ConversationPin => {
				if let Some(key) = self.conv.as_ref().map(|c| c.key.clone()) {
					let pinned = self.rows.iter().any(|r| r.key == key && r.pinned);
					self.set_pinned(&[key], !pinned);
				}
			},
			Action::ConversationUnread => self.conversation_unread(),
			Action::ConversationArchive => self.conversation_away(FolderKind::Archive, "Moved to Archive", "Conversation moved to Archive"),
			Action::ConversationTrash => self.conversation_away(FolderKind::Trash, "Moved to Trash", "Conversation moved to Trash"),
			Action::ConversationSpam => {
				if let Some((key, folder)) = self.conv.as_ref().map(|c| (c.key.clone(), c.folder.clone())) {
					let own = self.thread_here(&key, &folder).into_iter().filter(|m| !m.outgoing).collect();
					self.open_spam(own);
				}
			},
			Action::ConversationLabels => {
				if let Some((key, folder, subject)) = self.conv.as_ref().map(|c| (c.key.clone(), c.folder.clone(), c.subject.clone())) {
					let mails = self.thread_here(&key, &folder);
					self.open_labels(subject, mails);
				}
			},
			Action::OpenAttachment { mail, attachment } => self.attachment(mail, attachment, false),
			Action::SaveAttachment { mail, attachment } => self.attachment(mail, attachment, true),
			Action::OpenLink { url } => self.open_link(url),
			Action::Answer { mail, kind } => self.compose_answer(mail_id(&mail), kind, None),
			Action::AnswerPicked { mail, kind, picked } => self.compose_answer(mail_id(&mail), kind, picked),
			Action::ComposeFocus { id } => {
				if self.comps.iter().any(|c| c.id == id) {
					self.focus = Some(id);
				}
			},
			Action::EditDraft { mail } => self.edit_draft(mail_id(&mail)),
			Action::DiscardDraft { mail } => self.move_away(vec![mail_id(&mail)], FolderKind::Trash, "Draft discarded".to_string()),
			Action::ComposeNew => self.open_composer(Fields::default()),
			Action::ComposeSend { text } => self.compose_send(text),
			Action::ComposeClose { text } => self.compose_close(text),
			Action::ComposeDiscard { text } => self.compose_discard(text),
			Action::ComposeDiscardConfirmed => self.compose_discard_confirmed(),
			Action::ComposeAttach { name, mime, data } => self.compose_attach(name, mime, data),
			Action::ComposeAttachFailed { name } => {
				if let Some(comp) = self.comps.iter_mut().find(|c| Some(c.id) == self.focus) {
					comp.error = Some(format!("Could not read {name}"));
				}
			},
			Action::ComposeRemoveAttachment { index } => {
				if let Some(comp) = self.comps.iter_mut().find(|c| Some(c.id) == self.focus) {
					let i = index as usize;
					let kept = comp.fields.kept.len();
					if i < kept {
						comp.fields.kept.remove(i);
					} else if i - kept < comp.fields.attachments.len() {
						comp.fields.attachments.remove(i - kept);
					}
				}
			},
			Action::ComposeDropPicked => {
				if let Some(comp) = self.comps.iter_mut().find(|c| Some(c.id) == self.focus) {
					comp.fields.quote.picked = None;
				}
			},
			Action::ComposeIncludeWhole { included } => {
				if let Some(whole) = self.comps.iter_mut().find(|c| Some(c.id) == self.focus).and_then(|c| c.fields.quote.whole.as_mut()) {
					whole.included = included;
				}
			},
			Action::ComposeShowCc => {
				if let Some(comp) = self.comps.iter_mut().find(|c| Some(c.id) == self.focus) {
					comp.show_cc = true;
				}
			},
			Action::ComposeSuggest { field, text } => self.compose_suggest(field, text),
			Action::ComposeTakeSuggestion { text, field, index } => self.compose_take(text, field, index),
			Action::ComposeEdited => {
				if let Some(comp) = self.comps.iter_mut().find(|c| Some(c.id) == self.focus) {
					comp.error = None;
					comp.bad = (false, false, false);
				}
			},
			Action::OpenSearch => {
				let revision = self.next_revision();
				self.search = Some(SearchState { revision, query: String::new(), rule: None, label: None, searching: false, generation: 0, rows: Vec::new(), folders: HashMap::new() });
				self.push(Screen::Search);
			},
			Action::CloseSearch => {
				self.search = None;
				self.conv = None;
				self.remove_screen(Screen::Search);
				self.remove_screen(Screen::Conversation);
				self.reload_list();
			},
			Action::SearchText { text } => {
				let Some(search) = self.search.as_mut() else { return };
				if search.query == text {
					return;
				}
				if search.rule.as_deref() != Some(text.as_str()) {
					search.rule = None;
				}
				search.query = text;
				// Other words: nothing stays open, so no result opens, and is marked read, in its place.
				self.conv = None;
				self.remove_screen(Screen::Conversation);
				self.run_search();
			},
			Action::RuleInSearch => {
				if let Some(line) = self.rule_ed.as_ref().map(|e| e.line.clone()).filter(|l| parse(l).is_ok()) {
					let revision = self.next_revision();
					self.search = Some(SearchState { revision, query: line.clone(), rule: Some(line), label: None, searching: false, generation: 0, rows: Vec::new(), folders: HashMap::new() });
					self.push(Screen::Search);
					self.run_search();
				}
			},
			Action::OpenSettings => {
				self.push(Screen::Settings);
				self.measure();
			},
			Action::CloseSettings => {
				self.rule_ed = None;
				self.stack.retain(|s| !matches!(s, Screen::Settings | Screen::Rules | Screen::RuleEditor));
			},
			Action::SetKeepReady { value } => {
				if let Some(level) = Level::from_name(&value) {
					self.change_settings(|s| s.offline = level);
				}
			},
			Action::SetForget { on } => self.change_settings(|s| s.forget_on_sign_out = on),
			Action::SetCleanupLimit { value } => {
				if let Some(mb) = value.parse().ok().filter(|mb| CLEANUP_MB.contains(mb)) {
					self.change_settings(|s| s.cleanup_mb = mb);
				}
			},
			Action::SetSemantic { on } => self.set_semantic(on),
			Action::SetPureBlack { on } => self.change_settings(|s| s.pure_black = on),
			Action::SetMatchWallpaper { on } => self.change_settings(|s| s.match_wallpaper = on),
			Action::SetBackground { value } => {
				if let Some(minutes) = value.parse().ok().filter(|m| BACKGROUND_MINUTES.contains(m)) {
					// Notifications need the checks that find the mail.
					self.change_settings(|s| {
						s.background_minutes = minutes;
						s.notifications = s.notifications && minutes > 0;
					});
				}
			},
			Action::SetNotifications { on } => {
				self.change_settings(|s| {
					s.notifications = on;
					if on && s.background_minutes == 0 {
						s.background_minutes = DEFAULT_BACKGROUND;
					}
				});
				// From now on: what is already there is not news.
				self.mark_inbox_seen();
			},
			Action::OpenCleanup => {
				let Some(usage) = self.usage else {
					self.cleanup_waiting = true;
					self.measure();
					return;
				};
				self.sheet = Some(Sheet::Cleanup(usage));
			},
			Action::RunCleanup { older, vectors, model, all } => self.run_cleanup(Pick { older, vectors, model, all }),
			Action::OpenRules => {
				self.rules = rules::load(&self.paths);
				self.push(Screen::Rules);
			},
			Action::OpenRule { index } => self.open_rule(index.map(|i| i as usize)),
			Action::RuleLine { line } => {
				if let Some(ed) = self.rule_ed.as_mut() {
					if ed.line != line {
						ed.line = line;
						self.preview_rule();
					}
				}
			},
			Action::RulePut { piece, cursor, selection_end } => self.rule_put(piece, cursor, selection_end),
			Action::RuleSetDoes { does } => {
				if let Some(ed) = self.rule_ed.as_mut() {
					ed.does = does;
				}
			},
			Action::RuleSetFolder { name } => {
				if let Some(ed) = self.rule_ed.as_mut() {
					ed.folder = name;
				}
			},
			Action::RuleSetLabel { name } => {
				if let Some(ed) = self.rule_ed.as_mut() {
					ed.label = name;
				}
			},
			Action::RuleSave => self.rule_save(),
			Action::RuleDelete => self.rule_delete(),
			Action::ToggleLabel { id } => self.toggle_label(id),
			Action::CreateLabel { name } => self.create_label(name),
			Action::SpamLine { line } => {
				if let Some(Sheet::Spam(sheet)) = self.sheet.as_mut() {
					sheet.line = line;
				}
				self.refresh_spam();
			},
			Action::SpamUse { value } => self.spam_use(value),
			Action::SpamToggle { id } => {
				if let Some(Sheet::Spam(sheet)) = self.sheet.as_mut() {
					let id = mail_id(&id);
					if !sheet.picked.remove(&id) {
						sheet.picked.insert(id);
					}
				}
			},
			Action::SpamSelectAll { on } => {
				if let Some(Sheet::Spam(sheet)) = self.sheet.as_mut() {
					sheet.picked = if on { sheet.mails.iter().map(|m| m.id.clone()).collect() } else { HashSet::new() };
				}
			},
			Action::SpamKeepRule { on } => {
				if let Some(Sheet::Spam(sheet)) = self.sheet.as_mut() {
					sheet.keep_rule = on;
				}
			},
			Action::SpamConfirm => self.spam_confirm(),
			Action::CloseSheet => self.sheet = None,
		}
	}

	fn spam_use(&mut self, value: String) {
		let revision = self.next_revision();
		let Some(Sheet::Spam(sheet)) = self.sheet.as_mut() else { return };
		let Some(first) = sheet.own.last() else { return };
		sheet.line = match value.as_str() {
			"domain" => term("from", &first.from.address.rsplit_once('@').map(|(_, d)| format!("@{d}")).unwrap_or_default()),
			"subject" => term("subject", tern_mail::bare_subject(&first.subject)),
			_ => term("from", &first.from.address),
		};
		sheet.revision = revision;
		self.refresh_spam();
	}

	fn back(&mut self) {
		if self.sheet.take().is_some() {
			return;
		}
		if !self.selection.is_empty() {
			self.selection.clear();
			return;
		}
		if self.stack.len() <= 1 {
			return;
		}
		match self.stack.pop() {
			Some(Screen::Conversation) => self.conv = None,
			Some(Screen::Search) => self.search = None,
			Some(Screen::RuleEditor) => self.rule_ed = None,
			Some(Screen::Composer) => {
				if let Some(id) = self.focus {
					self.remove_composer(id);
				}
			},
			_ => {},
		}
	}

	fn network_changed(&mut self, online: bool) {
		if !online {
			self.conn = Conn::Offline;
			return;
		}
		if self.conn == Conn::Offline || self.conn == Conn::Busy {
			let was_offline = self.conn == Conn::Offline;
			self.note_fine();
			if was_offline && self.session.is_some() {
				self.say("Back online");
			}
			self.resume_now();
			if let Some(session) = &self.session {
				session.wake.notify_one();
			}
		}
	}

	fn measure(&mut self) {
		let Some(session) = self.session.as_ref() else { return };
		let cache = session.cache.clone();
		let settings = self.settings();
		let (meaning, paths) = (self.meaning.clone(), self.paths.clone());
		self.task(async move {
			let usage = cleanup::measure(&cache, settings.offline, settings.semantic, &meaning);
			let shown = cleanup::offer(usage.total, settings.cleanup_mb, disk(&paths));
			move |app: &mut App| {
				app.usage = Some(usage);
				app.cleanup_offer = shown.then_some(usage.total);
				if std::mem::take(&mut app.cleanup_waiting) {
					app.sheet = Some(Sheet::Cleanup(usage));
				}
			}
		});
	}

	fn run_cleanup(&mut self, pick: Pick) {
		self.sheet = None;
		let Some(session) = self.session.as_ref() else { return };
		let cache = session.cache.clone();
		let level = self.settings().offline;
		let meaning = self.meaning.clone();
		self.task(async move {
			let freed = cleanup::clean(&cache, level, pick, &meaning);
			move |app: &mut App| {
				app.say(format!("Freed {}", format::size(freed)));
				app.measure();
				app.folder_changed(None);
				if let Some(session) = &app.session {
					session.wake.notify_one();
				}
			}
		});
	}

	fn set_semantic(&mut self, on: bool) {
		if on == self.settings().semantic {
			return;
		}
		self.change_settings(|s| s.semantic = on);
		if !on {
			self.semantic_note = None;
			self.meaning.forget(self.session.as_ref().map(|s| s.cache.as_ref()));
			return;
		}
		// Fetch the model now and say so; the sync then works out each mail's meaning.
		self.semantic_note = Some("Downloading the language model…".into());
		let meaning = self.meaning.clone();
		self.task(async move {
			let result = meaning.ensure().await;
			move |app: &mut App| match result {
				Ok(_) => {
					app.semantic_note = None;
					if let Some(session) = &app.session {
						session.wake.notify_one();
					}
				},
				Err(e) => {
					app.semantic_note = Some(format!("Could not download the model: {e}"));
					app.change_settings(|s| s.semantic = false);
				},
			}
		});
	}

	// ---- What is published

	fn status(&self) -> StatusView {
		if self.conn == Conn::SignedOut {
			return StatusView { kind: ConnectionKind::SignedOut, line: "Signed out".into() };
		}
		if let Some(at) = self.busy.iter().position(|n| *n > 0) {
			return StatusView { kind: ConnectionKind::Working, line: WORDS[at].1.into() };
		}
		match self.conn {
			Conn::Offline => StatusView { kind: ConnectionKind::Offline, line: "Offline · saved mail".into() },
			Conn::Busy => StatusView { kind: ConnectionKind::Busy, line: "Server busy · retrying".into() },
			_ => {
				let unread = self.current.lock().unwrap_or_else(|p| p.into_inner()).as_ref().map_or(0, |f| f.unread);
				StatusView { kind: ConnectionKind::Connected, line: if unread > 0 { format!("{unread} unread") } else { String::new() } }
			},
		}
	}

	fn shell_view(&self) -> ShellView {
		let settings = self.settings();
		ShellView {
			stack: self.stack.clone(),
			address: self.session.as_ref().map(|s| s.backend.account().address).unwrap_or_default(),
			status: self.status(),
			snack: self.snack.clone(),
			pure_black: settings.pure_black,
			match_wallpaper: settings.match_wallpaper,
			cleanup_offer: self.cleanup_offer.map(format::size),
			background_minutes: if self.session.is_some() { settings.background_minutes } else { 0 },
		}
	}

	fn row_view(&self, row: &ThreadRow, folder: &FolderId, now: i64) -> RowView {
		let m = &row.latest;
		RowView {
			key: row.key.clone(),
			folder_id: folder.0.clone(),
			who: who(m),
			subject: subject_or_none(&m.subject),
			date: format::short_date(m.received_ms, now),
			unread: row.unread,
			pinned: row.pinned,
			count: row.count,
			has_attachments: m.has_attachments,
			draft: m.draft,
			labels: self.labels.iter().filter(|l| l.on(m)).map(|l| l.name.clone()).collect(),
			selected: self.selection.contains(&row.key),
			open: self.conv.as_ref().is_some_and(|c| c.key == row.key),
		}
	}

	fn list_view(&self) -> ListView {
		let current = self.current_folder();
		let id = current.as_ref().map(|f| f.id.clone()).unwrap_or(FolderId(String::new()));
		let now = format::now_ms();
		let rows: Vec<RowView> = self.rows.iter().map(|r| self.row_view(r, &id, now)).collect();
		let state = if !rows.is_empty() {
			ListState::Rows
		} else if self.failed.is_some() {
			ListState::Failed
		} else if !self.rows_loaded || (!self.synced && self.in_flight.contains(&id)) || (self.folders.is_empty() && self.session.as_ref().is_some_and(|s| s.connected)) {
			ListState::Loading
		} else {
			ListState::Empty
		};
		let selection = (!self.selection.is_empty()).then(|| {
			let chosen = self.selected_rows();
			SelectionView { count: self.selection.len() as u32, pin: chosen.iter().any(|r| !r.pinned), mark_read: chosen.iter().any(|r| r.unread) }
		});
		let browsing = self.search.as_ref().is_some_and(|s| s.label.is_some());
		ListView {
			folders: self
				.folders
				.iter()
				.map(|f| FolderView {
					id: f.id.0.clone(),
					name: f.name.clone(),
					icon: match f.kind {
						FolderKind::Inbox => FolderIcon::Inbox,
						FolderKind::Drafts => FolderIcon::Drafts,
						FolderKind::Sent => FolderIcon::Sent,
						FolderKind::Archive => FolderIcon::Archive,
						FolderKind::Spam => FolderIcon::Spam,
						FolderKind::Trash => FolderIcon::Trash,
						FolderKind::Custom => FolderIcon::Folder,
					},
					unread: f.unread,
					selected: f.id == id && !browsing,
				})
				.collect(),
			labels: self
				.labels
				.iter()
				.map(|l| FolderView {
					id: l.id.0.clone(),
					name: l.name.clone(),
					icon: FolderIcon::Label,
					unread: 0,
					selected: self.search.as_ref().and_then(|s| s.label.as_ref()).is_some_and(|(own, _)| own == l.own_id()),
				})
				.collect(),
			title: current.map_or_else(|| "Tern".to_string(), |f| f.name),
			state,
			empty_title: match (&self.failed, self.folders.is_empty()) {
				(Some(_), true) => "Could not load folders".into(),
				(Some(_), false) => "Could not load messages".into(),
				(None, _) => "No messages".into(),
			},
			empty_detail: self.failed.as_ref().map(|e| format!("{e}. {}", self.retry_hint)),
			more: rows.len() >= self.shown || !self.complete,
			rows,
			selection,
		}
	}

	fn conversation_view(&self) -> Option<ConversationView> {
		let conv = self.conv.as_ref()?;
		let has = |kind: FolderKind| self.folders.iter().any(|f| f.kind == kind);
		let here = self.folder_kind(&conv.folder);
		let cards = conv
			.cards
			.iter()
			.map(|c| {
				let opened = c.opened.as_ref();
				let attachments = opened
					.map(|o| {
						o.mail
							.attachments
							.iter()
							.filter(|a| a.cid.as_ref().is_none_or(|cid| !o.inline.contains(cid)))
							.map(|a| AttachmentView { id: a.id.clone(), name: a.name.clone(), size: format::size(a.size), image: a.mime.starts_with("image/"), mime: a.mime.clone() })
							.collect()
					})
					.unwrap_or_default();
				let snippet = c.error.clone().or_else(|| {
					if c.expanded {
						return None;
					}
					match opened {
						Some(o) => Some(o.plain.split_whitespace().collect::<Vec<_>>().join(" ").chars().take(120).collect()),
						None => c.summary.to.as_ref().map(|to| format!("To {}", to.display())),
					}
				});
				CardView {
					id: mail_ref(&c.summary.id),
					subject: subject_or_none(&c.summary.subject),
					sent: c.kind == FolderKind::Sent,
					from: if c.summary.from.display().is_empty() { "(Unknown sender)".into() } else { c.summary.from.display().to_string() },
					address: c.summary.from.address.clone(),
					initials: initials(c.summary.from.display()),
					recipients: opened.and_then(|o| recipients_line(&o.mail)),
					date: format::long_date(c.summary.received_ms),
					snippet,
					expanded: c.expanded,
					loading: c.loading,
					current: c.summary.id == conv.current,
					draft: c.summary.draft,
					unread: c.summary.unread,
					blocks: opened.map(|o| o.blocks.clone()).unwrap_or_default(),
					attachments,
					web_url: opened.filter(|o| o.notice.is_some()).map(|o| o.mail.web_url.clone()),
					notice: opened.and_then(|o| o.notice.clone()),
					open_url: opened.map(|o| o.mail.web_url.clone()),
				}
			})
			.collect();
		let labels = self.labels.iter().filter(|l| conv.cards.iter().any(|c| l.on(&c.summary))).map(|l| l.name.clone()).collect();
		Some(ConversationView {
			key: conv.key.clone(),
			subject: conv.subject.clone(),
			labels,
			cards,
			can_archive: has(FolderKind::Archive) && here != FolderKind::Archive,
			can_spam: has(FolderKind::Spam) && here != FolderKind::Spam,
			pinned: self.rows.iter().any(|r| r.key == conv.key && r.pinned),
		})
	}

	fn composer_view(&self) -> Option<ComposerView> {
		self.comps.iter().find(|c| Some(c.id) == self.focus).filter(|c| !c.saving).map(Self::view_of_composer)
	}

	fn view_of_composer(comp: &Comp) -> ComposerView {
		let f = &comp.fields;
		let subject = f.subject.trim();
		let kept = f.kept.iter().map(|a| ComposerAttachment { name: a.name.clone(), size: format::size(a.size), mime: a.mime.clone(), pending: false });
		let new = f.attachments.iter().map(|a| ComposerAttachment { name: a.name.clone(), size: format::size(a.data.len() as u64), mime: a.mime.clone(), pending: false });
		let pending = comp.pending.iter().map(|name| ComposerAttachment { name: name.clone(), size: String::new(), mime: String::new(), pending: true });
		let attachments: Vec<ComposerAttachment> = kept.chain(new).chain(pending).collect();
		ComposerView {
			id: comp.id,
			revision: comp.revision,
			title: if subject.is_empty() { "New message".to_string() } else { subject.to_string() },
			text: ComposeText {
				to: f.to.clone(),
				cc: f.cc.clone(),
				bcc: f.bcc.clone(),
				subject: f.subject.clone(),
				body: f.body.clone(),
				from_index: comp.addresses.iter().position(|a| a.eq_ignore_ascii_case(&f.from)).unwrap_or(0) as u32,
			},
			addresses: comp.addresses.clone(),
			show_cc: comp.show_cc,
			picked_who: f.quote.picked.as_ref().map(|(who, _)| format!("{who} wrote:")),
			picked_text: f.quote.picked.as_ref().map(|(_, lines)| lines.clone()),
			whole_label: f.quote.whole.as_ref().map(|w| w.label.clone()),
			whole_text: f.quote.whole.as_ref().map(|w| format!("{}\n{}", w.head, w.text)),
			whole_included: f.quote.whole.as_ref().is_some_and(|w| w.included),
			whole_forward: f.quote.whole.as_ref().is_some_and(|w| w.forward),
			total: (!attachments.is_empty()).then(|| format::size(f.total_size())),
			attachments,
			sending: comp.sending,
			saving: comp.saving,
			error: comp.error.clone(),
			tries: comp.tries,
			bad_to: comp.bad.0,
			bad_cc: comp.bad.1,
			bad_bcc: comp.bad.2,
			suggest_field: comp.suggest.as_ref().map(|(field, _)| *field),
			suggestions: comp.suggest.as_ref().map(|(_, found)| found.iter().map(|a| SuggestionView { name: a.name.clone(), address: a.address.clone() }).collect()).unwrap_or_default(),
		}
	}

	fn search_view(&self) -> SearchView {
		let now = format::now_ms();
		let Some(search) = self.search.as_ref() else {
			return SearchView { revision: 0, query: String::new(), searching: false, title: "Search".into(), note: String::new(), empty: false, empty_title: String::new(), empty_detail: String::new(), rows: Vec::new() };
		};
		let rows: Vec<RowView> = search
			.rows
			.iter()
			.map(|r| {
				let folder = search.folders.get(&r.key).cloned().unwrap_or(FolderId(String::new()));
				self.row_view(r, &folder, now)
			})
			.collect();
		let asked = !search.query.trim().is_empty();
		SearchView {
			revision: search.revision,
			query: search.query.clone(),
			searching: search.searching,
			title: match (&search.label, &search.rule) {
				(Some((_, name)), _) => name.clone(),
				(None, Some(_)) => "Mail the rule catches".into(),
				(None, None) => "Search".into(),
			},
			note: format!("Searching the mail kept on this {}", self.device),
			empty: (asked || search.label.is_some()) && !search.searching && rows.is_empty(),
			empty_title: if search.label.is_some() { "No mail with this label".into() } else { "No matches".into() },
			empty_detail: format!("{} in mail on this {}", if search.label.is_some() { "None" } else { "No matches" }, self.device),
			rows,
		}
	}

	fn settings_view(&self) -> SettingsView {
		let s = self.settings();
		SettingsView {
			address: self.session.as_ref().map(|s| s.backend.account().address).unwrap_or_default(),
			keep_ready: Level::ALL.iter().map(|l| ChoiceView { value: l.name().to_string(), label: level_label(*l), selected: *l == s.offline }).collect(),
			keep_ready_line: level_label(s.offline),
			forget_on_sign_out: s.forget_on_sign_out,
			cleanup_limits: CLEANUP_MB.iter().map(|mb| ChoiceView { value: mb.to_string(), label: limit_label(*mb), selected: *mb == s.cleanup_mb }).collect(),
			cleanup_limit_line: limit_label(s.cleanup_mb),
			storage_line: self.usage.map_or_else(|| "Measuring…".to_string(), |usage| storage_line(usage, self.device)),
			semantic: s.semantic,
			semantic_line: self.semantic_note.clone().unwrap_or_else(|| semantic_about(self.device)),
			pure_black: s.pure_black,
			match_wallpaper: s.match_wallpaper,
			background: BACKGROUND_MINUTES.iter().map(|m| ChoiceView { value: m.to_string(), label: background_label(*m), selected: *m == s.background_minutes }).collect(),
			background_line: if s.background_minutes == 0 {
				"Mail is only fetched while Tern is open".to_string()
			} else {
				format!("{}. Android may wait longer to save battery.", background_label(s.background_minutes))
			},
			notifications: s.notifications,
			notifications_line: if s.notifications { "Says when new mail comes to the Inbox".to_string() } else { "Off".to_string() },
			rules_line: match self.rules.len() {
				0 => "Sort new mail in the Inbox as it arrives".to_string(),
				1 => "1 rule".to_string(),
				n => format!("{n} rules"),
			},
		}
	}

	fn rules_view(&self) -> RulesView {
		RulesView {
			rows: self
				.rules
				.iter()
				.enumerate()
				.map(|(i, rule)| {
					let target_gone = match &rule.action {
						tern_rules::Action::Spam => false,
						action @ tern_rules::Action::Move(_) => !self.folders.is_empty() && tern_rules::folder_for(action, &self.folders).is_none(),
						tern_rules::Action::Label(name) => !self.labels.is_empty() && !self.labels.iter().any(|l| l.name.eq_ignore_ascii_case(name)),
					};
					let broken = parse(&rule.when).is_err() || target_gone;
					RuleRowView { index: i as u32, when: rule.when.clone(), does: if broken { "Does nothing until fixed".into() } else { tern_rules::describe(&rule.action) }, broken }
				})
				.collect(),
		}
	}

	fn rule_editor_view(&self) -> Option<RuleEditorView> {
		let ed = self.rule_ed.as_ref()?;
		let spans = tern_rules::parts(&ed.line)
			.into_iter()
			.map(|(range, part)| LineSpan {
				start: range.start as u32,
				end: range.end as u32,
				part: match part {
					Part::Field(Field::From) => LinePart::From,
					Part::Field(Field::To) => LinePart::To,
					Part::Field(Field::Subject) => LinePart::Subject,
					Part::Field(Field::Any) => LinePart::Value,
					Part::Operator => LinePart::Operator,
					Part::Bracket => LinePart::Bracket,
					Part::Value => LinePart::Value,
				},
			})
			.collect();
		let target_ok = match ed.does {
			RuleDoes::Spam => true,
			RuleDoes::Folder => !ed.folder.is_empty(),
			RuleDoes::Label => !ed.label.trim().is_empty(),
		};
		Some(RuleEditorView {
			revision: ed.revision,
			cursor: ed.cursor,
			editing: ed.index.is_some(),
			line: ed.line.clone(),
			spans,
			tone: ed.tone,
			message: ed.message.clone(),
			preview: ed.preview.clone(),
			more: ed.more.clone(),
			does: ed.does,
			folders: self.rule_folders(),
			folder: ed.folder.clone(),
			labels: self.labels.iter().map(|l| l.name.clone()).collect(),
			label: ed.label.clone(),
			can_save: parse(&ed.line).is_ok() && target_ok && !ed.saving,
		})
	}

	fn sheet_view(&self) -> Option<SheetView> {
		let now = format::now_ms();
		match self.sheet.as_ref()? {
			Sheet::Labels(sheet) => Some(SheetView::Labels {
				subject: sheet.subject.clone(),
				choices: self
					.labels
					.iter()
					.map(|l| {
						let on = sheet.mails.iter().filter(|m| l.on(m)).count();
						LabelChoice { id: l.id.0.clone(), name: l.name.clone(), on: on > 0 && on == sheet.mails.len(), mixed: on > 0 && on < sheet.mails.len() }
					})
					.collect(),
			}),
			Sheet::Spam(sheet) => {
				let picked = sheet.mails.iter().filter(|m| sheet.picked.contains(&m.id)).count();
				Some(SheetView::Spam {
					revision: sheet.revision,
					line: sheet.line.clone(),
					problem: sheet.problem.clone(),
					rows: sheet.mails.iter().map(|m| SpamRow { id: mail_ref(&m.id), who: who(m), subject: subject_or_none(&m.subject), date: format::short_date(m.received_ms, now), picked: sheet.picked.contains(&m.id) }).collect(),
					count: match sheet.total {
						0 => "Nothing else matches".to_string(),
						1 => "1 match".to_string(),
						n if n > SPAM_SHOWN => format!("Newest {SPAM_SHOWN} of {n} matches"),
						n => format!("{n} matches"),
					},
					keep_rule: sheet.keep_rule,
					confirm: match (picked, sheet.keep_rule) {
						(0, true) => "Add rule".to_string(),
						(0, false) | (1, _) => "Move to Spam".to_string(),
						(n, _) => format!("Move {n} to Spam"),
					},
					choices: ["sender", "domain", "subject"].map(|v| ChoiceView { value: v.to_string(), label: { let mut c = v.chars(); c.next().map(|f| f.to_uppercase().collect::<String>() + c.as_str()).unwrap_or_default() }, selected: false }).to_vec(),
				})
			},
			Sheet::Cleanup(usage) => {
				let mut choices = vec![CleanupChoice { part: CleanupPart::Older, title: "Older mail".into(), detail: "Mail past Keep ready, and the text of older mail opened".into(), size: format::size(usage.older), bytes: usage.older }];
				if usage.vectors > 0 {
					choices.push(CleanupChoice { part: CleanupPart::Vectors, title: "Meanings for search".into(), detail: "Worked out again as mail is synced".into(), size: format::size(usage.vectors), bytes: usage.vectors });
				}
				if usage.model > 0 {
					choices.push(CleanupChoice { part: CleanupPart::Model, title: "Language model".into(), detail: "Downloaded again if search by meaning is turned on".into(), size: format::size(usage.model), bytes: usage.model });
				}
				choices.push(CleanupChoice { part: CleanupPart::All, title: "All mail".into(), detail: format!("Deletes all mail on this {}. It downloads again from Tuta.", self.device), size: format::size(usage.total), bytes: usage.total });
				Some(SheetView::Cleanup { total: format::size(usage.total), choices })
			},
			Sheet::Discard => Some(SheetView::Discard),
		}
	}

	/// Tells the screen what changed since it last heard.
	pub fn publish(&mut self) {
		macro_rules! send {
			($slot:ident, $view:expr, $update:expr) => {{
				let view = $view;
				if self.published.$slot.as_ref() != Some(&view) {
					self.published.$slot = Some(view.clone());
					#[allow(clippy::redundant_closure_call)]
					self.listener.on_update(($update)(view));
				}
			}};
		}
		send!(shell, self.shell_view(), |view| Update::Shell { view });
		send!(signin, self.signin.clone(), |view| Update::SignIn { view });
		send!(list, self.list_view(), |view| Update::List { view });
		send!(conversation, self.conversation_view(), |view| Update::Conversation { view });
		send!(composer, self.composer_view(), |view| Update::Composer { view });
		send!(composers, ComposersView { open: self.comps.iter().map(Self::view_of_composer).collect() }, |view| Update::Composers { view });
		send!(search, self.search_view(), |view| Update::Search { view });
		send!(settings, self.settings_view(), |view| Update::Settings { view });
		send!(rules, self.rules_view(), |view| Update::Rules { view });
		send!(rule_editor, self.rule_editor_view(), |view| Update::RuleEditor { view });
		send!(sheet, self.sheet_view(), |view| Update::Sheet { view });
	}

	pub fn generation(&self) -> u64 {
		self.generation
	}
}

#[cfg(test)]
mod session_tests {
	use super::*;

	struct Nobody;
	impl EngineListener for Nobody {
		fn on_update(&self, _: Update) {}
		fn on_effect(&self, _: Effect) {}
		fn on_log(&self, _: String) {}
	}

	/// A store that can be told to fail, and remembers what it was asked to keep.
	#[derive(Default)]
	struct Store {
		failing: std::sync::atomic::AtomicBool,
		kept: Mutex<HashMap<String, Vec<u8>>>,
	}
	impl SecretStore for Store {
		fn get(&self, name: String) -> Result<Option<Vec<u8>>, SecretError> {
			if self.failing.load(std::sync::atomic::Ordering::Relaxed) {
				return Err(SecretError::Unavailable);
			}
			Ok(self.kept.lock().unwrap().get(&name).cloned())
		}
		fn put(&self, name: String, value: Vec<u8>) -> Result<(), SecretError> {
			if self.failing.load(std::sync::atomic::Ordering::Relaxed) {
				return Err(SecretError::Unavailable);
			}
			self.kept.lock().unwrap().insert(name, value);
			Ok(())
		}
		fn remove(&self, name: String) {
			self.kept.lock().unwrap().remove(&name);
		}
	}

	#[test]
	fn the_cache_key_is_made_once_and_never_replaced_by_a_failure() {
		let store = Store::default();
		let key = cache_key(&store, "me@tuta.com").unwrap();
		assert_eq!(cache_key(&store, "me@tuta.com"), Some(key), "kept, not made again");

		store.failing.store(true, std::sync::atomic::Ordering::Relaxed);
		assert_eq!(cache_key(&store, "me@tuta.com"), None, "no cache on disk this time");
		store.failing.store(false, std::sync::atomic::Ordering::Relaxed);
		assert_eq!(cache_key(&store, "me@tuta.com"), Some(key), "still the same key afterwards");
	}

	#[test]
	fn a_cache_key_that_cannot_be_saved_or_read_is_not_used() {
		let store = Store::default();
		store.failing.store(true, std::sync::atomic::Ordering::Relaxed);
		assert_eq!(cache_key(&store, "new@tuta.com"), None);
		store.failing.store(false, std::sync::atomic::Ordering::Relaxed);

		store.kept.lock().unwrap().insert("cache-key:bad@tuta.com".into(), vec![1, 2, 3]);
		assert_eq!(cache_key(&store, "bad@tuta.com"), None);
		assert_eq!(store.kept.lock().unwrap()["cache-key:bad@tuta.com"], [1, 2, 3], "left as it was");
	}

	#[test]
	fn no_saved_session_differs_from_one_that_cannot_be_read() {
		let store = Store::default();
		assert_eq!(saved_session(&store).unwrap(), None);
		store.failing.store(true, std::sync::atomic::Ordering::Relaxed);
		assert!(saved_session(&store).is_err());
	}

	#[test]
	fn signing_out_stops_the_sync() {
		let runtime = tokio::runtime::Builder::new_multi_thread().worker_threads(1).enable_all().build().unwrap();
		// Kept open: the loop must stop because the account went, not because the app did.
		let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
		let mut app = App::new(tx, Arc::new(Nobody), Arc::new(Store::default()), runtime.handle().clone(), Paths::new(std::env::temp_dir(), std::env::temp_dir()), Options::default());
		app.sign_in("demo".into(), String::new());
		let demo = Arc::downgrade(app.session.as_ref().unwrap().demo.as_ref().unwrap());
		app.sign_out();
		let start = std::time::Instant::now();
		while demo.upgrade().is_some() {
			assert!(start.elapsed() < Duration::from_secs(5), "something still holds the account signed out of");
			std::thread::sleep(Duration::from_millis(10));
		}
	}
}

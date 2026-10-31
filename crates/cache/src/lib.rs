//! Tern's local copy of the mailbox: folders, mail lists, opened mails and the addresses seen, in an
//! encrypted SQLite database (SQLCipher), so the window shows mail at once and works offline.
//!
//! It is a cache: anything here can be fetched again, so a database that does not open with the key
//! or has an older layout is thrown away and started afresh.

pub mod embed;
pub mod search;
mod storage;
pub mod policy;
pub mod sync;

use std::path::{Path, PathBuf};
use std::sync::Mutex;

use rusqlite::types::Value;
use rusqlite::{params, params_from_iter, Connection, OptionalExtension};
use tern_core::rules::Expr;
use tern_core::{thread_key, Address, Folder, FolderId, FolderKind, Mail, MailId, MailSummary};

use search::{Found, Query};

/// Bump when the tables change; older databases are dropped.
const SCHEMA_VERSION: i64 = 2;

const SCHEMA: &str = "
	CREATE TABLE folders (id TEXT PRIMARY KEY, name TEXT NOT NULL, kind TEXT NOT NULL, pos INTEGER NOT NULL);
	CREATE TABLE mails (
		id INTEGER PRIMARY KEY, list TEXT NOT NULL, element TEXT NOT NULL, folder TEXT NOT NULL, thread TEXT NOT NULL,
		received INTEGER NOT NULL, unread INTEGER NOT NULL, summary TEXT NOT NULL,
		UNIQUE (list, element));
	CREATE INDEX mails_by_folder ON mails (folder, received DESC);
	CREATE INDEX mails_by_thread ON mails (thread, received);
	-- mail is NULL when the mail could not be opened, so it is not tried again on every sync.
	CREATE TABLE bodies (list TEXT NOT NULL, element TEXT NOT NULL, mail TEXT, PRIMARY KEY (list, element));
	-- How far back a folder has been fetched: the page cursor after the oldest mail, and whether that was the end.
	CREATE TABLE folder_state (folder TEXT PRIMARY KEY, next TEXT, complete INTEGER NOT NULL, synced INTEGER NOT NULL);
	-- seen: in mail received or read; used: written to; contact: in the address book.
	CREATE TABLE addresses (
		address TEXT PRIMARY KEY, name TEXT NOT NULL, contact INTEGER NOT NULL DEFAULT 0,
		used INTEGER NOT NULL DEFAULT 0, seen INTEGER NOT NULL DEFAULT 0, last INTEGER NOT NULL DEFAULT 0);
	-- The words of each mail (rowid = mails.id) for search: names and addresses with the sender and
	-- recipients, the body as plain text once the mail is opened.
	CREATE VIRTUAL TABLE words USING fts5(subject, sender, recipients, body, tokenize = 'unicode61 remove_diacritics 2', prefix = '2 3');
	-- What each mail is about, as a unit vector worked out by `model` (see embed::to_blob).
	CREATE TABLE vectors (mail INTEGER PRIMARY KEY, model TEXT NOT NULL, vector BLOB NOT NULL);
";

/// Conversation `key` as a list shows it, from the folder of its mail `mail` (a row id).
fn hit(conn: &Connection, key: String, mail: i64) -> Result<SearchHit> {
	let folder: String = conn.query_row("SELECT folder FROM mails WHERE id = ?1", [mail], |r| r.get(0))?;
	let from_here = "FROM mails m LEFT JOIN folders f ON f.id = m.folder WHERE m.thread = ?1 AND (m.folder = ?2 OR coalesce(f.kind, '') NOT IN ('Trash', 'Spam'))";
	let latest: String = conn.query_row(&format!("SELECT m.summary {from_here} ORDER BY m.received DESC LIMIT 1"), params![key, folder], |r| r.get(0))?;
	let (count, unread): (u32, bool) = conn.query_row(&format!("SELECT count(*), max(m.unread) {from_here}"), params![key, folder], |r| Ok((r.get(0)?, r.get(1)?)))?;
	let pinned = conn.query_row("SELECT 1 FROM pins WHERE thread = ?1", [&key], |_| Ok(())).optional()?.is_some();
	Ok(SearchHit { row: ThreadRow { key, latest: serde_json::from_str(&latest)?, count, unread, pinned }, folder: FolderId(folder) })
}

/// How close a mail's meaning must be to the query's (as a cosine) to be found by meaning; below
/// it every mail would be found, ranked.
pub const MEANING_MIN: f32 = 0.25;
/// Mails taken from each ranking before fusing them.
const DEPTH: usize = 200;
/// The start of a mail is what its meaning is worked out from: the model reads 512 word pieces at most.
const MEANING_CHARS: usize = 3000;

#[derive(Debug)]
pub enum CacheError {
	Sqlite(rusqlite::Error),
	Json(serde_json::Error),
}

impl std::fmt::Display for CacheError {
	fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
		match self {
			CacheError::Sqlite(e) => write!(f, "cache: {e}"),
			CacheError::Json(e) => write!(f, "cache: {e}"),
		}
	}
}

impl std::error::Error for CacheError {}

impl From<rusqlite::Error> for CacheError {
	fn from(e: rusqlite::Error) -> Self {
		CacheError::Sqlite(e)
	}
}

impl From<serde_json::Error> for CacheError {
	fn from(e: serde_json::Error) -> Self {
		CacheError::Json(e)
	}
}

pub type Result<T> = std::result::Result<T, CacheError>;

/// One conversation as a folder lists it: its newest mail in that folder.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ThreadRow {
	pub key: String,
	pub latest: MailSummary,
	/// Mails in the whole conversation, across folders (Trash and Spam only when listing them).
	pub count: u32,
	/// Any of the conversation's mails in this folder is unread.
	pub unread: bool,
	/// Pinned to the top of the list.
	pub pinned: bool,
}

/// A conversation search found, with the folder of its best match, which opening it uses.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SearchHit {
	pub row: ThreadRow,
	pub folder: FolderId,
}

/// One mail of a conversation, with the kind of folder it is in.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ThreadMail {
	pub summary: MailSummary,
	pub folder: FolderKind,
	pub folder_id: FolderId,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct FolderState {
	/// The cursor for the page after the oldest mail fetched so far.
	pub next: Option<String>,
	/// The oldest mail of the folder has been fetched.
	pub complete: bool,
	/// When the folder was last brought up to date, in Unix milliseconds; 0 for never.
	pub synced_ms: i64,
}

/// How an address was met, for ranking suggestions.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Met {
	Seen,
	Used,
	Contact,
}

pub struct Cache {
	conn: Mutex<Connection>,
	/// The database file, or None for a cache in memory.
	path: Option<PathBuf>,
}

/// The thread a mail belongs to: by subject, or alone when it has none.
fn thread_of(summary: &MailSummary) -> String {
	thread_key(&summary.subject).unwrap_or_else(|| format!("#{}/{}", summary.id.list, summary.id.element))
}

fn kind_text(kind: FolderKind) -> String {
	format!("{kind:?}")
}

fn kind_from(text: &str) -> FolderKind {
	match text {
		"Inbox" => FolderKind::Inbox,
		"Drafts" => FolderKind::Drafts,
		"Sent" => FolderKind::Sent,
		"Archive" => FolderKind::Archive,
		"Spam" => FolderKind::Spam,
		"Trash" => FolderKind::Trash,
		_ => FolderKind::Custom,
	}
}

fn now_ms() -> i64 {
	std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_millis() as i64).unwrap_or(0)
}

/// `text` for a LIKE pattern, with its wildcards taken literally (escape character `\`).
fn like_escape(text: &str) -> String {
	text.replace('\\', "\\\\").replace('%', "\\%").replace('_', "\\_")
}

impl Cache {
	/// Opens the database at `path` with `key`, starting a new one if it is missing, was written with
	/// another key, or has an older layout.
	pub fn open(path: &Path, key: &[u8; 32]) -> Result<Cache> {
		if let Some(dir) = path.parent() {
			let _ = std::fs::create_dir_all(dir);
		}
		match Self::open_existing(path, key) {
			Ok(cache) => Ok(cache),
			Err(_) => {
				for suffix in ["", "-wal", "-shm"] {
					let _ = std::fs::remove_file(format!("{}{suffix}", path.display()));
				}
				Self::open_existing(path, key)
			},
		}
	}

	fn open_existing(path: &Path, key: &[u8; 32]) -> Result<Cache> {
		let conn = Connection::open(path)?;
		// A wrong key is handled by starting afresh; SQLCipher need not log it.
		let _ = conn.execute_batch("PRAGMA cipher_log_level = NONE;");
		let hex: String = key.iter().map(|b| format!("{b:02x}")).collect();
		// The key is hex digits only, so it cannot break out of the quotes.
		conn.execute_batch(&format!("PRAGMA key = \"x'{hex}'\";"))?;
		// Fails here when the key is wrong.
		conn.query_row("SELECT count(*) FROM sqlite_master", [], |_| Ok(()))?;
		conn.execute_batch("PRAGMA journal_mode = WAL; PRAGMA synchronous = NORMAL;")?;
		Ok(Cache { path: Some(path.to_path_buf()), ..Self::prepare(conn)? })
	}

	/// A cache in memory only, for the demo account and tests.
	pub fn in_memory() -> Cache {
		Self::prepare(Connection::open_in_memory().expect("open an in-memory database")).expect("set up an in-memory cache")
	}

	fn prepare(conn: Connection) -> Result<Cache> {
		let version: i64 = conn.query_row("PRAGMA user_version", [], |r| r.get(0))?;
		if version != SCHEMA_VERSION {
			// Virtual tables first: dropping one drops the tables behind it.
			for virtual_only in [true, false] {
				let tables: Vec<String> = conn
					.prepare("SELECT name FROM sqlite_master WHERE type = 'table' AND name != 'pins' AND (?1 = 0 OR sql LIKE 'CREATE VIRTUAL TABLE%')")?
					.query_map([virtual_only], |r| r.get(0))?
					.collect::<std::result::Result<_, _>>()?;
				for table in tables {
					conn.execute_batch(&format!("DROP TABLE IF EXISTS \"{}\"", table.replace('"', "")))?;
				}
			}
			conn.execute_batch(SCHEMA)?;
			conn.execute_batch(&format!("PRAGMA user_version = {SCHEMA_VERSION}"))?;
		}
		// Named points in time, such as how far new mail has been through the rules. Kept apart from
		// the schema's version: adding it does not start the cache afresh.
		conn.execute_batch("CREATE TABLE IF NOT EXISTS marks (name TEXT PRIMARY KEY, value INTEGER NOT NULL);")?;
		// The conversations pinned to the top of the list, by thread key. Only here, as Tuta has no
		// pins, so a new schema keeps them.
		conn.execute_batch("CREATE TABLE IF NOT EXISTS pins (thread TEXT PRIMARY KEY);")?;
		// What a draft written here quotes of the mail it answers, as the composer keeps it apart from
		// the text: Tuta stores only the body, with the quotes in it.
		conn.execute_batch("CREATE TABLE IF NOT EXISTS draft_quotes (list TEXT NOT NULL, element TEXT NOT NULL, quote TEXT NOT NULL, PRIMARY KEY (list, element));")?;
		// Every word in the search index with how many mails hold it, for finding what a typo meant.
		// A view of the index, on this connection only: nothing more is stored.
		// The page cursor after a mail that ended a page, so mail trimmed past it is fetched again from there.
		conn.execute_batch("CREATE TABLE IF NOT EXISTS cursors (list TEXT NOT NULL, element TEXT NOT NULL, next TEXT NOT NULL, PRIMARY KEY (list, element));")?;
		conn.execute_batch("CREATE VIRTUAL TABLE temp.vocab USING fts5vocab(main, words, row);")?;
		Ok(Cache { conn: Mutex::new(conn), path: None })
	}

	fn conn(&self) -> std::sync::MutexGuard<'_, Connection> {
		self.conn.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
	}

	/// The folders, in the order last set, with unread counts from the cached mails.
	pub fn folders(&self) -> Result<Vec<Folder>> {
		let conn = self.conn();
		let mut stmt = conn.prepare(
			"SELECT f.id, f.name, f.kind, (SELECT count(*) FROM mails m WHERE m.folder = f.id AND m.unread)
			 FROM folders f ORDER BY f.pos",
		)?;
		let rows = stmt.query_map([], |r| {
			Ok(Folder { id: FolderId(r.get(0)?), name: r.get(1)?, kind: kind_from(&r.get::<_, String>(2)?), unread: r.get(3)? })
		})?;
		Ok(rows.collect::<std::result::Result<_, _>>()?)
	}

	/// Replaces the folder list. Mails of folders that are gone are dropped.
	pub fn set_folders(&self, folders: &[Folder]) -> Result<()> {
		let mut conn = self.conn();
		let tx = conn.transaction()?;
		tx.execute("DELETE FROM folders", [])?;
		for (pos, f) in folders.iter().enumerate() {
			tx.execute("INSERT INTO folders (id, name, kind, pos) VALUES (?1, ?2, ?3, ?4)", params![f.id.0, f.name, kind_text(f.kind), pos as i64])?;
		}
		for table in ["DELETE FROM words WHERE rowid", "DELETE FROM vectors WHERE mail"] {
			tx.execute(&format!("{table} IN (SELECT id FROM mails WHERE folder NOT IN (SELECT id FROM folders))"), [])?;
		}
		tx.execute("DELETE FROM mails WHERE folder NOT IN (SELECT id FROM folders)", [])?;
		tx.execute("DELETE FROM folder_state WHERE folder NOT IN (SELECT id FROM folders)", [])?;
		tx.commit()?;
		Ok(())
	}

	/// Stores a page of `folder` as the server listed it, newest first, and returns how many mails
	/// were new or changed. Mails of the folder within the page's span that the page lacks are gone
	/// from the server and are dropped: `first` says the page starts at the newest mail, `last` that
	/// it ends at the oldest.
	pub fn merge_page(&self, folder: &FolderId, mails: &[MailSummary], first: bool, last: bool) -> Result<usize> {
		let mut conn = self.conn();
		let tx = conn.transaction()?;
		let mut changed = 0;
		for m in mails {
			let summary = serde_json::to_string(m)?;
			let before: Option<(String, String)> = tx
				.query_row("SELECT folder, summary FROM mails WHERE list = ?1 AND element = ?2", params![m.id.list, m.id.element], |r| {
					Ok((r.get(0)?, r.get(1)?))
				})
				.optional()?;
			if before.as_ref() != Some(&(folder.0.clone(), summary.clone())) {
				changed += 1;
				tx.execute(
					"INSERT INTO mails (list, element, folder, thread, received, unread, summary) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)
					 ON CONFLICT (list, element) DO UPDATE SET folder = ?3, thread = ?4, received = ?5, unread = ?6, summary = ?7",
					params![m.id.list, m.id.element, folder.0, thread_of(m), m.received_ms, m.unread, summary],
				)?;
				index(&tx, &m.id)?;
			}
			note(&tx, &m.from, Met::Seen)?;
		}
		let newest = if first { i64::MAX } else { mails.iter().map(|m| m.received_ms).max().unwrap_or(i64::MIN) };
		let oldest = if last { i64::MIN } else { mails.iter().map(|m| m.received_ms).min().unwrap_or(i64::MAX) };
		if newest >= oldest {
			let keep: Vec<String> = mails.iter().map(|m| format!("{}/{}", m.id.list, m.id.element)).collect();
			let mut stale = tx.prepare("SELECT list, element FROM mails WHERE folder = ?1 AND received BETWEEN ?2 AND ?3")?;
			let gone: Vec<(String, String)> = stale
				.query_map(params![folder.0, oldest, newest], |r| Ok((r.get(0)?, r.get(1)?)))?
				.collect::<std::result::Result<Vec<(String, String)>, _>>()?
				.into_iter()
				.filter(|(l, e)| !keep.contains(&format!("{l}/{e}")))
				.collect();
			drop(stale);
			for (list, element) in gone.iter().cloned() {
				forget(&tx, &MailId { list, element })?;
			}
			changed += gone.len();
		}
		tx.commit()?;
		Ok(changed)
	}

	/// The newest `limit` mails of `folder`, after skipping `offset`.
	pub fn list(&self, folder: &FolderId, offset: usize, limit: usize) -> Result<Vec<MailSummary>> {
		let conn = self.conn();
		let mut stmt = conn.prepare("SELECT summary FROM mails WHERE folder = ?1 ORDER BY received DESC LIMIT ?2 OFFSET ?3")?;
		let rows = stmt.query_map(params![folder.0, limit as i64, offset as i64], |r| r.get::<_, String>(0))?;
		rows.map(|json| Ok(serde_json::from_str(&json?)?)).collect()
	}

	/// The conversations of `folder`: the pinned ones first, each part newest first.
	pub fn threads(&self, folder: &FolderId, limit: usize) -> Result<Vec<ThreadRow>> {
		let conn = self.conn();
		let mut stmt = conn.prepare(
			"WITH here AS (
				SELECT thread, summary, received,
					ROW_NUMBER() OVER (PARTITION BY thread ORDER BY received DESC) AS rank,
					MAX(unread) OVER (PARTITION BY thread) AS any_unread
				FROM mails WHERE folder = ?1)
			 SELECT here.thread, here.summary, here.any_unread,
				(SELECT count(*) FROM mails m LEFT JOIN folders f ON f.id = m.folder
				 WHERE m.thread = here.thread AND (m.folder = ?1 OR coalesce(f.kind, '') NOT IN ('Trash', 'Spam'))),
				here.thread IN (SELECT thread FROM pins) AS pinned
			 FROM here WHERE rank = 1 ORDER BY pinned DESC, here.received DESC LIMIT ?2",
		)?;
		let rows = stmt.query_map(params![folder.0, limit as i64], |r| {
			Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?, r.get::<_, bool>(2)?, r.get::<_, u32>(3)?, r.get::<_, bool>(4)?))
		})?;
		rows.map(|row| {
			let (key, summary, unread, count, pinned) = row?;
			Ok(ThreadRow { key, latest: serde_json::from_str(&summary)?, count, unread, pinned })
		})
		.collect()
	}

	/// Every mail of the conversation `key`, oldest first. Mails in Trash and Spam are left out
	/// unless `folder` is that folder.
	pub fn thread(&self, key: &str, folder: &FolderId) -> Result<Vec<ThreadMail>> {
		let conn = self.conn();
		let mut stmt = conn.prepare(
			"SELECT m.summary, coalesce(f.kind, ''), m.folder FROM mails m LEFT JOIN folders f ON f.id = m.folder
			 WHERE m.thread = ?1 AND (m.folder = ?2 OR coalesce(f.kind, '') NOT IN ('Trash', 'Spam'))
			 ORDER BY m.received",
		)?;
		let rows = stmt.query_map(params![key, folder.0], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?, r.get::<_, String>(2)?)))?;
		rows.map(|row| {
			let (summary, kind, folder_id) = row?;
			Ok(ThreadMail { summary: serde_json::from_str(&summary)?, folder: kind_from(&kind), folder_id: FolderId(folder_id) })
		})
		.collect()
	}

	/// The conversation a mail is in.
	pub fn thread_key_of(&self, id: &MailId) -> Result<Option<String>> {
		let conn = self.conn();
		Ok(conn.query_row("SELECT thread FROM mails WHERE list = ?1 AND element = ?2", params![id.list, id.element], |r| r.get(0)).optional()?)
	}

	/// An opened mail, if it was stored.
	pub fn mail(&self, id: &MailId) -> Result<Option<Mail>> {
		let conn = self.conn();
		let json: Option<Option<String>> =
			conn.query_row("SELECT mail FROM bodies WHERE list = ?1 AND element = ?2", params![id.list, id.element], |r| r.get(0)).optional()?;
		Ok(match json.flatten() {
			Some(json) => Some(serde_json::from_str(&json)?),
			None => None,
		})
	}

	/// Stores an opened mail; its list row takes the mail's summary.
	pub fn put_mail(&self, mail: &Mail) -> Result<()> {
		let mut conn = self.conn();
		let tx = conn.transaction()?;
		let id = &mail.summary.id;
		tx.execute(
			"INSERT INTO bodies (list, element, mail) VALUES (?1, ?2, ?3) ON CONFLICT (list, element) DO UPDATE SET mail = ?3",
			params![id.list, id.element, serde_json::to_string(mail)?],
		)?;
		tx.execute(
			"UPDATE mails SET summary = ?3, unread = ?4, thread = ?5, received = ?6 WHERE list = ?1 AND element = ?2",
			params![id.list, id.element, serde_json::to_string(&mail.summary)?, mail.summary.unread, thread_of(&mail.summary), mail.summary.received_ms],
		)?;
		index(&tx, id)?;
		let met = if mail.summary.outgoing { Met::Used } else { Met::Seen };
		for a in mail.to.iter().chain(&mail.cc).chain(&mail.bcc) {
			note(&tx, a, met)?;
		}
		note(&tx, &mail.summary.from, Met::Seen)?;
		for a in &mail.reply_to {
			note(&tx, a, Met::Seen)?;
		}
		tx.commit()?;
		Ok(())
	}

	/// Whether `id` is stored opened.
	pub fn has_mail(&self, id: &MailId) -> Result<bool> {
		let conn = self.conn();
		let found: Option<bool> =
			conn.query_row("SELECT mail IS NOT NULL FROM bodies WHERE list = ?1 AND element = ?2", params![id.list, id.element], |r| r.get(0)).optional()?;
		Ok(found == Some(true))
	}

	/// Remembers that `id` cannot be opened, so syncing does not try again.
	pub fn mark_unopenable(&self, id: &MailId) -> Result<()> {
		self.conn().execute("INSERT OR IGNORE INTO bodies (list, element, mail) VALUES (?1, ?2, NULL)", params![id.list, id.element])?;
		Ok(())
	}

	/// Of the newest `limit` mails, those not stored opened yet, newest first.
	pub fn missing_bodies(&self, limit: usize) -> Result<Vec<MailId>> {
		let conn = self.conn();
		let mut stmt = conn.prepare(
			"SELECT m.list, m.element FROM (SELECT list, element, received FROM mails ORDER BY received DESC LIMIT ?1) m
			 LEFT JOIN bodies b ON b.list = m.list AND b.element = m.element WHERE b.list IS NULL ORDER BY m.received DESC",
		)?;
		let rows = stmt.query_map(params![limit as i64], |r| Ok(MailId { list: r.get(0)?, element: r.get(1)? }))?;
		Ok(rows.collect::<std::result::Result<_, _>>()?)
	}

	pub fn set_unread(&self, id: &MailId, unread: bool) -> Result<()> {
		let mut conn = self.conn();
		let tx = conn.transaction()?;
		let summary: Option<String> =
			tx.query_row("SELECT summary FROM mails WHERE list = ?1 AND element = ?2", params![id.list, id.element], |r| r.get(0)).optional()?;
		if let Some(json) = summary {
			let mut s: MailSummary = serde_json::from_str(&json)?;
			s.unread = unread;
			tx.execute("UPDATE mails SET unread = ?3, summary = ?4 WHERE list = ?1 AND element = ?2", params![id.list, id.element, unread, serde_json::to_string(&s)?])?;
		}
		let body: Option<Option<String>> =
			tx.query_row("SELECT mail FROM bodies WHERE list = ?1 AND element = ?2", params![id.list, id.element], |r| r.get(0)).optional()?;
		if let Some(Some(json)) = body {
			let mut m: Mail = serde_json::from_str(&json)?;
			m.summary.unread = unread;
			tx.execute("UPDATE bodies SET mail = ?3 WHERE list = ?1 AND element = ?2", params![id.list, id.element, serde_json::to_string(&m)?])?;
		}
		tx.commit()?;
		Ok(())
	}

	/// Keeps what draft `id` quotes, as the composer describes it, or forgets it with `None`. It
	/// goes when the draft does.
	pub fn set_draft_quote(&self, id: &MailId, quote: Option<&str>) -> Result<()> {
		let conn = self.conn();
		match quote {
			Some(q) => conn.execute("INSERT OR REPLACE INTO draft_quotes (list, element, quote) VALUES (?1, ?2, ?3)", params![id.list, id.element, q])?,
			None => conn.execute("DELETE FROM draft_quotes WHERE list = ?1 AND element = ?2", params![id.list, id.element])?,
		};
		Ok(())
	}

	pub fn draft_quote(&self, id: &MailId) -> Result<Option<String>> {
		Ok(self.conn().query_row("SELECT quote FROM draft_quotes WHERE list = ?1 AND element = ?2", params![id.list, id.element], |r| r.get(0)).optional()?)
	}

	/// Pins the conversation `key` to the top of the lists it is in, or unpins it.
	pub fn set_pinned(&self, key: &str, pinned: bool) -> Result<()> {
		let sql = if pinned { "INSERT OR IGNORE INTO pins (thread) VALUES (?1)" } else { "DELETE FROM pins WHERE thread = ?1" };
		self.conn().execute(sql, [key])?;
		Ok(())
	}

	/// Moves a mail to another folder, as trashing does.
	pub fn move_to(&self, id: &MailId, folder: &FolderId) -> Result<()> {
		self.conn().execute("UPDATE mails SET folder = ?3 WHERE list = ?1 AND element = ?2", params![id.list, id.element, folder.0])?;
		Ok(())
	}

	/// The mails `rule` catches, newest first, leaving out folders of the kinds in `skip`, each with
	/// its folder: at most `limit` of them, and how many there are in all.
	pub fn matching(&self, rule: &Expr, skip: &[FolderKind], limit: usize) -> Result<(Vec<(MailSummary, FolderId)>, usize)> {
		let conn = self.conn();
		let mut stmt = conn.prepare("SELECT m.summary, m.folder, f.kind FROM mails m JOIN folders f ON f.id = m.folder ORDER BY m.received DESC")?;
		let rows = stmt.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?, r.get::<_, String>(2)?)))?;
		let (mut found, mut total) = (Vec::new(), 0);
		for row in rows {
			let (json, folder, kind) = row?;
			if skip.contains(&kind_from(&kind)) {
				continue;
			}
			let summary: MailSummary = serde_json::from_str(&json)?;
			if rule.matches(&summary) {
				total += 1;
				if found.len() < limit {
					found.push((summary, FolderId(folder)));
				}
			}
		}
		Ok((found, total))
	}

	/// The mails of `folder` received after `after_ms`, oldest first.
	pub fn received_after(&self, folder: &FolderId, after_ms: i64) -> Result<Vec<MailSummary>> {
		let conn = self.conn();
		let mut stmt = conn.prepare("SELECT summary FROM mails WHERE folder = ?1 AND received > ?2 ORDER BY received")?;
		let rows = stmt.query_map(params![folder.0, after_ms], |r| r.get::<_, String>(0))?;
		rows.map(|json| Ok(serde_json::from_str(&json?)?)).collect()
	}

	pub fn mark(&self, name: &str) -> Result<Option<i64>> {
		Ok(self.conn().query_row("SELECT value FROM marks WHERE name = ?1", params![name], |r| r.get(0)).optional()?)
	}

	pub fn set_mark(&self, name: &str, value: i64) -> Result<()> {
		self.conn().execute("INSERT INTO marks (name, value) VALUES (?1, ?2) ON CONFLICT (name) DO UPDATE SET value = ?2", params![name, value])?;
		Ok(())
	}

	/// Forgets a mail, as when a draft is replaced or sent.
	pub fn remove(&self, id: &MailId) -> Result<()> {
		let mut conn = self.conn();
		let tx = conn.transaction()?;
		forget(&tx, id)?;
		tx.commit()?;
		Ok(())
	}

	pub fn folder_state(&self, folder: &FolderId) -> Result<FolderState> {
		let conn = self.conn();
		Ok(conn
			.query_row("SELECT next, complete, synced FROM folder_state WHERE folder = ?1", params![folder.0], |r| {
				Ok(FolderState { next: r.get(0)?, complete: r.get(1)?, synced_ms: r.get(2)? })
			})
			.optional()?
			.unwrap_or_default())
	}

	pub fn set_folder_state(&self, folder: &FolderId, state: &FolderState) -> Result<()> {
		self.conn().execute(
			"INSERT INTO folder_state (folder, next, complete, synced) VALUES (?1, ?2, ?3, ?4)
			 ON CONFLICT (folder) DO UPDATE SET next = ?2, complete = ?3, synced = ?4",
			params![folder.0, state.next, state.complete, state.synced_ms],
		)?;
		Ok(())
	}

	/// How many mails of `folder` are stored.
	pub fn count(&self, folder: &FolderId) -> Result<usize> {
		Ok(self.conn().query_row("SELECT count(*) FROM mails WHERE folder = ?1", params![folder.0], |r| r.get::<_, i64>(0))? as usize)
	}

	/// Remembers addresses, for suggestions while writing.
	pub fn note_addresses(&self, addresses: &[Address], met: Met) -> Result<()> {
		let mut conn = self.conn();
		let tx = conn.transaction()?;
		for a in addresses {
			note(&tx, a, met)?;
		}
		tx.commit()?;
		Ok(())
	}

	/// Up to `limit` known addresses whose address or any word of the name starts with `query`:
	/// contacts first, then those written to most, then the most recent.
	pub fn suggest(&self, query: &str, limit: usize) -> Result<Vec<Address>> {
		let q = like_escape(query.trim());
		if q.is_empty() {
			return Ok(Vec::new());
		}
		let conn = self.conn();
		let mut stmt = conn.prepare(
			"SELECT name, address FROM addresses
			 WHERE address LIKE ?1 ESCAPE '\\' OR name LIKE ?1 ESCAPE '\\' OR name LIKE ?2 ESCAPE '\\'
			 ORDER BY contact DESC, used DESC, last DESC LIMIT ?3",
		)?;
		let rows = stmt.query_map(params![format!("{q}%"), format!("% {q}%"), limit as i64], |r| Ok(Address::new(r.get::<_, String>(0)?, r.get::<_, String>(1)?)))?;
		Ok(rows.collect::<std::result::Result<_, _>>()?)
	}

	/// Up to `limit` conversations matching `query`, best first. Words are matched as prefixes
	/// (ranked by BM25, the subject weighing most); with `meaning` (a model's name and the query's
	/// vector from it) mails close in meaning are found too, and both rankings are fused.
	pub fn search(&self, query: &Query, meaning: Option<(&str, &[f32])>, limit: usize) -> Result<Vec<SearchHit>> {
		let conn = self.conn();
		// Where to look: the folder asked for by name or kind, else all but Trash and Spam.
		let mut scope = match &query.folder {
			Some(_) => "(lower(f.name) = ? OR lower(f.kind) = ?)".to_string(),
			None => "coalesce(f.kind, '') NOT IN ('Trash', 'Spam')".to_string(),
		};
		let scope_args: Vec<Value> = query.folder.iter().flat_map(|f| [Value::Text(f.clone()), Value::Text(f.clone())]).collect();
		if query.attachment {
			scope.push_str(" AND json_extract(m.summary, '$.has_attachments') = 1");
		}
		let fixes: Vec<Option<String>> = query.words.iter().map(|w| typo_fix(&conn, w)).collect::<Result<_>>()?;
		let all = query.fts_fixed(&fixes);
		if all.is_none() && !query.words.is_empty() {
			// Only punctuation typed: nothing can match.
			return Ok(Vec::new());
		}
		let free_words = all != query.fts(false);
		let (sql, mut args) = match &all {
			Some(expr) => {
				let order = if free_words { "bm25(words, 4.0, 2.0, 1.0, 1.0)" } else { "m.received DESC" };
				(
					format!("SELECT m.id, m.thread, m.received FROM words JOIN mails m ON m.id = words.rowid LEFT JOIN folders f ON f.id = m.folder WHERE words MATCH ? AND {scope} ORDER BY {order} LIMIT ?"),
					vec![Value::Text(expr.clone())],
				)
			},
			None => (format!("SELECT m.id, m.thread, m.received FROM mails m LEFT JOIN folders f ON f.id = m.folder WHERE {scope} ORDER BY m.received DESC LIMIT ?"), vec![]),
		};
		args.extend(scope_args.iter().cloned());
		args.push(Value::Integer(DEPTH as i64));
		let found = |r: &rusqlite::Row| Ok(Found { mail: r.get(0)?, thread: r.get(1)?, received: r.get(2)? });
		let by_words: Vec<Found> = conn.prepare(&sql)?.query_map(params_from_iter(args), found)?.collect::<std::result::Result<_, _>>()?;

		let mut by_meaning = Vec::new();
		if let Some((model, vector)) = meaning.filter(|_| free_words) {
			let mut sql = format!(
				"SELECT m.id, m.thread, m.received, v.vector FROM vectors v JOIN mails m ON m.id = v.mail LEFT JOIN folders f ON f.id = m.folder WHERE v.model = ? AND {scope}"
			);
			let mut args = vec![Value::Text(model.to_string())];
			args.extend(scope_args);
			// The filters hold for mails found by meaning too.
			if let Some(filters) = query.fts(false) {
				sql.push_str(" AND m.id IN (SELECT rowid FROM words WHERE words MATCH ?)");
				args.push(Value::Text(filters));
			}
			let mut stmt = conn.prepare(&sql)?;
			let rows = stmt.query_map(params_from_iter(args), |r| Ok((found(r)?, embed::dot_blob(vector, &r.get::<_, Vec<u8>>(3)?))))?;
			let mut close: Vec<(Found, f32)> = rows.filter(|r| r.as_ref().map_or(true, |(_, cos)| *cos >= MEANING_MIN)).collect::<std::result::Result<_, _>>()?;
			close.sort_by(|a, b| b.1.total_cmp(&a.1));
			by_meaning = close.into_iter().take(DEPTH).map(|(f, _)| f).collect();
		}

		search::fuse(&[&by_words, &by_meaning]).into_iter().take(limit).map(|(key, mail)| hit(&conn, key, mail)).collect()
	}

	/// The conversations with mail `rule` catches outside Trash and Spam, newest first: at most
	/// `limit`, each opening in the folder of its newest caught mail.
	pub fn rule_hits(&self, rule: &Expr, limit: usize) -> Result<Vec<SearchHit>> {
		let conn = self.conn();
		let mut stmt = conn.prepare(
			"SELECT m.id, m.thread, m.summary FROM mails m LEFT JOIN folders f ON f.id = m.folder
			 WHERE coalesce(f.kind, '') NOT IN ('Trash', 'Spam') ORDER BY m.received DESC",
		)?;
		let rows = stmt.query_map([], |r| Ok((r.get::<_, i64>(0)?, r.get::<_, String>(1)?, r.get::<_, String>(2)?)))?;
		let mut found: Vec<(String, i64)> = Vec::new();
		for row in rows {
			let (mail, key, json) = row?;
			if found.len() == limit {
				break;
			}
			if !found.iter().any(|(k, _)| k == &key) && rule.matches(&serde_json::from_str(&json)?) {
				found.push((key, mail));
			}
		}
		drop(stmt);
		found.into_iter().map(|(key, mail)| hit(&conn, key, mail)).collect()
	}

	/// Up to `limit` mails, newest first, whose meaning `model` has not worked out yet, with the
	/// text to work it out from.
	pub fn unembedded(&self, model: &str, limit: usize) -> Result<Vec<(i64, String)>> {
		let conn = self.conn();
		let mut stmt = conn.prepare(
			"SELECT m.id, w.subject, w.body FROM mails m JOIN words w ON w.rowid = m.id LEFT JOIN vectors v ON v.mail = m.id AND v.model = ?1
			 WHERE v.mail IS NULL ORDER BY m.received DESC LIMIT ?2",
		)?;
		let rows = stmt.query_map(params![model, limit as i64], |r| Ok((r.get(0)?, meaning_text(&r.get::<_, String>(1)?, &r.get::<_, String>(2)?))))?;
		Ok(rows.collect::<std::result::Result<_, _>>()?)
	}

	/// Stores vectors worked out by `model` from the texts [`Cache::unembedded`] gave, and returns
	/// how many. A mail that changed or went meanwhile is left for the next round.
	pub fn put_vectors(&self, model: &str, vectors: &[(i64, String, Vec<f32>)]) -> Result<usize> {
		let mut conn = self.conn();
		let tx = conn.transaction()?;
		let mut stored = 0;
		for (id, text, vector) in vectors {
			let now: Option<String> = tx
				.query_row("SELECT subject, body FROM words WHERE rowid = ?1", [id], |r| Ok(meaning_text(&r.get::<_, String>(0)?, &r.get::<_, String>(1)?)))
				.optional()?;
			if now.as_ref() == Some(text) {
				tx.execute("INSERT OR REPLACE INTO vectors (mail, model, vector) VALUES (?1, ?2, ?3)", params![id, model, embed::to_blob(vector)])?;
				stored += 1;
			}
		}
		tx.commit()?;
		Ok(stored)
	}

	/// Forgets every mail's meaning, as when search by meaning is turned off.
	pub fn clear_vectors(&self) -> Result<()> {
		self.conn().execute("DELETE FROM vectors", [])?;
		Ok(())
	}
}

/// What a mail's meaning is worked out from: its subject and the start of its body.
fn meaning_text(subject: &str, body: &str) -> String {
	format!("{subject}\n{body}").chars().take(MEANING_CHARS).collect()
}

/// The indexed word `word` is most likely a typo of: the one with the fewest typos, then in the
/// most mails. Only for a word in fewer than `RARE` mails, and only a word in at least twice as
/// many, so a word spelled right but rare is left alone.
fn typo_fix(conn: &Connection, word: &str) -> Result<Option<String>> {
	const RARE: i64 = 3;
	let typed = word.to_lowercase();
	let len = typed.chars().count();
	let max = search::typos_allowed(len);
	if max == 0 || !typed.chars().all(char::is_alphanumeric) {
		return Ok(None);
	}
	// Mails holding a word that starts with what was typed, as the search matches prefixes.
	let have: i64 = conn.query_row("SELECT coalesce(sum(doc), 0) FROM temp.vocab WHERE term >= ?1 AND term < ?1 || char(1114111)", [&typed], |r| r.get(0))?;
	if have >= RARE {
		return Ok(None);
	}
	// Only words starting with the first or second letter typed: a typo is seldom the first letter,
	// and looking only there keeps a large index quick to search.
	let mut stmt = conn.prepare("SELECT term, doc FROM temp.vocab WHERE term >= ?1 AND term < ?1 || char(1114111) AND length(term) BETWEEN ?2 AND ?3 AND doc >= ?4")?;
	let mut starts: Vec<char> = typed.chars().take(2).collect();
	starts.dedup();
	let mut best: Option<(usize, i64, String)> = None;
	for start in starts {
		let range = params![start.to_string(), len.saturating_sub(max) as i64, (len + max) as i64, (2 * have).max(1)];
		let found = stmt
			.query_map(range, |r| Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)?)))?
			.filter_map(|r| r.ok())
			.filter(|(term, _)| !term.starts_with(&typed))
			.filter_map(|(term, doc)| search::typos(&typed, &term, max).map(|d| (d, -doc, term)))
			.min();
		if let Some(found) = found {
			best = Some(best.map_or(found.clone(), |b| b.min(found)));
		}
	}
	Ok(best.map(|(_, _, term)| term))
}

/// Writes the search words of a stored mail from its list row and, once opened, its body. A mail
/// whose text changed loses its vector, to be worked out again.
fn index(tx: &Connection, id: &MailId) -> Result<()> {
	let row: Option<(i64, String, Option<String>)> = tx
		.query_row(
			"SELECT m.id, m.summary, b.mail FROM mails m LEFT JOIN bodies b ON b.list = m.list AND b.element = m.element WHERE m.list = ?1 AND m.element = ?2",
			params![id.list, id.element],
			|r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
		)
		.optional()?;
	let Some((rowid, summary, mail)) = row else { return Ok(()) };
	let summary: MailSummary = serde_json::from_str(&summary)?;
	let mail: Option<Mail> = mail.map(|json| serde_json::from_str(&json)).transpose()?;
	let who = |a: &Address| format!("{} {}", a.name, a.address);
	let recipients: Vec<String> = match &mail {
		Some(m) => m.to.iter().chain(&m.cc).chain(&m.bcc).map(who).collect(),
		None => summary.to.iter().map(who).collect(),
	};
	let body = mail.map(|m| search::plain_text(&m.body_html)).unwrap_or_default();
	let before: Option<String> = tx
		.query_row("SELECT subject, body FROM words WHERE rowid = ?1", [rowid], |r| Ok(meaning_text(&r.get::<_, String>(0)?, &r.get::<_, String>(1)?)))
		.optional()?;
	if before.is_some_and(|b| b != meaning_text(&summary.subject, &body)) {
		tx.execute("DELETE FROM vectors WHERE mail = ?1", [rowid])?;
	}
	tx.execute("DELETE FROM words WHERE rowid = ?1", [rowid])?;
	tx.execute(
		"INSERT INTO words (rowid, subject, sender, recipients, body) VALUES (?1, ?2, ?3, ?4, ?5)",
		params![rowid, summary.subject, who(&summary.from), recipients.join(", "), body],
	)?;
	Ok(())
}

/// Drops a mail with its body, words and vector.
fn forget(tx: &Connection, id: &MailId) -> Result<()> {
	let rowid: Option<i64> = tx.query_row("SELECT id FROM mails WHERE list = ?1 AND element = ?2", params![id.list, id.element], |r| r.get(0)).optional()?;
	if let Some(rowid) = rowid {
		tx.execute("DELETE FROM words WHERE rowid = ?1", [rowid])?;
		tx.execute("DELETE FROM vectors WHERE mail = ?1", [rowid])?;
	}
	tx.execute("DELETE FROM mails WHERE list = ?1 AND element = ?2", params![id.list, id.element])?;
	tx.execute("DELETE FROM bodies WHERE list = ?1 AND element = ?2", params![id.list, id.element])?;
	tx.execute("DELETE FROM draft_quotes WHERE list = ?1 AND element = ?2", params![id.list, id.element])?;
	tx.execute("DELETE FROM cursors WHERE list = ?1 AND element = ?2", params![id.list, id.element])?;
	Ok(())
}

/// Records one meeting with `a`. A name already known is kept unless this one is from a contact.
fn note(conn: &Connection, a: &Address, met: Met) -> Result<()> {
	let address = a.address.trim().to_lowercase();
	if address.is_empty() || !address.contains('@') {
		return Ok(());
	}
	let (contact, used, seen) = match met {
		Met::Contact => (1, 0, 0),
		Met::Used => (0, 1, 0),
		Met::Seen => (0, 0, 1),
	};
	conn.execute(
		"INSERT INTO addresses (address, name, contact, used, seen, last) VALUES (?1, ?2, ?3, ?4, ?5, ?6)
		 ON CONFLICT (address) DO UPDATE SET
			name = CASE WHEN ?3 = 1 OR name = '' THEN ?2 ELSE name END,
			contact = max(contact, ?3), used = used + ?4, seen = seen + ?5, last = max(last, ?6)",
		params![address, a.name.trim(), contact, used, seen, now_ms()],
	)?;
	Ok(())
}

#[cfg(test)]
mod tests {
	use super::*;

	fn summary(n: u32, subject: &str, received_ms: i64) -> MailSummary {
		MailSummary {
			id: MailId { list: "l".into(), element: format!("e{n}") },
			from: Address::new("Mia", "mia@tuta.com"),
			subject: subject.into(),
			received_ms,
			unread: false,
			has_attachments: false,
			to: None,
			outgoing: false,
			draft: false,
		}
	}

	fn folder(id: &str, kind: FolderKind) -> Folder {
		Folder { id: FolderId(id.into()), name: id.into(), kind, unread: 0 }
	}

	fn setup() -> (Cache, FolderId, FolderId) {
		let cache = Cache::in_memory();
		cache.set_folders(&[folder("in", FolderKind::Inbox), folder("sent", FolderKind::Sent), folder("trash", FolderKind::Trash)]).unwrap();
		(cache, FolderId("in".into()), FolderId("sent".into()))
	}

	#[test]
	fn rules_find_their_mail_outside_skipped_folders() {
		let (cache, inbox, sent) = setup();
		cache.merge_page(&inbox, &[summary(3, "50% off", 300), summary(2, "Lunch", 200), summary(1, "Last chance: 50% off", 100)], true, true).unwrap();
		cache.merge_page(&sent, &[summary(4, "Re: 50% off", 400)], true, true).unwrap();
		let rule = tern_core::rules::parse("subject:\"50% off\"").unwrap();
		let (found, total) = cache.matching(&rule, &[FolderKind::Sent], 1).unwrap();
		assert_eq!(total, 2, "the sent one is skipped");
		assert_eq!(found.iter().map(|(m, f)| (m.id.element.as_str(), f)).collect::<Vec<_>>(), [("e3", &inbox)], "newest first, up to the limit");
		let (_, total) = cache.matching(&rule, &[], 10).unwrap();
		assert_eq!(total, 3);
	}

	#[test]
	fn new_mail_is_what_came_after_the_mark() {
		let (cache, inbox, _) = setup();
		cache.merge_page(&inbox, &[summary(3, "c", 300), summary(2, "b", 200), summary(1, "a", 100)], true, true).unwrap();
		assert_eq!(cache.mark("rules").unwrap(), None);
		cache.set_mark("rules", 100).unwrap();
		cache.set_mark("rules", 150).unwrap();
		let after = cache.mark("rules").unwrap().unwrap();
		let ids: Vec<String> = cache.received_after(&inbox, after).unwrap().into_iter().map(|m| m.id.element).collect();
		assert_eq!(ids, ["e2", "e3"], "oldest first");
	}

	#[test]
	fn pages_merge_and_mails_gone_from_the_server_are_dropped() {
		let (cache, inbox, _) = setup();
		assert_eq!(cache.merge_page(&inbox, &[summary(3, "c", 300), summary(2, "b", 200), summary(1, "a", 100)], true, true).unwrap(), 3);
		// Nothing changed: nothing counted.
		assert_eq!(cache.merge_page(&inbox, &[summary(3, "c", 300), summary(2, "b", 200), summary(1, "a", 100)], true, true).unwrap(), 0);
		// b was deleted on the server, and c is now read.
		let mut c = summary(3, "c", 300);
		c.unread = true;
		assert_eq!(cache.merge_page(&inbox, &[c, summary(1, "a", 100)], true, true).unwrap(), 2);
		let ids: Vec<String> = cache.list(&inbox, 0, 10).unwrap().into_iter().map(|m| m.id.element).collect();
		assert_eq!(ids, ["e3", "e1"]);
		assert_eq!(cache.folders().unwrap()[0].unread, 1);
	}

	#[test]
	fn a_middle_page_only_drops_mails_within_its_span() {
		let (cache, inbox, _) = setup();
		cache.merge_page(&inbox, &[summary(4, "d", 400), summary(3, "c", 300), summary(2, "b", 200), summary(1, "a", 100)], true, true).unwrap();
		// A page from 300 down to 200 without 3: only 3 goes.
		cache.merge_page(&inbox, &[summary(5, "x", 300), summary(2, "b", 200)], false, false).unwrap();
		let ids: Vec<String> = cache.list(&inbox, 0, 10).unwrap().into_iter().map(|m| m.id.element).collect();
		assert_eq!(ids, ["e4", "e5", "e2", "e1"]);
	}

	#[test]
	fn a_rule_lists_its_conversations_newest_first_outside_trash() {
		let (cache, inbox, sent) = setup();
		let trash = FolderId("trash".into());
		cache.merge_page(&inbox, &[summary(4, "Re: 50% off", 400), summary(3, "Lunch", 300), summary(1, "50% off", 100)], true, true).unwrap();
		cache.merge_page(&sent, &[summary(2, "Re: 50% off", 200)], true, true).unwrap();
		cache.merge_page(&trash, &[summary(5, "Old 50% off", 500)], true, true).unwrap();
		let rule = tern_core::rules::parse("subject:\"50% off\" or lunch").unwrap();
		let hits = cache.rule_hits(&rule, 10).unwrap();
		assert_eq!(hits.iter().map(|h| (h.row.latest.id.element.as_str(), h.row.count, h.folder.0.as_str())).collect::<Vec<_>>(), [("e4", 3, "in"), ("e3", 1, "in")], "one row per conversation; Trash left out");
		assert_eq!(cache.rule_hits(&rule, 1).unwrap().len(), 1);
	}

	#[test]
	fn a_draft_s_quote_goes_with_the_draft() {
		let (cache, inbox, _) = setup();
		let draft = summary(1, "Re: Lunch", 100);
		cache.merge_page(&inbox, std::slice::from_ref(&draft), true, true).unwrap();
		cache.set_draft_quote(&draft.id, Some("{\"picked\":null}")).unwrap();
		assert_eq!(cache.draft_quote(&draft.id).unwrap().as_deref(), Some("{\"picked\":null}"));
		cache.set_draft_quote(&draft.id, None).unwrap();
		assert_eq!(cache.draft_quote(&draft.id).unwrap(), None);
		cache.set_draft_quote(&draft.id, Some("q")).unwrap();
		cache.remove(&draft.id).unwrap();
		assert_eq!(cache.draft_quote(&draft.id).unwrap(), None, "replaced, sent or deleted, the draft takes its quote along");
	}

	#[test]
	fn pinned_conversations_come_first() {
		let (cache, inbox, _) = setup();
		cache.merge_page(&inbox, &[summary(3, "New", 300), summary(2, "Re: Old", 200), summary(1, "Older", 100)], true, true).unwrap();
		let order = |cache: &Cache| cache.threads(&inbox, 10).unwrap().iter().map(|r| (r.latest.id.element.clone(), r.pinned)).collect::<Vec<_>>();
		let key = cache.thread_key_of(&summary(2, "", 0).id).unwrap().unwrap();
		cache.set_pinned(&key, true).unwrap();
		cache.set_pinned(&key, true).unwrap();
		assert_eq!(order(&cache), [("e2".into(), true), ("e3".into(), false), ("e1".into(), false)]);
		// Only one row fits, and it is the pinned one.
		assert_eq!(cache.threads(&inbox, 1).unwrap()[0].latest.id.element, "e2");
		let hits = cache.search(&Query::parse("old"), None, 10).unwrap();
		assert!(hits.iter().all(|h| h.row.pinned == (h.row.key == key)), "search shows the pin too");

		cache.set_pinned(&key, false).unwrap();
		assert_eq!(order(&cache), [("e3".into(), false), ("e2".into(), false), ("e1".into(), false)]);
	}

	#[test]
	fn threads_group_by_subject_across_folders_but_not_trash() {
		let (cache, inbox, sent) = setup();
		let trash = FolderId("trash".into());
		cache.merge_page(&inbox, &[summary(4, "Re: Lunch", 400), summary(3, "Other", 300), summary(1, "Lunch", 100)], true, true).unwrap();
		cache.merge_page(&sent, &[summary(2, "RE: lunch", 200)], true, true).unwrap();
		cache.merge_page(&trash, &[summary(9, "Lunch", 50)], true, true).unwrap();

		let rows = cache.threads(&inbox, 10).unwrap();
		assert_eq!(rows.iter().map(|r| (r.latest.id.element.as_str(), r.count)).collect::<Vec<_>>(), [("e4", 3), ("e3", 1)]);
		let thread = cache.thread(&rows[0].key, &inbox).unwrap();
		assert_eq!(thread.iter().map(|m| (m.summary.id.element.as_str(), m.folder)).collect::<Vec<_>>(), [
			("e1", FolderKind::Inbox),
			("e2", FolderKind::Sent),
			("e4", FolderKind::Inbox)
		]);
		// Listing Trash shows the trashed one with the rest.
		assert_eq!(cache.thread(&rows[0].key, &trash).unwrap().len(), 4);
	}

	#[test]
	fn opened_mails_round_trip_and_follow_unread() {
		let (cache, inbox, _) = setup();
		let s = summary(1, "Hello", 100);
		cache.merge_page(&inbox, std::slice::from_ref(&s), true, true).unwrap();
		let mail = Mail {
			summary: s.clone(),
			to: vec![Address::new("Me", "me@tuta.com")],
			cc: vec![],
			bcc: vec![],
			reply_to: vec![],
			body_html: "<p>Hi</p>".into(),
			attachments: vec![],
			web_url: String::new(),
		};
		assert_eq!(cache.missing_bodies(10).unwrap(), vec![s.id.clone()]);
		assert!(!cache.has_mail(&s.id).unwrap());
		cache.put_mail(&mail).unwrap();
		assert!(cache.missing_bodies(10).unwrap().is_empty());
		assert!(cache.has_mail(&s.id).unwrap());
		cache.set_unread(&s.id, true).unwrap();
		assert!(cache.mail(&s.id).unwrap().unwrap().summary.unread);
		assert!(cache.list(&inbox, 0, 1).unwrap()[0].unread);
	}

	#[test]
	fn suggestions_rank_contacts_then_used_then_seen() {
		let cache = Cache::in_memory();
		cache.note_addresses(&[Address::new("Sam Seen", "sam.seen@x.org")], Met::Seen).unwrap();
		cache.note_addresses(&[Address::new("", "sam.used@x.org")], Met::Used).unwrap();
		cache.note_addresses(&[Address::new("Samira Contact", "contact@y.org")], Met::Contact).unwrap();
		cache.note_addresses(&[Address::new("Bob", "bob@x.org")], Met::Used).unwrap();
		let found: Vec<String> = cache.suggest("sam", 10).unwrap().into_iter().map(|a| a.address).collect();
		assert_eq!(found, ["contact@y.org", "sam.used@x.org", "sam.seen@x.org"]);
		// A word inside the name matches too; wildcards are literal.
		assert_eq!(cache.suggest("contact", 10).unwrap().len(), 1);
		assert!(cache.suggest("%", 10).unwrap().is_empty());
	}

	fn opened(s: &MailSummary, body: &str) -> Mail {
		Mail {
			summary: s.clone(),
			to: vec![Address::new("Sam Okafor", "sam@example.net")],
			cc: vec![Address::new("", "priya@example.org")],
			bcc: vec![],
			reply_to: vec![],
			body_html: body.into(),
			attachments: vec![],
			web_url: String::new(),
		}
	}

	/// The newest mail of each conversation `query` finds, best first.
	fn found(cache: &Cache, query: &str) -> Vec<String> {
		cache.search(&Query::parse(query), None, 50).unwrap().into_iter().map(|h| h.row.latest.id.element).collect()
	}

	#[test]
	fn search_matches_prefixes_and_filters() {
		let (cache, inbox, sent) = setup();
		let mut invoice = summary(1, "Your invoice for October", 100);
		invoice.from = Address::new("Accounts", "accounts@example.org");
		invoice.has_attachments = true;
		let lunch = summary(2, "Lunch on Friday?", 200);
		let mut reply = summary(3, "Re: Lunch on Friday?", 300);
		reply.from = Address::new("You", "you@tuta.com");
		cache.merge_page(&inbox, &[lunch.clone(), invoice.clone()], true, true).unwrap();
		cache.merge_page(&sent, std::slice::from_ref(&reply), true, true).unwrap();
		cache.put_mail(&opened(&lunch, "<p>Are you free at <b>12:30</b>? The café on Mühlenweg.</p>")).unwrap();

		assert_eq!(found(&cache, "inv"), ["e1"]);
		// A conversation shows once, as its newest mail; a body word finds it, accents or not.
		assert_eq!(found(&cache, "lunch"), ["e3"]);
		assert_eq!(found(&cache, "cafe muhlen"), ["e3"]);
		assert_eq!(found(&cache, "from:accounts"), ["e1"]);
		assert_eq!(found(&cache, "from:mia lunch"), ["e3"]);
		assert_eq!(found(&cache, "from:accounts lunch"), Vec::<String>::new());
		assert_eq!(found(&cache, "to:sam"), ["e3"]);
		assert_eq!(found(&cache, "to:priya@example"), ["e3"]);
		assert_eq!(found(&cache, "subject:october"), ["e1"]);
		assert_eq!(found(&cache, "subject:12"), Vec::<String>::new());
		assert_eq!(found(&cache, "has:attachment"), ["e1"]);
		assert_eq!(found(&cache, "in:sent"), ["e3"]);
		assert_eq!(found(&cache, "in:Inbox friday"), ["e3"]);
		assert_eq!(found(&cache, "\"on friday\""), ["e3"]);
		assert_eq!(found(&cache, "\"friday on\""), Vec::<String>::new());
		// Filters alone list newest first.
		assert_eq!(found(&cache, "in:inbox"), ["e3", "e1"]);
		// The subject weighs most: a subject match ranks above a body match.
		let mut cafe = summary(4, "Café list", 50);
		cafe.from = Address::new("Ann", "ann@x.org");
		cache.merge_page(&inbox, &[lunch, invoice, cafe], true, true).unwrap();
		assert_eq!(found(&cache, "cafe"), ["e4", "e3"]);
		// Opening the hit opens its folder's view of the conversation.
		let hit = cache.search(&Query::parse("12:30"), None, 10).unwrap().remove(0);
		assert_eq!((hit.folder, hit.row.count), (inbox, 2));
	}

	#[test]
	fn search_finds_what_a_typo_meant() {
		let (cache, inbox, _) = setup();
		let mails: Vec<MailSummary> = (1..=3).map(|n| summary(n, &format!("Touch rugby on Saturday, week {n}"), n as i64 * 100)).collect();
		let ruby = summary(4, "Ruby meetup", 400);
		cache.merge_page(&inbox, &[mails.clone(), vec![ruby]].concat(), true, true).unwrap();
		let sorted = |q: &str| {
			let mut f = found(&cache, q);
			f.sort();
			f
		};
		// One letter off, two swapped, one missing.
		assert_eq!(sorted("touch rutby"), ["e1", "e2", "e3"]);
		assert_eq!(sorted("rguby"), ["e1", "e2", "e3"]);
		assert_eq!(sorted("saturdy"), ["e1", "e2", "e3"]);
		// A word in enough mails is taken as meant: "ruby" is not also "rugby".
		let more: Vec<MailSummary> = (5..=7).map(|n| summary(n, &format!("Ruby notes {n}"), n as i64 * 100)).collect();
		cache.merge_page(&inbox, &[mails, more].concat(), true, true).unwrap();
		assert_eq!(found(&cache, "ruby").len(), 3);
		// Short words are not guessed at, and nothing far off is.
		assert!(found(&cache, "rub").len() == 3 && found(&cache, "xylophone").is_empty());
	}

	#[test]
	fn the_index_follows_moves_removals_and_new_bodies() {
		let (cache, inbox, sent) = setup();
		let trash = FolderId("trash".into());
		let a = summary(1, "Harbour photos", 100);
		cache.merge_page(&inbox, &[summary(2, "Other", 200), a.clone()], true, true).unwrap();
		cache.merge_page(&sent, &[summary(3, "Harbour walk", 300)], true, true).unwrap();
		cache.put_mail(&opened(&a, "sunset")).unwrap();
		assert_eq!(found(&cache, "sunset"), ["e1"]);
		// A new body replaces the old words.
		cache.put_mail(&opened(&a, "sunrise")).unwrap();
		assert_eq!((found(&cache, "sunset"), found(&cache, "sunrise")), (vec![], vec!["e1".to_string()]));
		// Trash and Spam are searched only when asked for.
		cache.move_to(&a.id, &trash).unwrap();
		assert_eq!(found(&cache, "harbour"), ["e3"]);
		assert_eq!(found(&cache, "harbour in:trash"), ["e1"]);
		cache.remove(&a.id).unwrap();
		assert_eq!(found(&cache, "sunrise in:trash"), Vec::<String>::new());
		// Gone from the server: gone from the index.
		cache.merge_page(&inbox, &[], true, true).unwrap();
		assert_eq!(found(&cache, "other"), Vec::<String>::new());
		// A folder that is gone takes its mails' words along.
		cache.set_folders(&[folder("in", FolderKind::Inbox)]).unwrap();
		assert_eq!(found(&cache, "harbour"), Vec::<String>::new());
		let conn = cache.conn();
		let left: i64 = conn.query_row("SELECT (SELECT count(*) FROM words) + (SELECT count(*) FROM vectors)", [], |r| r.get(0)).unwrap();
		assert_eq!(left, 0);
	}

	#[test]
	fn typed_input_never_breaks_the_query() {
		let (cache, inbox, _) = setup();
		cache.merge_page(&inbox, &[summary(1, "Plain \"quoted\" subject: * AND OR", 100)], true, true).unwrap();
		let nasty = ["\"", "\"\"", "*", "-x", "^a", "a:b:c", "NEAR(a b, 2)", "{subject sender}: x", "a OR", "AND", "(", ")", "'", "subject:\"", "from:*", "in:\"", "\u{345}", "\0", "x\"y\"z\"", ":", "::", "has:", "%_\\", "日本語", "ﬁ"];
		for q in nasty {
			assert!(cache.search(&Query::parse(q), Some(("m", &[1.0, 0.0])), 10).is_ok(), "{q:?}");
		}
		// And any mix of these characters, in a fixed pseudo-random order.
		let alphabet: Vec<char> = "ab \"*:^-()+{}'\\.,;&|!@#~é\u{345}\u{301}".chars().collect();
		let mut seed = 7u64;
		for _ in 0..2000 {
			let q: String = (0..12)
				.map(|_| {
					seed = seed.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
					alphabet[(seed >> 33) as usize % alphabet.len()]
				})
				.collect();
			assert!(cache.search(&Query::parse(&q), None, 10).is_ok(), "{q:?}");
		}
		assert_eq!(found(&cache, "subject: AND"), ["e1"]);
	}

	#[test]
	fn meaning_finds_mails_without_the_words_and_keeps_to_the_filters() {
		let (cache, inbox, _) = setup();
		let trash = FolderId("trash".into());
		let pizza = summary(1, "Pizza tonight?", 100);
		let pay = summary(2, "Your payment", 200);
		let dinner = summary(3, "Dinner plans", 300);
		cache.merge_page(&inbox, &[pay.clone(), pizza.clone()], true, true).unwrap();
		cache.merge_page(&trash, std::slice::from_ref(&dinner), true, true).unwrap();
		let model = crate::embed::tests::tiny();
		assert_eq!(sync::vectors(&cache, &model, "tiny").unwrap(), 3);
		assert_eq!(sync::vectors(&cache, &model, "tiny").unwrap(), 0, "worked out once");
		let search = |q: &str| -> Vec<String> {
			let q = Query::parse(q);
			let v = model.embed(&q.text());
			cache.search(&q, Some(("tiny", &v)), 10).unwrap().into_iter().map(|h| h.row.latest.id.element).collect()
		};
		// No mail has the word "lunch": meaning finds the pizza, not the payment, and not Trash.
		assert_eq!(found(&cache, "lunch"), Vec::<String>::new());
		assert_eq!(search("lunch"), ["e1"]);
		assert_eq!(search("lunch in:trash"), ["e3"]);
		assert_eq!(search("lunch from:nobody"), Vec::<String>::new());
		// A word match and a meaning match: the mail with both comes first.
		assert_eq!(search("payment"), ["e2"]);
		assert_eq!(search("pizza"), ["e1"]);
		// Vectors from another model do not count, and are worked out again for this one.
		assert_eq!(cache.search(&Query::parse("lunch"), Some(("other", &model.embed("lunch"))), 10).unwrap(), vec![]);
	}

	#[test]
	fn a_changed_mail_has_its_meaning_worked_out_again() {
		let (cache, inbox, _) = setup();
		let s = summary(1, "Plans", 100);
		cache.merge_page(&inbox, std::slice::from_ref(&s), true, true).unwrap();
		let model = crate::embed::tests::tiny();
		let stale = cache.unembedded("tiny", 10).unwrap();
		assert_eq!(stale, [(1, "Plans\n".to_string())]);
		sync::vectors(&cache, &model, "tiny").unwrap();
		assert!(cache.unembedded("tiny", 10).unwrap().is_empty());
		// Its body arrives: worked out again, from the body too.
		cache.put_mail(&opened(&s, "pizza")).unwrap();
		assert_eq!(cache.unembedded("tiny", 10).unwrap(), [(1, "Plans\npizza".to_string())]);
		// A vector worked out from the old text while the body arrived is not kept.
		let (id, text) = stale[0].clone();
		assert_eq!(cache.put_vectors("tiny", &[(id, text, model.embed("Plans"))]).unwrap(), 0);
		// Opening it again with the same body keeps the vector.
		sync::vectors(&cache, &model, "tiny").unwrap();
		cache.put_mail(&opened(&s, "pizza")).unwrap();
		assert!(cache.unembedded("tiny", 10).unwrap().is_empty());
		cache.clear_vectors().unwrap();
		assert_eq!(cache.unembedded("tiny", 10).unwrap().len(), 1);
	}

	#[test]
	fn an_older_layout_is_rebuilt_with_its_index_and_pins_kept() {
		let dir = std::env::temp_dir().join(format!("tern-cache-layout-{}", std::process::id()));
		let path = dir.join("mail.db");
		let cache = Cache::open(&path, &[7; 32]).unwrap();
		cache.set_folders(&[folder("in", FolderKind::Inbox)]).unwrap();
		cache.merge_page(&FolderId("in".into()), &[summary(1, "Kept?", 100)], true, true).unwrap();
		cache.set_pinned("kept?", true).unwrap();
		cache.conn().execute_batch("PRAGMA user_version = 1").unwrap();
		drop(cache);
		let cache = Cache::open(&path, &[7; 32]).unwrap();
		assert!(cache.folders().unwrap().is_empty());
		assert_eq!(found(&cache, "kept"), Vec::<String>::new());
		cache.set_folders(&[folder("in", FolderKind::Inbox)]).unwrap();
		cache.merge_page(&FolderId("in".into()), &[summary(1, "Kept?", 100)], true, true).unwrap();
		assert_eq!(found(&cache, "kept"), ["e1"]);
		assert!(cache.threads(&FolderId("in".into()), 10).unwrap()[0].pinned, "pins are not mail the server has: they stay");
		drop(cache);
		let _ = std::fs::remove_dir_all(dir);
	}

	#[test]
	fn a_database_opened_with_another_key_starts_afresh() {
		let dir = std::env::temp_dir().join(format!("tern-cache-test-{}", std::process::id()));
		let path = dir.join("mail.db");
		let (cache, inbox, _) = {
			let cache = Cache::open(&path, &[7; 32]).unwrap();
			cache.set_folders(&[folder("in", FolderKind::Inbox)]).unwrap();
			(cache, FolderId("in".into()), ())
		};
		let s = summary(1, "secret subject", 100);
		cache.merge_page(&inbox, std::slice::from_ref(&s), true, true).unwrap();
		// The search words and the meaning of an opened mail stay encrypted too.
		cache.put_mail(&opened(&s, "<p>The zanzibarite lunch plan</p>")).unwrap();
		let model = crate::embed::tests::tiny();
		assert_eq!(sync::vectors(&cache, &model, "tiny").unwrap(), 1);
		let vector = crate::embed::to_blob(&model.embed("secret subject\nThe zanzibarite lunch plan"));
		drop(cache);
		// No file holds readable text, the write-ahead log included.
		for suffix in ["", "-wal"] {
			let raw = std::fs::read(format!("{}{suffix}", path.display())).unwrap_or_default();
			assert!(!raw.windows(14).any(|w| w == b"secret subject"), "plain text in {suffix:?}");
			assert!(!raw.windows(11).any(|w| w == b"zanzibarite"), "an indexed word in {suffix:?}");
			assert!(!raw.windows(vector.len()).any(|w| w == vector), "a vector in {suffix:?}");
		}
		assert!(std::fs::metadata(&path).unwrap().len() > 0);
		assert_eq!(Cache::open(&path, &[7; 32]).unwrap().count(&inbox).unwrap(), 1);
		assert_eq!(found(&Cache::open(&path, &[7; 32]).unwrap(), "zanzib"), ["e1"]);
		assert_eq!(Cache::open(&path, &[8; 32]).unwrap().count(&inbox).unwrap(), 0);
		let _ = std::fs::remove_dir_all(dir);
	}
}

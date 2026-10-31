//! Tern's local copy of the mailbox: folders, mail lists, opened mails and the addresses seen, in an
//! encrypted SQLite database (SQLCipher), so the window shows mail at once and works offline.
//!
//! It is a cache: anything here can be fetched again, so a database that does not open with the key
//! or has an older layout is thrown away and started afresh.

pub mod sync;

use std::path::Path;
use std::sync::Mutex;

use rusqlite::{params, Connection, OptionalExtension};
use tern_core::{thread_key, Address, Folder, FolderId, FolderKind, Mail, MailId, MailSummary};

/// Bump when the tables change; older databases are dropped.
const SCHEMA_VERSION: i64 = 1;

const SCHEMA: &str = "
	CREATE TABLE folders (id TEXT PRIMARY KEY, name TEXT NOT NULL, kind TEXT NOT NULL, pos INTEGER NOT NULL);
	CREATE TABLE mails (
		list TEXT NOT NULL, element TEXT NOT NULL, folder TEXT NOT NULL, thread TEXT NOT NULL,
		received INTEGER NOT NULL, unread INTEGER NOT NULL, summary TEXT NOT NULL,
		PRIMARY KEY (list, element));
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
";

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
		Self::prepare(conn)
	}

	/// A cache in memory only, for the demo account and tests.
	pub fn in_memory() -> Cache {
		Self::prepare(Connection::open_in_memory().expect("open an in-memory database")).expect("set up an in-memory cache")
	}

	fn prepare(conn: Connection) -> Result<Cache> {
		let version: i64 = conn.query_row("PRAGMA user_version", [], |r| r.get(0))?;
		if version != SCHEMA_VERSION {
			let tables: Vec<String> = conn
				.prepare("SELECT name FROM sqlite_master WHERE type = 'table'")?
				.query_map([], |r| r.get(0))?
				.collect::<std::result::Result<_, _>>()?;
			for table in tables {
				conn.execute_batch(&format!("DROP TABLE IF EXISTS \"{}\"", table.replace('"', "")))?;
			}
			conn.execute_batch(SCHEMA)?;
			conn.execute_batch(&format!("PRAGMA user_version = {SCHEMA_VERSION}"))?;
		}
		Ok(Cache { conn: Mutex::new(conn) })
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
			for (list, element) in &gone {
				tx.execute("DELETE FROM mails WHERE list = ?1 AND element = ?2", params![list, element])?;
				tx.execute("DELETE FROM bodies WHERE list = ?1 AND element = ?2", params![list, element])?;
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

	/// The conversations of `folder`, newest first.
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
				 WHERE m.thread = here.thread AND (m.folder = ?1 OR coalesce(f.kind, '') NOT IN ('Trash', 'Spam')))
			 FROM here WHERE rank = 1 ORDER BY here.received DESC LIMIT ?2",
		)?;
		let rows = stmt.query_map(params![folder.0, limit as i64], |r| {
			Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?, r.get::<_, bool>(2)?, r.get::<_, u32>(3)?))
		})?;
		rows.map(|row| {
			let (key, summary, unread, count) = row?;
			Ok(ThreadRow { key, latest: serde_json::from_str(&summary)?, count, unread })
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

	/// Moves a mail to another folder, as trashing does.
	pub fn move_to(&self, id: &MailId, folder: &FolderId) -> Result<()> {
		self.conn().execute("UPDATE mails SET folder = ?3 WHERE list = ?1 AND element = ?2", params![id.list, id.element, folder.0])?;
		Ok(())
	}

	/// Forgets a mail, as when a draft is replaced or sent.
	pub fn remove(&self, id: &MailId) -> Result<()> {
		let conn = self.conn();
		conn.execute("DELETE FROM mails WHERE list = ?1 AND element = ?2", params![id.list, id.element])?;
		conn.execute("DELETE FROM bodies WHERE list = ?1 AND element = ?2", params![id.list, id.element])?;
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
		cache.merge_page(&inbox, &[s.clone()], true, true).unwrap();
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
		cache.put_mail(&mail).unwrap();
		assert!(cache.missing_bodies(10).unwrap().is_empty());
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

	#[test]
	fn a_database_opened_with_another_key_starts_afresh() {
		let dir = std::env::temp_dir().join(format!("tern-cache-test-{}", std::process::id()));
		let path = dir.join("mail.db");
		let (cache, inbox, _) = {
			let cache = Cache::open(&path, &[7; 32]).unwrap();
			cache.set_folders(&[folder("in", FolderKind::Inbox)]).unwrap();
			(cache, FolderId("in".into()), ())
		};
		cache.merge_page(&inbox, &[summary(1, "secret subject", 100)], true, true).unwrap();
		drop(cache);
		// No file holds readable text, the write-ahead log included.
		for suffix in ["", "-wal"] {
			let raw = std::fs::read(format!("{}{suffix}", path.display())).unwrap_or_default();
			assert!(!raw.windows(14).any(|w| w == b"secret subject"), "plain text in {suffix:?}");
		}
		assert!(std::fs::metadata(&path).unwrap().len() > 0);
		assert_eq!(Cache::open(&path, &[7; 32]).unwrap().count(&inbox).unwrap(), 1);
		assert_eq!(Cache::open(&path, &[8; 32]).unwrap().count(&inbox).unwrap(), 0);
		let _ = std::fs::remove_dir_all(dir);
	}
}

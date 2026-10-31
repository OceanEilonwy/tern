//! How much room the cache takes, and giving it back: mail past Keep Ready is trimmed after each
//! sync, and Clean Up drops more on request.

use rusqlite::{params, OptionalExtension};
use tern_core::{FolderId, MailId};

use crate::{forget, sync::PAGE, Cache, FolderState, Result};

/// Mails that always stay: those in Drafts and in pinned conversations.
const SPARED: &str = "(m.thread IN (SELECT thread FROM pins) OR m.folder IN (SELECT id FROM folders WHERE kind = 'Drafts'))";

impl Cache {
	/// Remembers the page cursor after `id`, the last mail of a page, so that once older mail is
	/// trimmed, fetching older mail continues from there.
	pub fn set_cursor(&self, id: &MailId, next: &str) -> Result<()> {
		self.conn().execute(
			"INSERT INTO cursors (list, element, next) VALUES (?1, ?2, ?3) ON CONFLICT (list, element) DO UPDATE SET next = ?3",
			params![id.list, id.element, next],
		)?;
		Ok(())
	}

	/// Drops the mail of `folder` older than its newest `keep`, with bodies, words and meaning.
	/// Cuts at the first cursor within a page past `keep` when there is one, so fetching older mail
	/// picks up right after; otherwise it starts over from the newest. Drafts and pinned
	/// conversations stay. Returns how many mails went.
	pub fn trim(&self, folder: &FolderId, keep: usize) -> Result<usize> {
		let mut conn = self.conn();
		let tx = conn.transaction()?;
		let rows: Vec<(String, String, Option<String>, bool)> = tx
			.prepare(&format!(
				"SELECT m.list, m.element, c.next, {SPARED} FROM mails m LEFT JOIN cursors c ON c.list = m.list AND c.element = m.element
				 WHERE m.folder = ?1 ORDER BY m.received DESC, m.id DESC"
			))?
			.query_map([&folder.0], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)))?
			.collect::<std::result::Result<_, _>>()?;
		if keep == 0 || rows.len() <= keep {
			return Ok(0);
		}
		let at_cursor = (keep - 1..rows.len().min(keep - 1 + PAGE)).find(|&i| rows[i].2.is_some());
		let (last_kept, next) = match at_cursor {
			Some(i) => (i, rows[i].2.clone()),
			None => (keep - 1, None),
		};
		let mut gone = 0;
		for (list, element, _, spared) in &rows[last_kept + 1..] {
			if !spared {
				forget(&tx, &MailId { list: list.clone(), element: element.clone() })?;
				gone += 1;
			}
		}
		if gone > 0 {
			let synced_ms: i64 = tx.query_row("SELECT synced FROM folder_state WHERE folder = ?1", [&folder.0], |r| r.get(0)).optional()?.unwrap_or(0);
			let state = FolderState { next, complete: false, synced_ms };
			tx.execute(
				"INSERT INTO folder_state (folder, next, complete, synced) VALUES (?1, ?2, ?3, ?4)
				 ON CONFLICT (folder) DO UPDATE SET next = ?2, complete = ?3, synced = ?4",
				params![folder.0, state.next, state.complete, state.synced_ms],
			)?;
		}
		tx.commit()?;
		Ok(gone)
	}

	/// Bytes the cache takes on disk: the database with its log, or its pages when in memory.
	pub fn disk_bytes(&self) -> u64 {
		match &self.path {
			Some(path) => ["", "-wal", "-shm"].iter().filter_map(|suffix| std::fs::metadata(format!("{}{suffix}", path.display())).ok()).map(|m| m.len()).sum(),
			None => self.pages("page_count").unwrap_or(0),
		}
	}

	/// Bytes of the database's `pragma` pages ("page_count" or "freelist_count").
	fn pages(&self, pragma: &str) -> Result<u64> {
		let conn = self.conn();
		let count: i64 = conn.query_row(&format!("PRAGMA {pragma}"), [], |r| r.get(0))?;
		let size: i64 = conn.query_row("PRAGMA page_size", [], |r| r.get(0))?;
		Ok((count * size).max(0) as u64)
	}

	/// About how many bytes [`Cache::clear_older`] frees: mail past `keep` per folder, the bodies of
	/// all but the newest `bodies` mails, and room already free in the file.
	pub fn older_bytes(&self, keep: usize, bodies: usize) -> Result<u64> {
		let past: i64 = {
			let conn = self.conn();
			conn.query_row(
				&format!(
					"SELECT coalesce(sum(length(CAST(m.summary AS BLOB))), 0) FROM
					 (SELECT *, row_number() OVER (PARTITION BY folder ORDER BY received DESC, id DESC) AS n FROM mails) m
					 WHERE m.n > ?1 AND NOT {SPARED}"
				),
				[keep as i64],
				|r| r.get(0),
			)?
		};
		let opened: i64 = self.conn().query_row(&format!("SELECT coalesce(sum(length(CAST(b.mail AS BLOB))), 0) FROM bodies b WHERE {}", Self::old_body()), [bodies as i64], |r| r.get(0))?;
		Ok(past.max(0) as u64 + opened.max(0) as u64 + self.pages("freelist_count")?)
	}

	/// Bodies past the newest `?1` mails that are not spared, for `bodies b`.
	fn old_body() -> String {
		format!(
			"NOT EXISTS (SELECT 1 FROM mails m WHERE m.list = b.list AND m.element = b.element
			 AND (m.id IN (SELECT id FROM mails ORDER BY received DESC LIMIT ?1) OR {SPARED}))"
		)
	}

	/// Trims every folder to `keep` mails and drops the bodies of all but the newest `bodies`
	/// mails; their words stay, so search still finds them, and they open from the server.
	pub fn clear_older(&self, keep: usize, bodies: usize) -> Result<()> {
		let folders: Vec<String> = self.conn().prepare("SELECT id FROM folders")?.query_map([], |r| r.get(0))?.collect::<std::result::Result<_, _>>()?;
		for folder in folders {
			self.trim(&FolderId(folder), keep)?;
		}
		self.conn().execute(&format!("DELETE FROM bodies WHERE rowid IN (SELECT b.rowid FROM bodies b WHERE {})", Self::old_body()), [bodies as i64])?;
		Ok(())
	}

	/// Bytes of the mails' meanings for search.
	pub fn vector_bytes(&self) -> Result<u64> {
		let bytes: i64 = self.conn().query_row("SELECT coalesce(sum(length(vector)), 0) FROM vectors", [], |r| r.get(0))?;
		Ok(bytes.max(0) as u64)
	}

	/// Drops every mail, to be fetched again like at a first sign-in. The folders, pins, draft
	/// quotes, addresses and marks (such as how far the rules have run) stay.
	pub fn clear_all(&self) -> Result<()> {
		self.conn().execute_batch(
			"BEGIN; DELETE FROM mails; DELETE FROM bodies; DELETE FROM words; DELETE FROM vectors;
			 DELETE FROM cursors; DELETE FROM folder_state; COMMIT;",
		)?;
		Ok(())
	}

	/// Gives free room in the file back to the disk.
	pub fn compact(&self) -> Result<()> {
		let conn = self.conn();
		conn.execute_batch("VACUUM;")?;
		// In memory there is no log; the pragma then reports a busy row, not an error.
		conn.query_row("PRAGMA wal_checkpoint(TRUNCATE)", [], |_| Ok(())).optional()?;
		Ok(())
	}
}

#[cfg(test)]
mod tests {
	use tern_core::{Address, Folder, FolderKind, Mail, MailSummary};

	use super::*;

	fn mail(n: i64, subject: &str) -> MailSummary {
		MailSummary {
			id: MailId { list: "l".into(), element: format!("e{n:03}") },
			from: Address::new("Mia", "mia@tuta.com"),
			subject: subject.into(),
			received_ms: n,
			unread: false,
			has_attachments: false,
			to: None,
			outgoing: false,
			draft: false,
		}
	}

	fn body(summary: &MailSummary) -> Mail {
		let word = format!("kiwi{} ", summary.received_ms);
		Mail {
			summary: summary.clone(),
			to: vec![],
			cc: vec![],
			bcc: vec![],
			reply_to: vec![],
			body_html: word.repeat(1000 / word.len() + 1),
			attachments: vec![],
			web_url: String::new(),
		}
	}

	/// An Inbox of mails 1..=n (n newest), each subject its own, stored as pages of PAGE newest first.
	fn inbox(n: i64) -> (Cache, FolderId) {
		let cache = Cache::in_memory();
		let inbox = Folder { id: FolderId("in".into()), name: "Inbox".into(), kind: FolderKind::Inbox, unread: 0 };
		cache.set_folders(std::slice::from_ref(&inbox)).unwrap();
		let all: Vec<MailSummary> = (1..=n).rev().map(|i| mail(i, &format!("Mail {i}"))).collect();
		for (p, page) in all.chunks(PAGE).enumerate() {
			cache.merge_page(&inbox.id, page, p == 0, (p + 1) * PAGE >= all.len()).unwrap();
			cache.set_cursor(&page.last().unwrap().id, &format!("after {}", page.last().unwrap().received_ms)).unwrap();
		}
		(cache, inbox.id)
	}

	fn subjects(cache: &Cache, folder: &FolderId) -> Vec<String> {
		cache.list(folder, 0, 1000).unwrap().into_iter().map(|m| m.subject).collect()
	}

	#[test]
	fn trimming_cuts_at_a_page_edge_and_fetching_older_continues_there() {
		let (cache, inbox) = inbox(160);
		// Newest 60 asked for: the second page ends at 100 newest, so the cut is there.
		assert_eq!(cache.trim(&inbox, 60).unwrap(), 60);
		assert_eq!(cache.count(&inbox).unwrap(), 100);
		assert_eq!(cache.folder_state(&inbox).unwrap().next.as_deref(), Some("after 61"));
		assert!(!cache.folder_state(&inbox).unwrap().complete);
		// Within what is kept, nothing more goes.
		assert_eq!(cache.trim(&inbox, 100).unwrap(), 0);
	}

	#[test]
	fn without_a_cursor_trimming_keeps_exactly_the_newest_and_starts_over() {
		let (cache, inbox) = inbox(10);
		cache.conn().execute("DELETE FROM cursors", []).unwrap();
		cache.set_folder_state(&inbox, &FolderState { next: Some("old".into()), complete: true, synced_ms: 5 }).unwrap();
		assert_eq!(cache.trim(&inbox, 4).unwrap(), 6);
		assert_eq!(subjects(&cache, &inbox), ["Mail 10", "Mail 9", "Mail 8", "Mail 7"]);
		assert_eq!(cache.folder_state(&inbox).unwrap(), FolderState { next: None, complete: false, synced_ms: 5 });
	}

	#[test]
	fn trimming_spares_pinned_conversations_and_drops_bodies_words_and_meaning() {
		let (cache, inbox) = inbox(10);
		cache.conn().execute("DELETE FROM cursors", []).unwrap();
		let old = mail(1, "Mail 1");
		cache.set_pinned(&crate::thread_of(&mail(2, "Mail 2")), true).unwrap();
		cache.put_mail(&body(&old)).unwrap();
		assert_eq!(cache.trim(&inbox, 3).unwrap(), 6);
		assert_eq!(subjects(&cache, &inbox), ["Mail 10", "Mail 9", "Mail 8", "Mail 2"]);
		assert!(!cache.has_mail(&old.id).unwrap());
		let words: i64 = cache.conn().query_row("SELECT count(*) FROM words", [], |r| r.get(0)).unwrap();
		assert_eq!(words, 4);
	}

	#[test]
	fn clearing_older_mail_keeps_the_newest_bodies_and_their_search_words() {
		let (cache, inbox) = inbox(10);
		// One page, whose cursor is past its oldest: without it the cut falls at exactly 8.
		cache.conn().execute("DELETE FROM cursors", []).unwrap();
		for i in 1..=10 {
			cache.put_mail(&body(&mail(i, &format!("Mail {i}")))).unwrap();
		}
		let before = cache.older_bytes(8, 3).unwrap();
		// Mails 1 and 2 past the 8 kept, and bodies of 3 to 7 past the newest 3.
		assert!(before >= 7 * 1000, "{before}");
		cache.clear_older(8, 3).unwrap();
		assert_eq!(cache.count(&inbox).unwrap(), 8);
		let opened: Vec<bool> = (3..=10).map(|i| cache.has_mail(&mail(i, "").id).unwrap()).collect();
		assert_eq!(opened, [false, false, false, false, false, true, true, true]);
		// Mail 5's body words are still found.
		assert_eq!(cache.search(&crate::search::Query::parse("kiwi5"), None, 10).unwrap().len(), 1);
		cache.compact().unwrap();
		assert!(cache.older_bytes(8, 3).unwrap() < before);
	}

	#[test]
	fn clearing_everything_keeps_folders_and_pins_and_starts_each_folder_afresh() {
		let (cache, inbox) = inbox(10);
		cache.set_pinned("thread", true).unwrap();
		cache.set_folder_state(&inbox, &FolderState { next: Some("c".into()), complete: true, synced_ms: 5 }).unwrap();
		cache.clear_all().unwrap();
		cache.compact().unwrap();
		assert_eq!(cache.count(&inbox).unwrap(), 0);
		assert_eq!(cache.folders().unwrap().len(), 1);
		assert_eq!(cache.folder_state(&inbox).unwrap(), FolderState::default());
		let pins: i64 = cache.conn().query_row("SELECT count(*) FROM pins", [], |r| r.get(0)).unwrap();
		assert_eq!(pins, 1);
		assert!(cache.disk_bytes() > 0);
	}
}

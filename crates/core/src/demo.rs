//! An account that lives in memory. Sign in as `demo` to use it; tests drive the UI with it.

use std::sync::Mutex;

use async_trait::async_trait;

use crate::{Account, Address, Attachment, Backend, Draft, Error, Folder, FolderId, FolderKind, Label, Mail, MailId, MailPage, MailSummary, Result};

const ME: (&str, &str) = ("You", "you@tuta.com");

pub const DEMO_LOGIN: &str = "demo";

struct Stored {
	folder: FolderKind,
	mail: Mail,
	files: Vec<Vec<u8>>,
	labels: Vec<FolderId>,
}

pub struct DemoBackend {
	mails: Mutex<Vec<Stored>>,
}

const FOLDERS: [(FolderKind, &str); 6] = [
	(FolderKind::Inbox, "Inbox"),
	(FolderKind::Drafts, "Drafts"),
	(FolderKind::Sent, "Sent"),
	(FolderKind::Archive, "Archive"),
	(FolderKind::Spam, "Spam"),
	(FolderKind::Trash, "Trash"),
];

fn folder_id(kind: FolderKind) -> FolderId {
	FolderId(format!("{kind:?}").to_lowercase())
}

const LABELS: [&str; 2] = ["Receipts", "Travel"];

fn label_id(name: &str) -> FolderId {
	FolderId(format!("label-{}", name.to_lowercase()))
}

/// A small sunset, so the demo's photos look like photos.
pub const SUNSET_PNG: &[u8] = include_bytes!("../assets/sunset.png");

/// A 2×2 PNG, for tests that only need a valid image.
pub const TINY_PNG: &[u8] = &[
	0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a, 0x00, 0x00, 0x00, 0x0d, 0x49, 0x48, 0x44, 0x52, 0x00, 0x00, 0x00, 0x02, 0x00, 0x00, 0x00, 0x02, 0x08, 0x02, 0x00,
	0x00, 0x00, 0xfd, 0xd4, 0x9a, 0x73, 0x00, 0x00, 0x00, 0x16, 0x49, 0x44, 0x41, 0x54, 0x78, 0x9c, 0x63, 0xf8, 0xcf, 0xc0, 0xf0, 0x1f, 0x08, 0x18, 0x18, 0x18, 0xfe,
	0x03, 0x00, 0x1c, 0xf0, 0x04, 0xfc, 0x8c, 0x2f, 0x3e, 0x4d, 0x00, 0x00, 0x00, 0x00, 0x49, 0x45, 0x4e, 0x44, 0xae, 0x42, 0x60, 0x82,
];

impl Default for DemoBackend {
	fn default() -> Self {
		Self::new()
	}
}

impl DemoBackend {
	pub fn new() -> Self {
		let now = 1_793_000_000_000_i64;
		let hour = 3_600_000;
		let me = Address::new("You", "you@tuta.com");
		let mut mails = Vec::new();
		let mut add = |n: usize, folder: FolderKind, from: Address, subject: &str, ago: i64, unread: bool, body: &str, files: Vec<(Attachment, Vec<u8>)>| {
			let id = MailId { list: "demo".into(), element: format!("m{n:03}") };
			let summary = MailSummary {
				id: id.clone(),
				from: from.clone(),
				subject: subject.into(),
				received_ms: now - ago * hour,
				unread,
				has_attachments: !files.is_empty(),
				outgoing: from.address == me.address,
				to: Some(me.clone()),
				draft: false,
			};
			let (attachments, data): (Vec<_>, Vec<_>) = files.into_iter().unzip();
			mails.push(Stored {
				folder,
				mail: Mail {
					summary,
					to: vec![me.clone()],
					cc: vec![],
					bcc: vec![],
					reply_to: vec![],
					body_html: body.into(),
					attachments,
					web_url: format!("https://app.tuta.com/mail/demo/{}", id.element),
				},
				files: data,
				labels: Vec::new(),
			});
		};
		let att = |n: &str, name: &str, mime: &str, data: &[u8], cid: Option<&str>| {
			(Attachment { id: n.into(), name: name.into(), mime: mime.into(), size: data.len() as u64, cid: cid.map(Into::into) }, data.to_vec())
		};

		add(
			1,
			FolderKind::Inbox,
			Address::new("Mia Chen", "mia@tuta.com"),
			"Photos from Saturday",
			1,
			true,
			"<p>Hi!</p><p>Here are the photos from <b>Saturday</b>. The one below is my favourite:</p>\
			 <p><img src=\"cid:sunset\" alt=\"Sunset\"></p><p>Speak soon,<br>Mia</p>",
			vec![att("a1", "sunset.png", "image/png", SUNSET_PNG, Some("sunset")), att("a2", "beach.png", "image/png", SUNSET_PNG, None)],
		);
		add(
			2,
			FolderKind::Inbox,
			Address::new("Accounts", "accounts@example.org"),
			"Your invoice for October",
			5,
			true,
			"<h2>Invoice 2026-10</h2><table border=\"1\"><tr><th>Item</th><th>Qty</th><th>Price</th></tr>\
			 <tr><td>Hosting</td><td>1</td><td>€12.00</td></tr><tr><td>Domain</td><td>2</td><td>€3.50</td></tr>\
			 <tr><td><b>Total</b></td><td></td><td><b>€19.00</b></td></tr></table>\
			 <p>The PDF is attached. Questions? <a href=\"https://example.org/help\">Visit our help page</a>.</p>",
			vec![att("a3", "invoice-2026-10.pdf", "application/pdf", b"%PDF-1.4\n% demo\n", None)],
		);
		add(
			3,
			FolderKind::Inbox,
			Address::new("Sam Okafor", "sam@example.net"),
			"Re: Lunch on Friday?",
			27,
			false,
			"<p>Friday works. <i>12:30</i> at the usual place?</p>\
			 <blockquote><p>Are you free for lunch on Friday?</p></blockquote>",
			vec![],
		);
		add(
			4,
			FolderKind::Inbox,
			Address::new("", "newsletter@example.com"),
			"This week: five small things",
			50,
			false,
			"<ul><li>Faster sync</li><li>Dark mode fixes</li><li>New <code>keyboard</code> shortcuts</li></ul>\
			 <ol><li>One</li><li>Two</li></ol><p><a href=\"https://example.com/unsubscribe\">Unsubscribe</a></p>",
			vec![],
		);
		// One sender's offers, for marking as spam together.
		let deals = Address::new("Mega Deals", "offers@deals.example");
		add(6, FolderKind::Inbox, deals.clone(), "50% off everything this weekend", 70, false, "<p>Everything must go.</p>", vec![]);
		add(7, FolderKind::Inbox, deals.clone(), "Last chance: 50% off ends tonight", 90, false, "<p>Hurry.</p>", vec![]);
		add(8, FolderKind::Inbox, deals, "Your exclusive coupon inside", 120, false, "<p>Just for you.</p>", vec![]);
		add(5, FolderKind::Sent, Address::new("You", "you@tuta.com"), "Lunch on Friday?", 30, false, "<div>Are you free for lunch on Friday?</div>", vec![]);
		// The one sent mail went to Sam.
		let sam = Address::new("Sam Okafor", "sam@example.net");
		for stored in mails.iter_mut().filter(|m| m.folder == FolderKind::Sent) {
			stored.mail.to = vec![sam.clone()];
			stored.mail.summary.to = Some(sam.clone());
		}
		Self { mails: Mutex::new(mails) }
	}

	/// Where a mail is and the labels it carries, for tests.
	pub fn place_of(&self, id: &MailId) -> Option<(FolderKind, Vec<FolderId>)> {
		self.find(id, |m| (m.folder, m.labels.clone())).ok()
	}

	/// Files `draft` in `folder` as a new mail, replacing the draft it was saved over.
	fn store(&self, draft: Draft, folder: FolderKind) -> MailId {
		let mut mails = self.mails.lock().unwrap();
		let mut files: Vec<(Attachment, Vec<u8>)> = Vec::new();
		if let Some(existing) = &draft.existing {
			if let Some(pos) = mails.iter().position(|m| &m.mail.summary.id == existing) {
				let old = mails.remove(pos);
				for (att, data) in old.mail.attachments.into_iter().zip(old.files) {
					if draft.kept.iter().any(|k| k.id == att.id) {
						files.push((att, data));
					}
				}
			}
		}
		let n = mails.iter().filter_map(|m| m.mail.summary.id.element.strip_prefix('m')?.parse::<usize>().ok()).max().unwrap_or(0) + 1;
		let id = MailId { list: "demo".into(), element: format!("m{n:03}") };
		for (i, a) in draft.attachments.iter().enumerate() {
			files.push((Attachment { id: format!("s{n}-{i}"), name: a.name.clone(), mime: a.mime.clone(), size: a.data.len() as u64, cid: None }, a.data.clone()));
		}
		let received_ms = mails.iter().map(|m| m.mail.summary.received_ms).max().unwrap_or(0) + 60_000;
		let (attachments, data): (Vec<_>, Vec<_>) = files.into_iter().unzip();
		mails.push(Stored {
			folder,
			mail: Mail {
				summary: MailSummary {
					id: id.clone(),
					from: Address::new(ME.0, ME.1),
					subject: draft.subject,
					received_ms,
					unread: false,
					has_attachments: !attachments.is_empty(),
					to: draft.to.first().or(draft.cc.first()).or(draft.bcc.first()).cloned(),
					outgoing: true,
					draft: folder == FolderKind::Drafts,
				},
				to: draft.to,
				cc: draft.cc,
				bcc: draft.bcc,
				reply_to: vec![],
				body_html: draft.body_html,
				attachments,
				web_url: format!("https://app.tuta.com/mail/demo/{}", id.element),
			},
			files: data,
			labels: Vec::new(),
		});
		id
	}

	fn find<T>(&self, id: &MailId, f: impl FnOnce(&mut Stored) -> T) -> Result<T> {
		let mut mails = self.mails.lock().unwrap();
		mails.iter_mut().find(|m| &m.mail.summary.id == id).map(f).ok_or_else(|| Error::Other("Mail not found".into()))
	}
}

#[async_trait]
impl Backend for DemoBackend {
	fn account(&self) -> Account {
		Account { address: ME.1.into(), name: ME.0.into() }
	}

	async fn folders(&self) -> Result<Vec<Folder>> {
		let mails = self.mails.lock().unwrap();
		Ok(FOLDERS
			.iter()
			.map(|&(kind, name)| Folder {
				id: folder_id(kind),
				name: name.into(),
				kind,
				unread: mails.iter().filter(|m| m.folder == kind && m.mail.summary.unread).count() as u32,
			})
			.collect())
	}

	async fn list(&self, folder: &FolderId, next: Option<String>, count: usize) -> Result<MailPage> {
		let mails = self.mails.lock().unwrap();
		let mut found: Vec<MailSummary> =
			mails.iter().filter(|m| &folder_id(m.folder) == folder).map(|m| m.mail.summary.clone()).collect();
		found.sort_by_key(|m| std::cmp::Reverse(m.received_ms));
		let start: usize = next.and_then(|n| n.parse().ok()).unwrap_or(0);
		let page: Vec<_> = found.iter().skip(start).take(count).cloned().collect();
		let next = (start + page.len() < found.len()).then(|| (start + page.len()).to_string());
		Ok(MailPage { mails: page, next })
	}

	async fn open(&self, id: &MailId) -> Result<Mail> {
		self.find(id, |m| m.mail.clone())
	}

	async fn open_many(&self, ids: &[MailId]) -> Result<Vec<(MailId, Result<Mail>)>> {
		Ok(ids.iter().map(|id| (id.clone(), self.find(id, |m| m.mail.clone()))).collect())
	}

	async fn attachment(&self, mail: &MailId, attachment: &Attachment) -> Result<Vec<u8>> {
		self.find(mail, |m| {
			m.mail.attachments.iter().position(|a| a.id == attachment.id).map(|i| m.files[i].clone()).ok_or_else(|| Error::Other("Attachment not found".into()))
		})?
	}

	async fn send(&self, draft: Draft) -> Result<()> {
		if draft.to.is_empty() && draft.cc.is_empty() && draft.bcc.is_empty() {
			return Err(Error::Other("Add a recipient".into()));
		}
		let missing: Vec<String> =
			draft.to.iter().chain(&draft.cc).chain(&draft.bcc).filter(|a| a.address.ends_with("@nowhere.invalid")).map(|a| a.address.clone()).collect();
		if !missing.is_empty() {
			return Err(Error::RecipientsNotFound(missing));
		}
		self.store(draft, FolderKind::Sent);
		Ok(())
	}

	async fn save_draft(&self, draft: Draft) -> Result<MailId> {
		Ok(self.store(draft, FolderKind::Drafts))
	}

	async fn contacts(&self) -> Result<Vec<Address>> {
		Ok(vec![Address::new("Mia Chen", "mia@tuta.com"), Address::new("Sam Okafor", "sam@example.net"), Address::new("Priya Natarajan", "priya@example.org")])
	}

	async fn set_unread(&self, id: &MailId, unread: bool) -> Result<()> {
		self.find(id, |m| m.mail.summary.unread = unread)
	}

	async fn trash(&self, id: &MailId) -> Result<()> {
		self.find(id, |m| m.folder = FolderKind::Trash)
	}

	async fn move_mails(&self, ids: &[MailId], folder: &FolderId) -> Result<()> {
		let kind = FOLDERS.iter().map(|&(k, _)| k).find(|&k| &folder_id(k) == folder).ok_or_else(|| Error::Other("Folder not found".into()))?;
		for id in ids {
			self.find(id, |m| m.folder = kind)?;
		}
		Ok(())
	}

	async fn labels(&self) -> Result<Vec<Label>> {
		Ok(LABELS.iter().map(|&name| Label { id: label_id(name), name: name.into() }).collect())
	}

	async fn add_label(&self, ids: &[MailId], label: &FolderId) -> Result<()> {
		if !LABELS.iter().any(|&name| &label_id(name) == label) {
			return Err(Error::Other("Label not found".into()));
		}
		for id in ids {
			self.find(id, |m| {
				if !m.labels.contains(label) {
					m.labels.push(label.clone());
				}
			})?;
		}
		Ok(())
	}

	async fn sign_out(&self) -> Result<()> {
		Ok(())
	}
}

#[cfg(test)]
mod tests {
	use super::*;
	use crate::NewAttachment;

	#[tokio::test]
	async fn sent_mail_keeps_its_attachments() {
		let demo = DemoBackend::new();
		let pdf = b"%PDF-1.4 hello".to_vec();
		demo.send(Draft {
			to: vec![Address::new("", "mia@tuta.com")],
			subject: "Report".into(),
			body_html: crate::text_to_html("See attached"),
			attachments: vec![NewAttachment { name: "r.pdf".into(), mime: "application/pdf".into(), data: pdf.clone() }],
			..Default::default()
		})
		.await
		.unwrap();
		let sent = demo.list(&folder_id(FolderKind::Sent), None, 10).await.unwrap();
		let mail = demo.open(&sent.mails[0].id).await.unwrap();
		assert_eq!(mail.summary.subject, "Report");
		assert_eq!(demo.attachment(&mail.summary.id, &mail.attachments[0]).await.unwrap(), pdf);
	}

	#[tokio::test]
	async fn paging_walks_the_whole_folder() {
		let demo = DemoBackend::new();
		let inbox = folder_id(FolderKind::Inbox);
		let first = demo.list(&inbox, None, 3).await.unwrap();
		assert_eq!(first.mails.len(), 3);
		let second = demo.list(&inbox, first.next, 3).await.unwrap();
		assert_eq!(second.mails.len(), 3);
		let third = demo.list(&inbox, second.next, 3).await.unwrap();
		assert_eq!(third.mails.len(), 1);
		assert!(third.next.is_none());
	}

	#[tokio::test]
	async fn a_saved_draft_is_replaced_and_keeps_chosen_attachments() {
		let demo = DemoBackend::new();
		let drafts = folder_id(FolderKind::Drafts);
		let first = demo
			.save_draft(Draft {
				subject: "Plan".into(),
				attachments: vec![
					NewAttachment { name: "a.txt".into(), mime: "text/plain".into(), data: b"a".to_vec() },
					NewAttachment { name: "b.txt".into(), mime: "text/plain".into(), data: b"b".to_vec() },
				],
				..Default::default()
			})
			.await
			.unwrap();
		let saved = demo.open(&first).await.unwrap();
		assert!(saved.summary.draft);
		let kept = vec![saved.attachments[1].clone()];
		let second = demo.save_draft(Draft { subject: "Plan v2".into(), existing: Some(first.clone()), kept, ..Default::default() }).await.unwrap();
		let page = demo.list(&drafts, None, 10).await.unwrap();
		assert_eq!(page.mails.iter().map(|m| m.subject.as_str()).collect::<Vec<_>>(), ["Plan v2"]);
		let reopened = demo.open(&second).await.unwrap();
		assert_eq!(reopened.attachments.iter().map(|a| a.name.as_str()).collect::<Vec<_>>(), ["b.txt"]);
		assert_eq!(demo.attachment(&second, &reopened.attachments[0]).await.unwrap(), b"b");

		// Sending it moves it out of Drafts.
		demo.send(Draft { to: vec![Address::new("", "mia@tuta.com")], subject: "Plan v2".into(), existing: Some(second), ..Default::default() }).await.unwrap();
		assert!(demo.list(&drafts, None, 10).await.unwrap().mails.is_empty());
	}

	#[tokio::test]
	async fn a_batch_opens_what_it_can_and_names_what_it_cannot() {
		let demo = DemoBackend::new();
		let inbox = demo.list(&folder_id(FolderKind::Inbox), None, 10).await.unwrap();
		let gone = MailId { list: "demo".into(), element: "gone".into() };
		let ids = vec![inbox.mails[1].id.clone(), gone.clone(), inbox.mails[0].id.clone()];
		let opened = demo.open_many(&ids).await.unwrap();
		assert_eq!(opened.iter().map(|(id, _)| id.clone()).collect::<Vec<_>>(), ids, "in the order asked");
		assert_eq!(opened[0].1.as_ref().unwrap().summary.id, ids[0]);
		assert!(opened[1].1.is_err(), "the missing mail fails alone");
		assert_eq!(opened[2].1.as_ref().unwrap().summary.subject, inbox.mails[0].subject);
	}

	#[tokio::test]
	async fn unknown_recipients_are_named() {
		let demo = DemoBackend::new();
		let err = demo.send(Draft { to: vec![Address::new("", "x@nowhere.invalid")], ..Default::default() }).await.unwrap_err();
		assert_eq!(err, Error::RecipientsNotFound(vec!["x@nowhere.invalid".into()]));
	}
}

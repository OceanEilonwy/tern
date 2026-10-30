//! An account that lives in memory. Sign in as `demo` to use it; tests drive the UI with it.

use std::sync::Mutex;

use async_trait::async_trait;

use crate::{Account, Address, Attachment, Backend, Draft, Error, Folder, FolderId, FolderKind, Mail, MailId, MailPage, MailSummary, Result};

pub const DEMO_LOGIN: &str = "demo";

struct Stored {
	folder: FolderKind,
	mail: Mail,
	files: Vec<Vec<u8>>,
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

/// A 2×2 PNG, so inline and attached images have something to draw.
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
				from,
				subject: subject.into(),
				received_ms: now - ago * hour,
				unread,
				has_attachments: !files.is_empty(),
			};
			let (attachments, data): (Vec<_>, Vec<_>) = files.into_iter().unzip();
			mails.push(Stored {
				folder,
				mail: Mail {
					summary,
					to: vec![me.clone()],
					cc: vec![],
					reply_to: vec![],
					body_html: body.into(),
					attachments,
					web_url: format!("https://app.tuta.com/mail/demo/{}", id.element),
				},
				files: data,
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
			vec![att("a1", "sunset.png", "image/png", TINY_PNG, Some("sunset")), att("a2", "beach.png", "image/png", TINY_PNG, None)],
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
		add(5, FolderKind::Sent, Address::new("You", "you@tuta.com"), "Are you free for lunch on Friday?", 30, false, "<div>Are you free for lunch on Friday?</div>", vec![]);
		Self { mails: Mutex::new(mails) }
	}

	fn find<T>(&self, id: &MailId, f: impl FnOnce(&mut Stored) -> T) -> Result<T> {
		let mut mails = self.mails.lock().unwrap();
		mails.iter_mut().find(|m| &m.mail.summary.id == id).map(f).ok_or_else(|| Error::Other("Mail not found".into()))
	}
}

#[async_trait]
impl Backend for DemoBackend {
	fn account(&self) -> Account {
		Account { address: "you@tuta.com".into(), name: "You".into() }
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
		let mut mails = self.mails.lock().unwrap();
		let n = mails.len() + 1;
		let id = MailId { list: "demo".into(), element: format!("m{n:03}") };
		let attachments = draft
			.attachments
			.iter()
			.enumerate()
			.map(|(i, a)| Attachment { id: format!("s{n}-{i}"), name: a.name.clone(), mime: a.mime.clone(), size: a.data.len() as u64, cid: None })
			.collect();
		let received_ms = mails.iter().map(|m| m.mail.summary.received_ms).max().unwrap_or(0) + 60_000;
		mails.push(Stored {
			folder: FolderKind::Sent,
			mail: Mail {
				summary: MailSummary {
					id: id.clone(),
					from: Address::new("You", "you@tuta.com"),
					subject: draft.subject,
					received_ms,
					unread: false,
					has_attachments: !draft.attachments.is_empty(),
				},
				to: draft.to,
				cc: draft.cc,
				reply_to: vec![],
				body_html: draft.body_html,
				attachments,
				web_url: format!("https://app.tuta.com/mail/demo/{}", id.element),
			},
			files: draft.attachments.into_iter().map(|a| a.data).collect(),
		});
		Ok(())
	}

	async fn set_unread(&self, id: &MailId, unread: bool) -> Result<()> {
		self.find(id, |m| m.mail.summary.unread = unread)
	}

	async fn trash(&self, id: &MailId) -> Result<()> {
		self.find(id, |m| m.folder = FolderKind::Trash)
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
		assert_eq!(second.mails.len(), 1);
		assert!(second.next.is_none());
	}

	#[tokio::test]
	async fn unknown_recipients_are_named() {
		let demo = DemoBackend::new();
		let err = demo.send(Draft { to: vec![Address::new("", "x@nowhere.invalid")], ..Default::default() }).await.unwrap_err();
		assert_eq!(err, Error::RecipientsNotFound(vec!["x@nowhere.invalid".into()]));
	}
}

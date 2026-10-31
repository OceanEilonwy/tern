//! What the Tern window needs from a mail account, with no GTK and no Tuta in it.
//!
//! The app talks only to [`Backend`]. `tern-tuta` implements it against the Tuta servers and
//! [`demo::DemoBackend`] implements it in memory, so the UI runs and is tested without an account.

pub mod demo;
pub mod rules;

use async_trait::async_trait;
use serde::{Deserialize, Serialize};

/// A mail, by the list it lives in and its own id.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct MailId {
	pub list: String,
	pub element: String,
}

/// A folder, opaque to the UI.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct FolderId(pub String);

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum FolderKind {
	Inbox,
	Drafts,
	Sent,
	Archive,
	Spam,
	Trash,
	Custom,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Folder {
	pub id: FolderId,
	pub name: String,
	pub kind: FolderKind,
	pub unread: u32,
}

/// A label (Tuta's tags): unlike a folder, a mail can carry several.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Label {
	pub id: FolderId,
	pub name: String,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Address {
	pub name: String,
	pub address: String,
}

impl Address {
	pub fn new(name: impl Into<String>, address: impl Into<String>) -> Self {
		Self { name: name.into(), address: address.into() }
	}

	/// The name if there is one, else the address.
	pub fn display(&self) -> &str {
		if self.name.trim().is_empty() {
			&self.address
		} else {
			&self.name
		}
	}
}

/// One row of a mail list.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct MailSummary {
	pub id: MailId,
	pub from: Address,
	pub subject: String,
	/// Unix time in milliseconds.
	pub received_ms: i64,
	pub unread: bool,
	pub has_attachments: bool,
	/// The first recipient, shown instead of the sender for mail the account sent.
	pub to: Option<Address>,
	/// Sent by this account (Sent, Drafts, or a copy to yourself).
	pub outgoing: bool,
	/// Not sent yet: opening it continues writing it.
	#[serde(default)]
	pub draft: bool,
}

/// A page of a folder, newest first. Pass `next` back to [`Backend::list`] for the page after.
#[derive(Clone, Debug, Default)]
pub struct MailPage {
	pub mails: Vec<MailSummary>,
	pub next: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Attachment {
	/// Opaque to the UI; the backend reads it back in [`Backend::attachment`].
	pub id: String,
	pub name: String,
	pub mime: String,
	pub size: u64,
	/// Content-ID, set when the HTML body shows this attachment inline as `cid:<cid>`.
	pub cid: Option<String>,
}

/// A mail opened for reading.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Mail {
	pub summary: MailSummary,
	pub to: Vec<Address>,
	pub cc: Vec<Address>,
	/// Only known for mail this account wrote.
	#[serde(default)]
	pub bcc: Vec<Address>,
	pub reply_to: Vec<Address>,
	/// The body as the sender wrote it: HTML, unsanitized. The UI renders only a safe subset.
	pub body_html: String,
	pub attachments: Vec<Attachment>,
	/// Where this mail opens in Tuta's own web app, for mails too rich to show here.
	pub web_url: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NewAttachment {
	pub name: String,
	pub mime: String,
	pub data: Vec<u8>,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum ReplyKind {
	#[default]
	New,
	Reply,
	Forward,
}

/// A mail to send.
#[derive(Clone, Debug, Default)]
pub struct Draft {
	pub to: Vec<Address>,
	pub cc: Vec<Address>,
	pub bcc: Vec<Address>,
	pub subject: String,
	/// HTML body. The composer builds it from plain text with [`text_to_html`].
	pub body_html: String,
	pub attachments: Vec<NewAttachment>,
	/// The mail this answers or forwards, if any. Ignored when `existing` is set: a saved draft
	/// keeps what it answers.
	pub previous: Option<(MailId, ReplyKind)>,
	/// The saved draft this replaces.
	pub existing: Option<MailId>,
	/// Attachments of `existing` that stay on the draft; the others are removed.
	pub kept: Vec<Attachment>,
}

/// The signed-in account.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Account {
	pub address: String,
	pub name: String,
}

#[derive(Clone, Debug, thiserror::Error, PartialEq, Eq)]
pub enum Error {
	#[error("Wrong email address or password")]
	BadLogin,
	#[error("This account uses two-factor authentication, which Tern does not support yet")]
	SecondFactorRequired,
	#[error("Signed out, please sign in again")]
	SessionExpired,
	#[error("No connection to the mail server")]
	Offline,
	#[error("These recipients do not exist: {}", .0.join(", "))]
	RecipientsNotFound(Vec<String>),
	/// The server wants fewer requests for a while: wait this many seconds, if it says, then try again.
	#[error("The mail server is busy, trying again shortly")]
	RateLimited(Option<u64>),
	#[error("{0}")]
	Other(String),
}

pub type Result<T> = std::result::Result<T, Error>;

#[async_trait]
pub trait Backend: Send + Sync {
	fn account(&self) -> Account;
	async fn folders(&self) -> Result<Vec<Folder>>;
	async fn list(&self, folder: &FolderId, next: Option<String>, count: usize) -> Result<MailPage>;
	async fn open(&self, id: &MailId) -> Result<Mail>;
	/// Opens several mails with as few requests as the server allows, in the order asked. An error
	/// here is the batch failing as a whole; a mail that cannot be opened gets its own error.
	async fn open_many(&self, ids: &[MailId]) -> Result<Vec<(MailId, Result<Mail>)>>;
	async fn attachment(&self, mail: &MailId, attachment: &Attachment) -> Result<Vec<u8>>;
	/// Sends `draft`, after saving it over `draft.existing` if set.
	async fn send(&self, draft: Draft) -> Result<()>;
	/// Saves `draft` to Drafts, over `draft.existing` if set, and returns where it is now.
	async fn save_draft(&self, draft: Draft) -> Result<MailId>;
	/// The account's contacts that have an email address.
	async fn contacts(&self) -> Result<Vec<Address>>;
	async fn set_unread(&self, id: &MailId, unread: bool) -> Result<()>;
	async fn trash(&self, id: &MailId) -> Result<()>;
	/// Moves mails into `folder`.
	async fn move_mails(&self, ids: &[MailId], folder: &FolderId) -> Result<()>;
	async fn labels(&self) -> Result<Vec<Label>>;
	/// Makes a label called `name`.
	async fn create_label(&self, name: &str) -> Result<Label>;
	/// Adds `label` to mails.
	async fn add_label(&self, ids: &[MailId], label: &FolderId) -> Result<()>;
	/// Forget the session on the server too.
	async fn sign_out(&self) -> Result<()>;
}

/// Turns what the user typed in the composer into the HTML body Tuta sends.
pub fn text_to_html(text: &str) -> String {
	let mut html = String::with_capacity(text.len() + 32);
	for (i, line) in text.split('\n').enumerate() {
		if i > 0 {
			html.push_str("<br>");
		}
		for c in line.trim_end_matches('\r').chars() {
			match c {
				'&' => html.push_str("&amp;"),
				'<' => html.push_str("&lt;"),
				'>' => html.push_str("&gt;"),
				'"' => html.push_str("&quot;"),
				c => html.push(c),
			}
		}
	}
	format!("<div>{html}</div>")
}

/// The key mails of one conversation share: the subject without "Re:", "Fwd:" and the like, in
/// lower case with spaces collapsed. Mails without a subject have none and stand alone.
pub fn thread_key(subject: &str) -> Option<String> {
	let key = bare_subject(subject).split_whitespace().collect::<Vec<_>>().join(" ").to_lowercase();
	(!key.is_empty()).then_some(key)
}

/// The subject without "Re:", "Fwd:" and the like before it.
pub fn bare_subject(subject: &str) -> &str {
	const PREFIXES: [&str; 10] = ["re", "fwd", "fw", "aw", "wg", "sv", "vs", "antw", "tr", "rv"];
	let mut rest = subject.trim();
	loop {
		let lower = rest.to_lowercase();
		// "Re:", "RE[2]:", "Fwd :" …
		let stripped = PREFIXES.iter().find_map(|p| {
			let after = lower.strip_prefix(p)?;
			let after = after.trim_start();
			let after = match after.strip_prefix('[') {
				Some(a) => a.split_once(']')?.1.trim_start(),
				None => after,
			};
			after.strip_prefix(':').map(|a| lower.len() - a.len())
		});
		match stripped {
			Some(cut) => rest = rest[cut..].trim_start(),
			None => break,
		}
	}
	rest
}

/// Splits `"Ann <ann@x.org>, bob@y.org"` into addresses. Returns the parts that are not addresses as the error.
pub fn parse_addresses(input: &str) -> std::result::Result<Vec<Address>, Vec<String>> {
	let mut good = Vec::new();
	let mut bad = Vec::new();
	for part in input.split([',', ';']).map(str::trim).filter(|p| !p.is_empty()) {
		let (name, address) = match (part.rfind('<'), part.ends_with('>')) {
			(Some(open), true) => (part[..open].trim().trim_matches('"').trim(), part[open + 1..part.len() - 1].trim()),
			_ => ("", part),
		};
		if is_address(address) {
			good.push(Address::new(name, address.to_lowercase()));
		} else {
			bad.push(part.to_string());
		}
	}
	if bad.is_empty() {
		Ok(good)
	} else {
		Err(bad)
	}
}

fn is_address(s: &str) -> bool {
	let Some((local, domain)) = s.split_once('@') else { return false };
	!local.is_empty() && domain.contains('.') && !domain.starts_with('.') && !domain.ends_with('.') && !s.contains(char::is_whitespace) && !domain.contains('@')
}

/// Guesses a MIME type from a file name, for attachments picked from disk.
pub fn mime_for(name: &str) -> &'static str {
	let ext = name.rsplit_once('.').map(|(_, e)| e.to_ascii_lowercase()).unwrap_or_default();
	match ext.as_str() {
		"pdf" => "application/pdf",
		"png" => "image/png",
		"jpg" | "jpeg" => "image/jpeg",
		"gif" => "image/gif",
		"webp" => "image/webp",
		"svg" => "image/svg+xml",
		"txt" | "log" | "md" => "text/plain",
		"html" | "htm" => "text/html",
		"csv" => "text/csv",
		"json" => "application/json",
		"zip" => "application/zip",
		"gz" => "application/gzip",
		"tar" => "application/x-tar",
		"doc" => "application/msword",
		"docx" => "application/vnd.openxmlformats-officedocument.wordprocessingml.document",
		"xls" => "application/vnd.ms-excel",
		"xlsx" => "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet",
		"ppt" => "application/vnd.ms-powerpoint",
		"pptx" => "application/vnd.openxmlformats-officedocument.presentationml.presentation",
		"odt" => "application/vnd.oasis.opendocument.text",
		"ods" => "application/vnd.oasis.opendocument.spreadsheet",
		"ics" => "text/calendar",
		"mp3" => "audio/mpeg",
		"mp4" => "video/mp4",
		_ => "application/octet-stream",
	}
}

#[cfg(test)]
mod tests {
	use super::*;

	#[test]
	fn text_to_html_escapes_and_keeps_lines() {
		assert_eq!(text_to_html("a < b\r\n& c"), "<div>a &lt; b<br>&amp; c</div>");
	}

	#[test]
	fn parse_addresses_reads_names_and_rejects_junk() {
		let parsed = parse_addresses("\"Ann Lee\" <Ann@X.org>; bob@y.org,").unwrap();
		assert_eq!(parsed, vec![Address::new("Ann Lee", "ann@x.org"), Address::new("", "bob@y.org")]);
		assert_eq!(parse_addresses("bob@y.org, nope, a@b").unwrap_err(), vec!["nope".to_string(), "a@b".to_string()]);
	}

	#[test]
	fn bare_subjects_keep_their_case() {
		assert_eq!(bare_subject("Re: FWD: RE[2]: 50% Off"), "50% Off");
		assert_eq!(bare_subject("Rear window"), "Rear window");
	}

	#[test]
	fn thread_key_ignores_reply_prefixes() {
		assert_eq!(thread_key("Re: RE[2]: Fwd:  Lunch   on Friday?"), Some("lunch on friday?".into()));
		assert_eq!(thread_key("AW: Termin"), thread_key("termin"));
		assert_eq!(thread_key("Rebate offer"), Some("rebate offer".into()));
		assert_eq!(thread_key("Re:"), None);
		assert_eq!(thread_key("  "), None);
	}

	#[test]
	fn mime_for_knows_common_files() {
		assert_eq!(mime_for("Report.PDF"), "application/pdf");
		assert_eq!(mime_for("noext"), "application/octet-stream");
	}
}

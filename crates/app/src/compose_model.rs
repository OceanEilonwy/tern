//! What the composer does that needs no GTK: subjects, quoting, recipients and building the draft.

use tern_core::{parse_addresses, text_to_html, Account, Address, Attachment, Draft, Mail, MailId, NewAttachment, ReplyKind};

/// Tuta refuses mails much bigger than this.
pub const MAX_TOTAL_BYTES: u64 = 25 * 1024 * 1024;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Answer {
	Reply,
	ReplyAll,
	Forward,
}

/// The earlier mail a reply or forward carries, kept apart from what is written so it can be left
/// out with a click.
#[derive(Clone, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Quote {
	/// Lines picked in the reader before answering, and whose they are: quoted above the reply.
	pub picked: Option<(String, String)>,
	/// The whole mail answered or forwarded.
	pub whole: Option<Whole>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Whole {
	/// What it is, in a few words: "Sam's message", "Forwarded: Accounts, 26 Oct".
	pub label: String,
	/// The line or lines before it: "On …, Sam <sam@x> wrote:", or a forward's From, Date…
	pub head: String,
	pub text: String,
	/// It goes with the mail; a forward's does from the start, a reply's when asked.
	pub included: bool,
	pub forward: bool,
}

impl Quote {
	/// The HTML sent for it around `body` (already HTML): picked lines above, the whole mail below.
	pub fn around(&self, body: &str) -> String {
		let mut html = String::new();
		if let Some((who, lines)) = &self.picked {
			html.push_str(&format!("<div>{}</div>{}", escape(&format!("{who} wrote:")), blockquote(lines)));
		}
		html.push_str(body);
		if let Some(whole) = self.whole.as_ref().filter(|w| w.included) {
			html.push_str("<br>");
			if whole.forward {
				html.push_str(&format!("{}{}", text_to_html(&whole.head), text_to_html(&whole.text)));
			} else {
				html.push_str(&format!("{}{}", text_to_html(&whole.head), blockquote(&whole.text)));
			}
		}
		html
	}
}

/// The opening of the blockquote [`blockquote`] writes, which finds Tern's own quotes again.
const QUOTE_OPEN: &str = "<blockquote style=\"margin:0 0 0 0.8ex;border-left:1px solid #ccc;padding-left:1ex\">";
const QUOTE_CLOSE: &str = "</blockquote>";

/// Text from the HTML [`text_to_html`] makes of it, or `None` for any other HTML.
fn untext(html: &str) -> Option<String> {
	let inner = html.strip_prefix("<div>")?.strip_suffix("</div>")?;
	if inner.replace("<br>", "").contains('<') {
		return None;
	}
	Some(inner.split("<br>").map(|line| line.replace("&lt;", "<").replace("&gt;", ">").replace("&quot;", "\"").replace("&amp;", "&")).collect::<Vec<_>>().join("\n"))
}

/// Takes `<div>…</div>` made by [`text_to_html`] off the front of `html`: its text and the rest.
fn take_div(html: &str) -> Option<(String, &str)> {
	let end = html.find("</div>")? + "</div>".len();
	Some((untext(&html[..end])?, &html[end..]))
}

/// Takes a blockquote made by [`blockquote`] off the front of `html`: its text and the rest.
fn take_quote(html: &str) -> Option<(String, &str)> {
	let rest = html.strip_prefix(QUOTE_OPEN)?;
	let end = rest.find(QUOTE_CLOSE)?;
	Some((untext(&rest[..end])?, &rest[end + QUOTE_CLOSE.len()..]))
}

/// What a draft Tern wrote is made of: the quotes [`Quote::around`] put around the text, and the
/// text. `None` when its HTML is not Tern's, as for a draft written in Tuta's own app. The whole
/// mail's short label is not in the HTML, so it is a general one.
pub fn split_quotes(html: &str) -> Option<(Quote, String)> {
	let mut rest = html;
	let mut quote = Quote::default();
	if let Some((who, after)) = take_div(rest).and_then(|(line, after)| Some((line.strip_suffix(" wrote:")?.to_string(), after))) {
		if let Some((lines, after)) = take_quote(after) {
			quote.picked = Some((who, lines));
			rest = after;
		}
	}
	let (body, after) = take_div(rest)?;
	if let Some(after) = after.strip_prefix("<br>") {
		let (head, after) = take_div(after)?;
		let forward = head.starts_with("---------- Forwarded message ----------");
		let (text, after) = if forward { take_div(after)? } else { take_quote(after)? };
		if !after.is_empty() {
			return None;
		}
		let label = if forward { "Forwarded Mail" } else { "Earlier Message" };
		quote.whole = Some(Whole { label: label.into(), head, text, included: true, forward });
	} else if !after.is_empty() {
		return None;
	}
	Some((quote, body))
}

fn escape(text: &str) -> String {
	text.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;").replace('"', "&quot;")
}

/// Quoted text as mail apps show it: indented under a line, and folded away by those that fold quotes.
fn blockquote(text: &str) -> String {
	format!("{QUOTE_OPEN}{}{QUOTE_CLOSE}", text_to_html(text))
}

/// What the composer's fields hold.
#[derive(Clone, Debug, Default)]
pub struct Fields {
	pub to: String,
	pub cc: String,
	pub bcc: String,
	pub subject: String,
	pub body: String,
	pub attachments: Vec<NewAttachment>,
	pub previous: Option<(MailId, ReplyKind)>,
	/// The saved draft these fields continue.
	pub existing: Option<MailId>,
	/// Attachments already on `existing`.
	pub kept: Vec<Attachment>,
	/// The address to send from; empty for the account's main one.
	pub from: String,
	pub quote: Quote,
}

/// Which fields hold text that is not an address, and that text.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Invalid {
	pub to: Vec<String>,
	pub cc: Vec<String>,
	pub bcc: Vec<String>,
	pub no_recipients: bool,
}

impl Invalid {
	pub fn message(&self) -> String {
		if self.no_recipients {
			return "Add a recipient".into();
		}
		let bad: Vec<&str> = self.to.iter().chain(&self.cc).chain(&self.bcc).map(String::as_str).collect();
		format!("Not an email address: {}", bad.join(", "))
	}
}

impl Fields {
	pub fn is_empty(&self) -> bool {
		self.to.trim().is_empty()
			&& self.cc.trim().is_empty()
			&& self.bcc.trim().is_empty()
			&& self.subject.trim().is_empty()
			&& self.body.trim().is_empty()
			&& self.attachments.is_empty()
			&& self.kept.is_empty()
	}

	pub fn total_size(&self) -> u64 {
		self.attachments.iter().map(|a| a.data.len() as u64).sum::<u64>() + self.kept.iter().map(|a| a.size).sum::<u64>()
	}

	/// The draft to save: like [`Fields::build_draft`], but recipients may be missing, and text in
	/// the address fields that is not an address is left out.
	pub fn saved_draft(&self) -> Draft {
		let valid = |text: &str| -> Vec<Address> {
			text.split([',', ';']).filter_map(|part| parse_addresses(part).ok()).flatten().collect()
		};
		Draft { to: valid(&self.to), cc: valid(&self.cc), bcc: valid(&self.bcc), ..self.draft_with(Vec::new(), Vec::new(), Vec::new()) }
	}

	fn draft_with(&self, to: Vec<Address>, cc: Vec<Address>, bcc: Vec<Address>) -> Draft {
		Draft {
			to,
			cc,
			bcc,
			subject: self.subject.trim().to_string(),
			body_html: self.quote.around(&text_to_html(&self.body)),
			attachments: self.attachments.clone(),
			previous: self.previous.clone(),
			existing: self.existing.clone(),
			kept: self.kept.clone(),
			from: Some(self.from.trim().to_string()).filter(|f| !f.is_empty()),
		}
	}

	/// The draft to send, or which recipients are wrong.
	pub fn build_draft(&self) -> Result<Draft, Invalid> {
		let mut invalid = Invalid::default();
		let to = parse_addresses(&self.to).unwrap_or_else(|bad| {
			invalid.to = bad;
			Vec::new()
		});
		let cc = parse_addresses(&self.cc).unwrap_or_else(|bad| {
			invalid.cc = bad;
			Vec::new()
		});
		let bcc = parse_addresses(&self.bcc).unwrap_or_else(|bad| {
			invalid.bcc = bad;
			Vec::new()
		});
		if invalid != Invalid::default() {
			return Err(invalid);
		}
		if to.is_empty() && cc.is_empty() && bcc.is_empty() {
			return Err(Invalid { no_recipients: true, ..Default::default() });
		}
		Ok(self.draft_with(to, cc, bcc))
	}
}

/// Adds `prefix` ("Re:" or "Fwd:") unless the subject already starts with it or a common variant.
fn prefixed(subject: &str, prefix: &str, variants: &[&str]) -> String {
	let s = subject.trim();
	let lower = s.to_lowercase();
	if variants.iter().any(|v| lower.starts_with(v)) {
		s.to_string()
	} else if s.is_empty() {
		prefix.trim_end().to_string()
	} else {
		format!("{prefix}{s}")
	}
}

pub fn reply_subject(subject: &str) -> String {
	prefixed(subject, "Re: ", &["re:", "aw:", "sv:", "antw:"])
}

pub fn forward_subject(subject: &str) -> String {
	prefixed(subject, "Fwd: ", &["fwd:", "fw:", "wg:"])
}

/// "Ann Lee <ann@x.org>", or the bare address.
pub fn format_address(a: &Address) -> String {
	if a.name.trim().is_empty() {
		a.address.clone()
	} else {
		format!("{} <{}>", a.name.trim(), a.address)
	}
}

pub fn format_addresses(list: &[Address]) -> String {
	list.iter().map(format_address).collect::<Vec<_>>().join(", ")
}

/// To and Cc for answering `mail`, leaving out `me`'s addresses.
pub fn reply_recipients(mail: &Mail, all: bool, me: &Account) -> (Vec<Address>, Vec<Address>) {
	let mut to: Vec<Address> = if mail.reply_to.is_empty() { vec![mail.summary.from.clone()] } else { mail.reply_to.clone() };
	let mut cc = Vec::new();
	// Answering a mail you sent goes back to its recipients.
	if to.iter().all(|a| me.owns(&a.address)) {
		to = mail.to.clone();
	}
	if all {
		let mut seen: Vec<String> = to.iter().map(|a| a.address.to_lowercase()).collect();
		for a in mail.to.iter().chain(&mail.cc) {
			let key = a.address.to_lowercase();
			if !seen.contains(&key) && !me.owns(&key) {
				seen.push(key);
				cc.push(a.clone());
			}
		}
	}
	(to, cc)
}

/// What answering `mail` (whose readable text is `text`) carries of it: a reply the lines `picked`
/// in it, with the whole mail ready to add; a forward the whole mail.
pub fn answer_quote(mail: &Mail, text: &str, date: &str, answer: Answer, picked: Option<&str>) -> Quote {
	let from = &mail.summary.from;
	let name = if from.name.trim().is_empty() { from.address.clone() } else { from.name.trim().to_string() };
	match answer {
		Answer::Reply | Answer::ReplyAll => Quote {
			picked: picked.map(str::trim).filter(|p| !p.is_empty()).map(|p| (name.clone(), p.to_string())),
			whole: Some(Whole {
				label: format!("{name}’s message"),
				head: format!("On {date}, {} wrote:", format_address(from)),
				text: text.trim_end().to_string(),
				included: false,
				forward: false,
			}),
		},
		Answer::Forward => {
			let mut head = format!(
				"---------- Forwarded message ----------\nFrom: {}\nDate: {date}\nSubject: {}\nTo: {}",
				format_address(from),
				mail.summary.subject,
				format_addresses(&mail.to)
			);
			if !mail.cc.is_empty() {
				head.push_str(&format!("\nCc: {}", format_addresses(&mail.cc)));
			}
			let files = match mail.attachments.len() {
				0 => String::new(),
				1 => " · 1 attachment".into(),
				n => format!(" · {n} attachments"),
			};
			Quote { picked: None, whole: Some(Whole { label: format!("Forwarded: {name}, {date}{files}"), head: format!("{head}\n"), text: text.trim_end().to_string(), included: true, forward: true }) }
		},
	}
}

/// The fields for answering `mail`, from the address of `me` it was sent to. Forwarded attachments
/// are added by the caller as they download.
pub fn answer_fields(mail: &Mail, text: &str, date: &str, answer: Answer, me: &Account, picked: Option<&str>) -> Fields {
	let (to, cc, subject, kind) = match answer {
		Answer::Reply | Answer::ReplyAll => {
			let (to, cc) = reply_recipients(mail, answer == Answer::ReplyAll, me);
			(format_addresses(&to), format_addresses(&cc), reply_subject(&mail.summary.subject), ReplyKind::Reply)
		}
		Answer::Forward => (String::new(), String::new(), forward_subject(&mail.summary.subject), ReplyKind::Forward),
	};
	Fields {
		to,
		cc,
		subject,
		quote: answer_quote(mail, text, date, answer, picked),
		previous: Some((mail.summary.id.clone(), kind)),
		from: mail.to.iter().chain(&mail.cc).find_map(|a| me.addresses().into_iter().find(|mine| mine.eq_ignore_ascii_case(&a.address))).unwrap_or_default(),
		..Default::default()
	}
}

/// The fields of a saved draft, to continue writing it. `text` is its body as plain text.
pub fn draft_fields(draft: &Mail, text: &str, kept_quote: Option<Quote>) -> Fields {
	// Tern's own drafts come apart into the text and its quotes, which show as written: as kept
	// here when they were (a quote left out included), else as read back from the mail. Any other
	// draft is all text.
	let (quote, body) = match split_quotes(&draft.body_html) {
		Some((read, body)) => (kept_quote.unwrap_or(read), body),
		None => (Quote::default(), text.to_string()),
	};
	Fields {
		to: format_addresses(&draft.to),
		cc: format_addresses(&draft.cc),
		bcc: format_addresses(&draft.bcc),
		subject: draft.summary.subject.clone(),
		body,
		quote,
		existing: Some(draft.summary.id.clone()),
		kept: draft.attachments.clone(),
		from: draft.summary.from.address.clone(),
		..Default::default()
	}
}

/// A file name safe to create in a directory of ours: no paths, no hidden files.
pub fn safe_file_name(name: &str) -> String {
	let base = name.rsplit(['/', '\\']).next().unwrap_or("");
	let cleaned: String = base.chars().map(|c| if c.is_control() || c == ':' { '_' } else { c }).collect();
	let cleaned = cleaned.trim().trim_start_matches('.').to_string();
	if cleaned.is_empty() {
		"attachment".into()
	} else {
		cleaned
	}
}

#[cfg(test)]
mod tests {
	use super::*;

	#[test]
	fn a_draft_reopens_with_its_recipients_and_attachments() {
		let mut draft = mail();
		draft.summary.draft = true;
		draft.bcc = vec![Address::new("", "boss@x.org")];
		draft.attachments = vec![tern_core::Attachment { id: "f/1".into(), name: "a.pdf".into(), mime: "application/pdf".into(), size: 3, cid: None }];
		let fields = draft_fields(&draft, "Hello", None);
		assert_eq!(fields.to, "You <you@tuta.com>, sam@x.org");
		assert_eq!(fields.bcc, "boss@x.org");
		assert_eq!(fields.existing, Some(draft.summary.id.clone()));
		assert_eq!(fields.kept.len(), 1);
		assert_eq!(fields.total_size(), 3);
		// Saving it again goes over the same draft and keeps the file.
		let saved = fields.saved_draft();
		assert_eq!(saved.existing, fields.existing);
		assert_eq!(saved.kept, fields.kept);
		assert_eq!(saved.bcc, vec![Address::new("", "boss@x.org")]);
	}

	/// A draft saved from `fields`, as Tuta gives it back.
	fn saved(fields: &Fields) -> Mail {
		let mut draft = mail();
		draft.summary.draft = true;
		draft.body_html = fields.saved_draft().body_html;
		draft
	}

	#[test]
	fn a_reopened_draft_shows_its_quotes_apart_from_the_text() {
		let mut f = answer_fields(&mail(), "Hi <all>\n\nThe photos & more", "today", Answer::Reply, &me(), Some("The photos"));
		f.body = "Lovely!\n\nSee you".into();
		f.quote.whole.as_mut().unwrap().included = true;
		// Kept here: exactly as written.
		let again = draft_fields(&saved(&f), "flattened", Some(f.quote.clone()));
		assert_eq!((again.body.as_str(), &again.quote), ("Lovely!\n\nSee you", &f.quote));
		// Not kept (saved on another computer): read back from the mail, under a general label.
		let again = draft_fields(&saved(&f), "flattened", None);
		assert_eq!(again.body, "Lovely!\n\nSee you");
		assert_eq!(again.quote.picked, f.quote.picked);
		let (read, wrote) = (again.quote.whole.unwrap(), f.quote.whole.clone().unwrap());
		assert_eq!((read.head, read.text, read.included, read.forward, read.label.as_str()), (wrote.head, wrote.text, true, false, "Earlier Message"));

		// A quote left out is not in the mail: only what was kept here brings it back.
		f.quote.whole.as_mut().unwrap().included = false;
		f.quote.picked = None;
		assert!(draft_fields(&saved(&f), "x", None).quote.whole.is_none());
		assert_eq!(draft_fields(&saved(&f), "x", Some(f.quote.clone())).quote, f.quote);
		assert_eq!(draft_fields(&saved(&f), "x", None).body, "Lovely!\n\nSee you");
	}

	#[test]
	fn a_reopened_forward_keeps_the_forwarded_mail_apart() {
		let mut f = answer_fields(&mail(), "Body\nmore", "today", Answer::Forward, &me(), None);
		f.body = "FYI".into();
		let again = draft_fields(&saved(&f), "x", None);
		let (read, wrote) = (again.quote.whole.unwrap(), f.quote.whole.unwrap());
		assert_eq!(again.body, "FYI");
		assert_eq!((read.head, read.text, read.forward, read.label.as_str()), (wrote.head, wrote.text, true, "Forwarded Mail"));
	}

	#[test]
	fn a_draft_from_elsewhere_is_all_text() {
		let mut draft = mail();
		draft.body_html = "<p>Hi</p><blockquote>quoted</blockquote>".into();
		let f = draft_fields(&draft, "Hi\nquoted", Some(Quote { picked: Some(("A".into(), "b".into())), whole: None }));
		assert_eq!((f.body.as_str(), f.quote), ("Hi\nquoted", Quote::default()), "a kept quote is not shown twice over text that holds it");
		draft.body_html = "<div>Typed &lt;div&gt; and <b>bold</b></div>".into();
		assert_eq!(draft_fields(&draft, "plain", None).body, "plain");
	}

	#[test]
	fn a_saved_draft_leaves_out_what_is_not_an_address() {
		let fields = Fields { to: "mia@tuta.com, not yet".into(), subject: "Plan".into(), ..Default::default() };
		assert_eq!(fields.saved_draft().to, vec![Address::new("", "mia@tuta.com")]);
		assert!(fields.build_draft().is_err());
	}
	use tern_core::demo::DemoBackend;
	use tern_core::{Backend, FolderId, MailSummary};

	fn me() -> Account {
		Account { address: "you@tuta.com".into(), name: "You".into(), aliases: vec!["shop@you.example".into()] }
	}

	fn mail() -> Mail {
		Mail {
			summary: MailSummary {
				id: MailId { list: "l".into(), element: "e".into() },
				from: Address::new("Mia Chen", "mia@tuta.com"),
				subject: "Photos".into(),
				received_ms: 0,
				unread: false,
				has_attachments: false,
				to: None,
				outgoing: false,
				draft: false,
			},
			to: vec![Address::new("You", "you@tuta.com"), Address::new("", "sam@x.org")],
			cc: vec![Address::new("Ann", "ann@x.org"), Address::new("", "MIA@tuta.com")],
			bcc: vec![],
			reply_to: vec![],
			body_html: String::new(),
			attachments: vec![],
			web_url: String::new(),
		}
	}

	#[test]
	fn subjects_are_prefixed_once() {
		assert_eq!(reply_subject("Lunch"), "Re: Lunch");
		assert_eq!(reply_subject("Re: Lunch"), "Re: Lunch");
		assert_eq!(reply_subject("RE: Lunch"), "RE: Lunch");
		assert_eq!(reply_subject("AW: Mittag"), "AW: Mittag");
		assert_eq!(reply_subject(""), "Re:");
		assert_eq!(forward_subject("Lunch"), "Fwd: Lunch");
		assert_eq!(forward_subject("Fw: Lunch"), "Fw: Lunch");
		assert_eq!(forward_subject("Re: Lunch"), "Fwd: Re: Lunch");
	}

	#[test]
	fn reply_all_leaves_out_me_and_duplicates() {
		let m = mail();
		let (to, cc) = reply_recipients(&m, false, &me());
		assert_eq!(to, vec![m.summary.from.clone()]);
		assert!(cc.is_empty());
		let (to, cc) = reply_recipients(&m, true, &me());
		assert_eq!(to.len(), 1);
		assert_eq!(cc, vec![Address::new("", "sam@x.org"), Address::new("Ann", "ann@x.org")]);
	}

	#[test]
	fn reply_to_wins_and_own_mail_goes_back_to_recipients() {
		let mut m = mail();
		m.reply_to = vec![Address::new("", "list@x.org")];
		assert_eq!(reply_recipients(&m, false, &me()).0, vec![Address::new("", "list@x.org")]);
		let mut sent = mail();
		sent.summary.from = Address::new("You", "you@tuta.com");
		assert_eq!(reply_recipients(&sent, false, &me()).0, sent.to);
		// Sent from an alias too.
		sent.summary.from = Address::new("You", "SHOP@you.example");
		assert_eq!(reply_recipients(&sent, false, &me()).0, sent.to);
	}

	#[test]
	fn a_reply_quotes_only_what_was_picked_until_the_whole_mail_is_added() {
		let mut f = answer_fields(&mail(), "Hi\n\nThe photos\n", "3 Oct 2026, 14:05", Answer::Reply, &me(), None);
		assert_eq!(f.to, "Mia Chen <mia@tuta.com>");
		assert_eq!(f.subject, "Re: Photos");
		assert_eq!(f.body, "", "the reply starts empty");
		assert_eq!(f.previous, Some((mail().summary.id, ReplyKind::Reply)));
		f.body = "Lovely!".into();
		assert_eq!(f.build_draft().unwrap().body_html, "<div>Lovely!</div>", "nothing quoted");
		let whole = f.quote.whole.as_mut().unwrap();
		assert_eq!((whole.label.as_str(), whole.head.as_str()), ("Mia Chen’s message", "On 3 Oct 2026, 14:05, Mia Chen <mia@tuta.com> wrote:"));
		whole.included = true;
		let html = f.build_draft().unwrap().body_html;
		assert!(html.starts_with("<div>Lovely!</div><br><div>On 3 Oct 2026, 14:05, Mia Chen &lt;mia@tuta.com&gt; wrote:</div><blockquote"), "{html}");
		assert!(html.ends_with(">Hi<br><br>The photos</div></blockquote>"), "{html}");

		// Lines picked in the reader go above the reply.
		let mut f = answer_fields(&mail(), "Hi\n\nThe photos", "today", Answer::ReplyAll, &me(), Some("  The photos \n"));
		assert_eq!(f.quote.picked, Some(("Mia Chen".to_string(), "The photos".to_string())));
		f.body = "Thanks".into();
		assert!(f.build_draft().unwrap().body_html.starts_with("<div>Mia Chen wrote:</div><blockquote"));
		assert!(f.build_draft().unwrap().body_html.ends_with("The photos</div></blockquote><div>Thanks</div>"));
		assert!(answer_fields(&mail(), "x", "today", Answer::Reply, &me(), Some("  ")).quote.picked.is_none(), "a blank pick quotes nothing");
	}

	#[test]
	fn an_answer_goes_out_from_the_address_the_mail_came_to() {
		let mut m = mail();
		assert_eq!(answer_fields(&m, "", "", Answer::Reply, &me(), None).from, "you@tuta.com");
		m.to = vec![Address::new("", "Shop@You.example")];
		m.cc = vec![Address::new("", "ann@x.org")];
		let f = answer_fields(&m, "", "", Answer::ReplyAll, &me(), None);
		assert_eq!(f.from, "shop@you.example", "as the account spells it");
		assert_eq!(f.cc, "ann@x.org", "the alias is not copied in");
		assert_eq!(f.build_draft().unwrap().from.as_deref(), Some("shop@you.example"));
		// Sent to none of them (a list, Bcc): the main address.
		m.to = vec![Address::new("", "list@x.org")];
		assert_eq!(answer_fields(&m, "", "", Answer::Reply, &me(), None).from, "");
		assert_eq!(answer_fields(&m, "", "", Answer::Reply, &me(), None).build_draft().unwrap().from, None);
	}

	#[test]
	fn forward_has_a_header_and_no_recipients() {
		let mut f = answer_fields(&mail(), "Body", "today", Answer::Forward, &me(), None);
		assert!(f.to.is_empty());
		assert_eq!(f.subject, "Fwd: Photos");
		assert_eq!(f.previous.clone().unwrap().1, ReplyKind::Forward);
		let whole = f.quote.whole.clone().unwrap();
		assert!(whole.included && whole.forward, "a forward carries the mail from the start");
		assert_eq!(whole.label, "Forwarded: Mia Chen, today");
		assert!(whole.head.starts_with("---------- Forwarded message ----------\nFrom: Mia Chen <mia@tuta.com>\n"));
		assert!(whole.head.ends_with("Cc: Ann <ann@x.org>, MIA@tuta.com\n"));
		f.to = "sam@x.org".into();
		let html = f.build_draft().unwrap().body_html;
		assert!(html.starts_with("<div></div><br><div>---------- Forwarded message ----------<br>From: Mia Chen &lt;mia@tuta.com&gt;<br>") && html.ends_with("<div>Body</div>"), "{html}");
		f.quote.whole.as_mut().unwrap().included = false;
		assert_eq!(f.build_draft().unwrap().body_html, "<div></div>", "left out with ×");
	}

	#[test]
	fn invalid_recipients_are_named_per_field() {
		let f = Fields { to: "ok@x.org, nope".into(), bcc: "also bad".into(), ..Default::default() };
		let err = f.build_draft().unwrap_err();
		assert_eq!(err.to, vec!["nope".to_string()]);
		assert_eq!(err.bcc, vec!["also bad".to_string()]);
		assert_eq!(err.message(), "Not an email address: nope, also bad");
		let none = Fields { subject: "x".into(), ..Default::default() }.build_draft().unwrap_err();
		assert!(none.no_recipients);
	}

	#[test]
	fn names_are_made_safe() {
		assert_eq!(safe_file_name("../../etc/passwd"), "passwd");
		assert_eq!(safe_file_name(".bashrc"), "bashrc");
		assert_eq!(safe_file_name("a\\b\nc.pdf"), "b_c.pdf");
		assert_eq!(safe_file_name(""), "attachment");
	}

	#[tokio::test]
	async fn composed_mail_with_attachments_reaches_the_demo_sent_folder() {
		let demo = DemoBackend::new();
		let pdf = b"%PDF-1.4 report".to_vec();
		let png = tern_core::demo::TINY_PNG.to_vec();
		let fields = Fields {
			to: "\"Mia Chen\" <mia@tuta.com>".into(),
			subject: "  Report ".into(),
			body: "Hi Mia,\nsee <attached>.".into(),
			attachments: vec![
				NewAttachment { name: "r.pdf".into(), mime: tern_core::mime_for("r.pdf").into(), data: pdf.clone() },
				NewAttachment { name: "p.png".into(), mime: tern_core::mime_for("p.png").into(), data: png.clone() },
			],
			..Default::default()
		};
		assert!(!fields.is_empty());
		assert_eq!(fields.total_size(), (pdf.len() + png.len()) as u64);
		demo.send(fields.build_draft().unwrap()).await.unwrap();

		let sent = demo.list(&FolderId("sent".into()), None, 50).await.unwrap();
		let mail = demo.open(&sent.mails[0].id).await.unwrap();
		assert_eq!(mail.summary.subject, "Report");
		assert_eq!(mail.to, vec![Address::new("Mia Chen", "mia@tuta.com")]);
		assert_eq!(mail.body_html, "<div>Hi Mia,<br>see &lt;attached&gt;.</div>");
		assert_eq!(mail.attachments.len(), 2);
		assert_eq!(mail.attachments[1].mime, "image/png");
		assert_eq!(demo.attachment(&mail.summary.id, &mail.attachments[0]).await.unwrap(), pdf);
		// What the reader shows of it is what was typed.
		assert_eq!(crate::html_doc::Doc::parse(&mail.body_html).to_plain_text(), "Hi Mia,\nsee <attached>.");
	}
}

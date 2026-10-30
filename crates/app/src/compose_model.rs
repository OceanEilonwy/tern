//! What the composer does that needs no GTK: subjects, quoting, recipients and building the draft.

use tern_core::{parse_addresses, text_to_html, Address, Draft, Mail, MailId, NewAttachment, ReplyKind};

/// Tuta refuses mails much bigger than this.
pub const MAX_TOTAL_BYTES: u64 = 25 * 1024 * 1024;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Answer {
	Reply,
	ReplyAll,
	Forward,
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
	}

	pub fn total_size(&self) -> u64 {
		self.attachments.iter().map(|a| a.data.len() as u64).sum()
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
		Ok(Draft {
			to,
			cc,
			bcc,
			subject: self.subject.trim().to_string(),
			body_html: text_to_html(&self.body),
			attachments: self.attachments.clone(),
			previous: self.previous.clone(),
		})
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

/// To and Cc for answering `mail`, leaving out `me`.
pub fn reply_recipients(mail: &Mail, all: bool, me: &str) -> (Vec<Address>, Vec<Address>) {
	let mut to: Vec<Address> = if mail.reply_to.is_empty() { vec![mail.summary.from.clone()] } else { mail.reply_to.clone() };
	let mut cc = Vec::new();
	// Answering a mail you sent goes back to its recipients.
	if to.iter().all(|a| a.address.eq_ignore_ascii_case(me)) {
		to = mail.to.clone();
	}
	if all {
		let mut seen: Vec<String> = to.iter().map(|a| a.address.to_lowercase()).collect();
		seen.push(me.to_lowercase());
		for a in mail.to.iter().chain(&mail.cc) {
			let key = a.address.to_lowercase();
			if !seen.contains(&key) {
				seen.push(key);
				cc.push(a.clone());
			}
		}
	}
	(to, cc)
}

fn quote_lines(text: &str) -> String {
	text.lines().map(|l| if l.is_empty() { ">".to_string() } else { format!("> {l}") }).collect::<Vec<_>>().join("\n")
}

/// The composer's starting text for answering a mail whose readable text is `text`.
pub fn answer_body(mail: &Mail, text: &str, date: &str, answer: Answer) -> String {
	let from = &mail.summary.from;
	match answer {
		Answer::Reply | Answer::ReplyAll => {
			format!("\n\nOn {date}, {} wrote:\n{}\n", format_address(from), quote_lines(text))
		}
		Answer::Forward => {
			let mut head = format!(
				"\n\n---------- Forwarded message ----------\nFrom: {}\nDate: {date}\nSubject: {}\nTo: {}\n",
				format_address(from),
				mail.summary.subject,
				format_addresses(&mail.to)
			);
			if !mail.cc.is_empty() {
				head.push_str(&format!("Cc: {}\n", format_addresses(&mail.cc)));
			}
			format!("{head}\n{text}\n")
		}
	}
}

/// The fields for answering `mail`. Forwarded attachments are added by the caller as they download.
pub fn answer_fields(mail: &Mail, text: &str, date: &str, answer: Answer, me: &str) -> Fields {
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
		body: answer_body(mail, text, date, answer),
		previous: Some((mail.summary.id.clone(), kind)),
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
	use tern_core::demo::DemoBackend;
	use tern_core::{Backend, FolderId, MailSummary};

	fn mail() -> Mail {
		Mail {
			summary: MailSummary {
				id: MailId { list: "l".into(), element: "e".into() },
				from: Address::new("Mia Chen", "mia@tuta.com"),
				subject: "Photos".into(),
				received_ms: 0,
				unread: false,
				has_attachments: false,
			},
			to: vec![Address::new("You", "you@tuta.com"), Address::new("", "sam@x.org")],
			cc: vec![Address::new("Ann", "ann@x.org"), Address::new("", "MIA@tuta.com")],
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
		let (to, cc) = reply_recipients(&m, false, "you@tuta.com");
		assert_eq!(to, vec![m.summary.from.clone()]);
		assert!(cc.is_empty());
		let (to, cc) = reply_recipients(&m, true, "YOU@tuta.com");
		assert_eq!(to.len(), 1);
		assert_eq!(cc, vec![Address::new("", "sam@x.org"), Address::new("Ann", "ann@x.org")]);
	}

	#[test]
	fn reply_to_wins_and_own_mail_goes_back_to_recipients() {
		let mut m = mail();
		m.reply_to = vec![Address::new("", "list@x.org")];
		assert_eq!(reply_recipients(&m, false, "you@tuta.com").0, vec![Address::new("", "list@x.org")]);
		let mut sent = mail();
		sent.summary.from = Address::new("You", "you@tuta.com");
		assert_eq!(reply_recipients(&sent, false, "you@tuta.com").0, sent.to);
	}

	#[test]
	fn reply_quotes_the_original() {
		let f = answer_fields(&mail(), "Hi\n\nThe photos", "3 Oct 2026, 14:05", Answer::Reply, "you@tuta.com");
		assert_eq!(f.to, "Mia Chen <mia@tuta.com>");
		assert_eq!(f.subject, "Re: Photos");
		assert_eq!(f.body, "\n\nOn 3 Oct 2026, 14:05, Mia Chen <mia@tuta.com> wrote:\n> Hi\n>\n> The photos\n");
		assert_eq!(f.previous, Some((mail().summary.id, ReplyKind::Reply)));
		// The recipients the reply prefills parse back.
		assert!(f.build_draft().is_ok());
	}

	#[test]
	fn forward_has_a_header_and_no_recipients() {
		let f = answer_fields(&mail(), "Body", "today", Answer::Forward, "you@tuta.com");
		assert!(f.to.is_empty());
		assert_eq!(f.subject, "Fwd: Photos");
		assert!(f.body.contains("---------- Forwarded message ----------\nFrom: Mia Chen <mia@tuta.com>\n"));
		assert!(f.body.contains("Cc: Ann <ann@x.org>, MIA@tuta.com\n\nBody\n"));
		assert_eq!(f.previous.unwrap().1, ReplyKind::Forward);
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

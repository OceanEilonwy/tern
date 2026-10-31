//! Mail rules: a line like `(from:@deals.example or subject:"50% off") and not to:me@x.org` says
//! which mail a rule catches, and the rule moves it to a folder, labels it, or sends it to Spam.
//!
//! A term is a word or a quoted phrase, matched anywhere in the subject or the sender, ignoring
//! case. `from:` (or `sender:`) matches the sender's name or address, `to:` the first recipient,
//! `subject:` (or `title:`) the subject. A value starting with `@` matches an address's domain and
//! its subdomains. Terms side by side must all match; `or` between them is either, `not` (or `-`)
//! before one is its opposite, and brackets group.

use serde::{Deserialize, Serialize};

use crate::{Address, MailSummary};

/// What a rule does with the mail it catches. Folders and labels are named as Tern shows them.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Action {
	Spam,
	Move(String),
	Label(String),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Rule {
	pub when: String,
	pub action: Action,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Field {
	From,
	To,
	Subject,
	/// The subject or the sender.
	Any,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Expr {
	Term(Field, String),
	Not(Box<Expr>),
	And(Vec<Expr>),
	Or(Vec<Expr>),
}

/// Why a rule's line does not read, and where (a char index into it).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ParseError {
	pub message: String,
	pub at: usize,
}

impl std::fmt::Display for ParseError {
	fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
		f.write_str(&self.message)
	}
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum Token {
	Open,
	Close,
	Not,
	And,
	Or,
	Term(Field, String),
}

fn field(name: &str) -> Option<Field> {
	match name.to_lowercase().as_str() {
		"from" | "sender" => Some(Field::From),
		"to" => Some(Field::To),
		"subject" | "title" => Some(Field::Subject),
		_ => None,
	}
}

fn tokens(line: &str) -> Result<Vec<(usize, Token)>, ParseError> {
	let chars: Vec<char> = line.chars().collect();
	let mut out = Vec::new();
	let mut i = 0;
	let err = |message: String, at: usize| Err(ParseError { message, at });
	while i < chars.len() {
		let c = chars[i];
		if c.is_whitespace() {
			i += 1;
			continue;
		}
		let start = i;
		match c {
			'(' => {
				out.push((start, Token::Open));
				i += 1;
				continue;
			},
			')' => {
				out.push((start, Token::Close));
				i += 1;
				continue;
			},
			'-' => {
				out.push((start, Token::Not));
				i += 1;
				continue;
			},
			_ => {},
		}
		// A word, maybe `field:` before a word or a quoted phrase.
		let mut word = String::new();
		while i < chars.len() && !chars[i].is_whitespace() && !matches!(chars[i], '(' | ')' | '"' | ':') {
			word.push(chars[i]);
			i += 1;
		}
		let mut fixed = Field::Any;
		if i < chars.len() && chars[i] == ':' {
			let Some(f) = field(&word) else {
				return err(format!("“{word}:” is not something a rule can look at; use from:, to: or subject:"), start);
			};
			fixed = f;
			i += 1;
			word.clear();
			while i < chars.len() && !chars[i].is_whitespace() && !matches!(chars[i], '(' | ')' | '"') {
				word.push(chars[i]);
				i += 1;
			}
		}
		let quoted = i < chars.len() && chars[i] == '"' && word.is_empty();
		if quoted {
			let open = i;
			i += 1;
			while i < chars.len() && chars[i] != '"' {
				word.push(chars[i]);
				i += 1;
			}
			if i == chars.len() {
				return err("A quote is not closed".into(), open);
			}
			i += 1;
		}
		if word.trim().is_empty() {
			return err("Something to look for is missing here".into(), start);
		}
		let token = match (fixed, quoted, word.to_lowercase().as_str()) {
			(Field::Any, false, "and") => Token::And,
			(Field::Any, false, "or") => Token::Or,
			(Field::Any, false, "not") => Token::Not,
			_ => Token::Term(fixed, word.trim().to_lowercase()),
		};
		out.push((start, token));
	}
	Ok(out)
}

struct Parser {
	tokens: Vec<(usize, Token)>,
	at: usize,
	end: usize,
}

impl Parser {
	fn peek(&self) -> Option<&Token> {
		self.tokens.get(self.at).map(|(_, t)| t)
	}

	fn here(&self) -> usize {
		self.tokens.get(self.at).map_or(self.end, |(i, _)| *i)
	}

	fn or(&mut self) -> Result<Expr, ParseError> {
		let mut parts = vec![self.and()?];
		while self.peek() == Some(&Token::Or) {
			self.at += 1;
			parts.push(self.and()?);
		}
		Ok(if parts.len() == 1 { parts.pop().unwrap() } else { Expr::Or(parts) })
	}

	fn and(&mut self) -> Result<Expr, ParseError> {
		let mut parts = vec![self.unary()?];
		loop {
			match self.peek() {
				Some(Token::And) => self.at += 1,
				Some(Token::Open | Token::Not | Token::Term(..)) => {},
				_ => break,
			}
			parts.push(self.unary()?);
		}
		Ok(if parts.len() == 1 { parts.pop().unwrap() } else { Expr::And(parts) })
	}

	fn unary(&mut self) -> Result<Expr, ParseError> {
		let at = self.here();
		match self.tokens.get(self.at).map(|(_, t)| t.clone()) {
			Some(Token::Not) => {
				self.at += 1;
				Ok(Expr::Not(Box::new(self.unary()?)))
			},
			Some(Token::Open) => {
				self.at += 1;
				let inner = self.or()?;
				if self.peek() != Some(&Token::Close) {
					return Err(ParseError { message: "A bracket is not closed".into(), at });
				}
				self.at += 1;
				Ok(inner)
			},
			Some(Token::Term(f, v)) => {
				self.at += 1;
				Ok(Expr::Term(f, v))
			},
			Some(Token::Close) => Err(ParseError { message: "This bracket closes nothing".into(), at }),
			Some(Token::And | Token::Or) => Err(ParseError { message: "Something to look for is missing before this".into(), at }),
			None => Err(ParseError { message: "Something to look for is missing at the end".into(), at }),
		}
	}
}

/// Reads a rule's line.
pub fn parse(line: &str) -> Result<Expr, ParseError> {
	let end = line.chars().count();
	let tokens = tokens(line)?;
	if tokens.is_empty() {
		return Err(ParseError { message: "Say which mail the rule is for".into(), at: 0 });
	}
	let mut parser = Parser { tokens, at: 0, end };
	let expr = parser.or()?;
	match parser.peek() {
		None => Ok(expr),
		Some(Token::Close) => Err(ParseError { message: "This bracket closes nothing".into(), at: parser.here() }),
		Some(_) => Err(ParseError { message: "This does not follow on".into(), at: parser.here() }),
	}
}

fn address_matches(a: &Address, value: &str) -> bool {
	let address = a.address.to_lowercase();
	match value.strip_prefix('@') {
		Some(domain) => address.rsplit_once('@').is_some_and(|(_, d)| d == domain || d.ends_with(&format!(".{domain}"))),
		None => address.contains(value) || a.name.to_lowercase().contains(value),
	}
}

impl Expr {
	pub fn matches(&self, mail: &MailSummary) -> bool {
		match self {
			Expr::Term(field, value) => {
				// Quotes cannot be written inside a phrase, so they are not looked at.
				let subject = || mail.subject.to_lowercase().replace('"', "").contains(value.as_str());
				match field {
					Field::From => address_matches(&mail.from, value),
					Field::To => mail.to.as_ref().is_some_and(|a| address_matches(a, value)),
					Field::Subject => subject(),
					Field::Any => subject() || address_matches(&mail.from, value),
				}
			},
			Expr::Not(e) => !e.matches(mail),
			Expr::And(all) => all.iter().all(|e| e.matches(mail)),
			Expr::Or(any) => any.iter().any(|e| e.matches(mail)),
		}
	}
}

/// A term for a rule's line that matches `value` in `field` as it is: quoted when it has spaces
/// or brackets. Quotes inside it cannot be written, so they are left out; matching skips them too.
pub fn term(field: &str, value: &str) -> String {
	let value: String = value.chars().filter(|&c| c != '"').collect();
	let value = value.trim();
	if value.chars().any(|c| c.is_whitespace() || matches!(c, '(' | ')' | ':')) || value.starts_with('-') {
		format!("{field}:\"{value}\"")
	} else {
		format!("{field}:{value}")
	}
}

#[cfg(test)]
mod tests {
	use super::*;
	use crate::MailId;

	fn mail(from: (&str, &str), to: &str, subject: &str) -> MailSummary {
		MailSummary {
			id: MailId { list: "l".into(), element: "e".into() },
			from: Address::new(from.0, from.1),
			subject: subject.into(),
			received_ms: 0,
			unread: false,
			has_attachments: false,
			to: Some(Address::new("", to)),
			outgoing: false,
			draft: false,
		}
	}

	fn catches(line: &str, m: &MailSummary) -> bool {
		parse(line).unwrap_or_else(|e| panic!("{line}: {e}")).matches(m)
	}

	#[test]
	fn fields_domains_and_words_match_ignoring_case() {
		let m = mail(("Mega Deals", "offers@mail.deals.example"), "me@tuta.com", "50% OFF everything");
		assert!(catches("from:@deals.example", &m), "a subdomain is in its domain");
		assert!(catches("from:@example", &m), "and in its parent");
		assert!(!catches("from:@deals.exam", &m), "a domain is matched whole");
		assert!(!catches("from:@als.example", &m), "nor its tail");
		assert!(catches("sender:mega", &m), "the sender's name");
		assert!(catches("title:\"50% off\"", &m));
		assert!(catches("subject:off", &m));
		assert!(catches("everything", &m), "a bare word looks in the subject");
		assert!(catches("deals", &m), "and the sender");
		assert!(catches("to:@tuta.com", &m));
		assert!(!catches("to:@deals.example", &m));
	}

	#[test]
	fn and_or_not_and_brackets_combine() {
		let m = mail(("", "news@foo.com"), "me@tuta.com", "Weekly digest");
		assert!(catches("(sender:@foo.com or title:\"foo\") and digest", &m));
		assert!(catches("from:@foo.com weekly", &m), "side by side means and");
		assert!(!catches("from:@foo.com and monthly", &m));
		assert!(catches("from:@bar.com or weekly", &m));
		assert!(!catches("from:@foo.com not weekly", &m));
		assert!(!catches("from:@foo.com -weekly", &m));
		assert!(catches("NOT (from:@bar.com OR monthly)", &m), "operators in any case");
		assert!(catches("a or b and weekly or from:@foo.com", &m), "and binds tighter than or");
		assert!(!catches("weekly and (a or b)", &m));
	}

	#[test]
	fn broken_lines_say_what_and_where() {
		let at = |line: &str| parse(line).map(|_| ()).map_err(|e| e.at);
		assert_eq!(at(""), Err(0));
		assert_eq!(at("(from:a"), Err(0), "unclosed bracket");
		assert_eq!(at("from:a)"), Err(6), "stray bracket");
		assert_eq!(at("subject:\"half"), Err(8), "unclosed quote");
		assert_eq!(at("date:today"), Err(0), "unknown field");
		assert_eq!(at("a or"), Err(4), "or at the end");
		assert_eq!(at("and a"), Err(0), "and at the start");
		assert_eq!(at("from:"), Err(0), "nothing after the field");
		assert!(parse("from:a b").is_ok());
	}

	#[test]
	fn terms_quote_what_needs_it() {
		assert_eq!(term("from", "offers@deals.example"), "from:offers@deals.example");
		assert_eq!(term("subject", "Last \"chance\": 50% off"), "subject:\"Last chance: 50% off\"");
		let m = mail(("", "a@b.c"), "", "Last \"chance\": 50% off (today)");
		assert!(catches(&term("subject", &m.subject), &m), "a subject's own term catches it, quotes and all");
		assert!(catches("\"and\"", &mail(("", "a@b.c"), "", "This and that")), "a quoted and is a word");
	}
}

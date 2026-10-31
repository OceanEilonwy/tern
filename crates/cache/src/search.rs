//! What search needs besides SQL: reading what was typed, the plain text of a mail's HTML, and
//! fusing the keyword and meaning rankings into one list of conversations.

use std::collections::HashMap;

/// What was typed into search: `from:`, `to:`, `subject:`, `has:attachment`, `in:<folder>` and
/// plain words; "double quotes" keep words together.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Query {
	/// Plain words and quoted phrases, matched anywhere in a mail.
	pub words: Vec<String>,
	pub from: Vec<String>,
	pub to: Vec<String>,
	pub subject: Vec<String>,
	pub attachment: bool,
	/// A folder by name or kind ("inbox", "trash"); without one, all but Trash and Spam.
	pub folder: Option<String>,
}

impl Query {
	pub fn parse(input: &str) -> Query {
		let mut q = Query::default();
		for term in terms(input) {
			let filter = term.split_once(':').filter(|(key, _)| !key.starts_with('"')).map(|(key, value)| (key.to_lowercase(), unquote(value)));
			match filter {
				Some((key, value)) if value.is_empty() && ["from", "to", "subject", "has", "in"].contains(&key.as_str()) => {},
				Some((key, value)) if key == "from" => q.from.push(value),
				Some((key, value)) if key == "to" => q.to.push(value),
				Some((key, value)) if key == "subject" => q.subject.push(value),
				Some((key, value)) if key == "in" => q.folder = Some(value.to_lowercase()),
				Some((key, value)) if key == "has" && value.to_lowercase().starts_with("attachment") => q.attachment = true,
				_ => q.words.push(unquote(&term)),
			}
		}
		q
	}

	/// Nothing to search for.
	pub fn is_empty(&self) -> bool {
		*self == Query::default()
	}

	/// The plain words, for search by meaning.
	pub fn text(&self) -> String {
		self.words.join(" ")
	}

	/// The FTS5 expression for the column filters, and the plain words too with `words`; `None`
	/// when there is nothing to match. Every term is quoted, so nothing typed is FTS5 syntax, and
	/// matches as a prefix ("inv" finds "invoice").
	pub fn fts(&self, words: bool) -> Option<String> {
		self.expression(words.then_some(&[]))
	}

	/// As `fts(true)`, with each plain word also matching the indexed word `fixes` gives for it, in
	/// order (none for most): "rutby" finds "rugby" too.
	pub fn fts_fixed(&self, fixes: &[Option<String>]) -> Option<String> {
		self.expression(Some(fixes))
	}

	fn expression(&self, words: Option<&[Option<String>]>) -> Option<String> {
		let mut parts = Vec::new();
		let columns = [("subject", &self.subject), ("sender", &self.from), ("recipients", &self.to)];
		for (column, terms) in columns {
			parts.extend(terms.iter().filter_map(|t| phrase(t)).map(|p| format!("{column} : {p}")));
		}
		if let Some(fixes) = words {
			for (i, word) in self.words.iter().enumerate() {
				let Some(typed) = phrase(word) else { continue };
				parts.push(match fixes.get(i).and_then(Option::as_ref) {
					Some(fix) => format!("({typed} OR \"{}\")", fix.replace('"', "\"\"")),
					None => typed,
				});
			}
		}
		(!parts.is_empty()).then(|| parts.join(" AND "))
	}
}

/// How many typos a word of `len` letters may hold and still find what was meant: none in short
/// words, where one letter off is usually another word.
pub fn typos_allowed(len: usize) -> usize {
	match len {
		0..=3 => 0,
		4..=7 => 1,
		_ => 2,
	}
}

/// The typos between `a` and `b` (a letter wrong, missing, extra, or two swapped), if at most `max`.
pub fn typos(a: &str, b: &str, max: usize) -> Option<usize> {
	let (a, b): (Vec<char>, Vec<char>) = (a.chars().collect(), b.chars().collect());
	if a.len().abs_diff(b.len()) > max {
		return None;
	}
	// Edit distance, counting a swap of neighbours as one (optimal string alignment).
	let mut rows = vec![(0..=b.len()).collect::<Vec<_>>(); 3];
	for i in 1..=a.len() {
		let (two_back, before) = (rows[0].clone(), rows[1].clone());
		let row = &mut rows[2];
		row[0] = i;
		for j in 1..=b.len() {
			let cost = usize::from(a[i - 1] != b[j - 1]);
			row[j] = (before[j] + 1).min(row[j - 1] + 1).min(before[j - 1] + cost);
			if i > 1 && j > 1 && a[i - 1] == b[j - 2] && a[i - 2] == b[j - 1] {
				row[j] = row[j].min(two_back[j - 2] + 1);
			}
		}
		if rows[2].iter().min().is_some_and(|&least| least > max) {
			return None;
		}
		rows.rotate_left(1);
	}
	Some(rows[1][b.len()]).filter(|&d| d <= max)
}

/// A term as an FTS5 prefix phrase, or `None` when it has no letters or digits to match.
fn phrase(term: &str) -> Option<String> {
	term.chars().any(char::is_alphanumeric).then(|| format!("\"{}\"*", term.replace('"', "\"\"")))
}

/// Splits at spaces outside double quotes; the quotes stay on the term.
fn terms(input: &str) -> Vec<String> {
	let mut terms = Vec::new();
	let (mut term, mut quoted) = (String::new(), false);
	for c in input.chars() {
		if c == '"' {
			quoted = !quoted;
		}
		if c.is_whitespace() && !quoted {
			if !term.is_empty() {
				terms.push(std::mem::take(&mut term));
			}
		} else {
			term.push(c);
		}
	}
	if !term.is_empty() {
		terms.push(term);
	}
	terms
}

fn unquote(term: &str) -> String {
	term.replace('"', "").trim().to_string()
}

/// The text of a mail's HTML body for the search index: tags, styles and scripts gone, common
/// entities decoded, spaces collapsed.
pub fn plain_text(html: &str) -> String {
	let mut out = String::with_capacity(html.len() / 2);
	let mut rest = html;
	while let Some(open) = rest.find('<') {
		push_text(&mut out, &rest[..open]);
		rest = &rest[open..];
		let Some(close) = rest.find('>') else { break };
		let inside = &rest[1..close];
		let tag = inside.split(|c: char| c.is_whitespace() || c == '/').next().unwrap_or("").to_lowercase();
		rest = &rest[close + 1..];
		// What is inside these is not text anyone reads.
		if ["style", "script", "head", "title"].contains(&tag.as_str()) && !inside.ends_with('/') {
			let lower = rest.to_ascii_lowercase();
			rest = match lower.find(&format!("</{tag}")) {
				Some(end) => &rest[end..],
				None => "",
			};
		}
		out.push(' ');
	}
	if !rest.starts_with('<') {
		push_text(&mut out, rest);
	}
	out.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn push_text(out: &mut String, text: &str) {
	let mut rest = text;
	while let Some(amp) = rest.find('&') {
		out.push_str(&rest[..amp]);
		rest = &rest[amp..];
		let entity = rest[1..].find(';').filter(|&end| end <= 10).map(|end| (&rest[1..end + 1], end + 2));
		let decoded = entity.and_then(|(name, len)| {
			let c = match name {
				"amp" => '&',
				"lt" => '<',
				"gt" => '>',
				"quot" => '"',
				"apos" => '\'',
				"nbsp" => ' ',
				_ => {
					let n = name.strip_prefix("#x").or(name.strip_prefix("#X")).map(|h| u32::from_str_radix(h, 16)).or(name.strip_prefix('#').map(str::parse));
					char::from_u32(n?.ok()?)?
				},
			};
			Some((c, len))
		});
		match decoded {
			Some((c, len)) => {
				out.push(c);
				rest = &rest[len..];
			},
			None => {
				out.push('&');
				rest = &rest[1..];
			},
		}
	}
	out.push_str(rest);
}

/// The damping of reciprocal rank fusion: rank 1 scores 1/61, rank 10 1/70, so a mail high in
/// either list counts and one in both counts most. 60 is the value from the original paper.
pub const RRF_K: f64 = 60.0;

/// A mail found by one way of searching.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Found {
	pub mail: i64,
	pub thread: String,
	pub received: i64,
}

/// Fuses rankings (each best first) into conversations, best first: a mail scores 1/(K + rank) in
/// each list it is in, a conversation takes its best mail's score, and the newer conversation wins
/// a tie. Returns each conversation's key with its best mail.
pub fn fuse(lists: &[&[Found]]) -> Vec<(String, i64)> {
	let mut mails: HashMap<i64, (f64, &Found)> = HashMap::new();
	for list in lists {
		for (rank, found) in list.iter().enumerate() {
			mails.entry(found.mail).or_insert((0.0, found)).0 += 1.0 / (RRF_K + rank as f64 + 1.0);
		}
	}
	// Per conversation: the best score, its mail, and the newest mail found.
	let mut threads: HashMap<&str, (f64, i64, i64)> = HashMap::new();
	for (score, found) in mails.values() {
		let t = threads.entry(&found.thread).or_insert((*score, found.mail, found.received));
		if *score > t.0 || (*score == t.0 && found.mail < t.1) {
			(t.0, t.1) = (*score, found.mail);
		}
		t.2 = t.2.max(found.received);
	}
	let mut ranked: Vec<(&str, (f64, i64, i64))> = threads.into_iter().collect();
	ranked.sort_by(|a, b| b.1 .0.total_cmp(&a.1 .0).then(b.1 .2.cmp(&a.1 .2)).then(a.0.cmp(b.0)));
	ranked.into_iter().map(|(thread, (_, mail, _))| (thread.to_string(), mail)).collect()
}

#[cfg(test)]
mod tests {
	use super::*;

	#[test]
	fn typos_count_wrong_missing_extra_and_swapped_letters() {
		assert_eq!(typos("rutby", "rugby", 1), Some(1));
		assert_eq!(typos("rguby", "rugby", 1), Some(1));
		assert_eq!(typos("rugy", "rugby", 1), Some(1));
		assert_eq!(typos("rugbby", "rugby", 1), Some(1));
		assert_eq!(typos("rugby", "rugby", 1), Some(0));
		assert_eq!(typos("rutbu", "rugby", 1), None);
		assert_eq!(typos("rutbu", "rugby", 2), Some(2));
		assert_eq!(typos("tuch", "touchline", 2), None);
		assert_eq!(typos("zürich", "zurich", 1), Some(1));
		assert_eq!(typos_allowed(3), 0);
		assert_eq!(typos_allowed(5), 1);
		assert_eq!(typos_allowed(9), 2);
	}

	#[test]
	fn fixed_words_also_match_what_was_meant() {
		let q = Query::parse("touch rutby from:sam");
		assert_eq!(q.fts_fixed(&[None, Some("rugby".into())]).unwrap(), r#"sender : "sam"* AND "touch"* AND ("rutby"* OR "rugby")"#);
		assert_eq!(q.fts_fixed(&[]), q.fts(true));
	}

	#[test]
	fn queries_read_filters_and_quoted_phrases() {
		let q = Query::parse(r#"from:mia To:"Sam Okafor" subject:lunch has:attachment in:Inbox "next friday" photos"#);
		assert_eq!(q.from, ["mia"]);
		assert_eq!(q.to, ["Sam Okafor"]);
		assert_eq!(q.subject, ["lunch"]);
		assert!(q.attachment);
		assert_eq!(q.folder.as_deref(), Some("inbox"));
		assert_eq!(q.words, ["next friday", "photos"]);
		assert_eq!(q.text(), "next friday photos");
		// Unknown filters and a lone colon are words; an empty filter is nothing.
		assert_eq!(Query::parse("re:lunch 12:30 from:").words, ["re:lunch", "12:30"]);
		assert!(Query::parse("  from:  ").is_empty());
		assert!(!Query::parse("has:attachments").is_empty());
	}

	#[test]
	fn fts_expressions_quote_every_term() {
		let q = Query::parse(r#"from:mia inv AND OR NOT "x" NEAR(a b) col:* ^start"#);
		assert_eq!(
			q.fts(true).unwrap(),
			r#"sender : "mia"* AND "inv"* AND "AND"* AND "OR"* AND "NOT"* AND "x"* AND "NEAR(a"* AND "b)"* AND "col:*"* AND "^start"*"#
		);
		assert_eq!(q.fts(false).unwrap(), r#"sender : "mia"*"#);
		// A stray quote is taken literally, and terms without letters drop out.
		assert_eq!(Query::parse("a\"b").fts(true).unwrap(), r#""ab"*"#);
		assert_eq!(Query::parse("*** -- \"").fts(true), None);
	}

	#[test]
	fn plain_text_drops_markup() {
		let html = "<html><head><title>T</title><style>p{color:red}</style></head><body><p>Hi&nbsp;Mia,</p><p>Fish &amp; chips at <b>12:30</b>&#33; &#x263A; &bogus; AT&T</p><script>alert(1)</script><br/>Bye</body></html>";
		assert_eq!(plain_text(html), "Hi Mia, Fish & chips at 12:30 ! ☺ &bogus; AT&T Bye");
		assert_eq!(plain_text("no tags < here"), "no tags");
		assert_eq!(plain_text("<style/>kept"), "kept");
	}

	fn found(mail: i64, thread: &str, received: i64) -> Found {
		Found { mail, thread: thread.into(), received }
	}

	#[test]
	fn fusion_ranks_mails_in_both_lists_first() {
		let keyword = [found(1, "invoice", 100), found(2, "lunch", 200), found(3, "trip", 300)];
		let meaning = [found(2, "lunch", 200), found(4, "payment", 400)];
		// 2 is in both (1/62 + 1/61); 1 is top by keyword (1/61); 4 second by meaning (1/62) beats 3 third by keyword (1/63).
		assert_eq!(fuse(&[&keyword, &meaning]), [("lunch".into(), 2), ("invoice".into(), 1), ("payment".into(), 4), ("trip".into(), 3)]);
	}

	#[test]
	fn fusion_groups_conversations_by_their_best_mail() {
		// Two mails of one conversation: it shows once, with the better one, ahead of the other.
		let keyword = [found(1, "lunch", 100), found(2, "trip", 300), found(3, "lunch", 500)];
		assert_eq!(fuse(&[&keyword]), [("lunch".into(), 1), ("trip".into(), 2)]);
	}

	#[test]
	fn fusion_breaks_ties_by_the_newest_mail() {
		// Same ranks in different lists: same score, so the newer conversation goes first.
		let keyword = [found(1, "old", 100)];
		let meaning = [found(2, "new", 900)];
		assert_eq!(fuse(&[&keyword, &meaning]), [("new".into(), 2), ("old".into(), 1)]);
		// A conversation found by an older mail but holding a newer found one counts as newer.
		let keyword = [found(1, "a", 100), found(3, "b", 500)];
		let meaning = [found(2, "b", 50), found(4, "a", 900)];
		assert_eq!(fuse(&[&keyword, &meaning]), [("a".into(), 1), ("b".into(), 2)]);
		assert!(fuse(&[&[], &[]]).is_empty());
	}
}

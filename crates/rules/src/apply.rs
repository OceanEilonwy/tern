//! What is done with rules, besides reading their language: their file format, the buttons under a
//! rule's line, and working out what a set of rules does to some mail.

use tern_mail::{Folder, FolderId, FolderKind, Label, MailId, MailSummary};

use crate::{parse, Action, Rule};

/// The rules a file holds. A rule missing a part is left out; one whose line does not read is kept,
/// to be fixed, and catches nothing.
pub fn from_toml(raw: &str) -> Vec<Rule> {
	let table: toml::Table = raw.parse().unwrap_or_default();
	let Some(rules) = table.get("rule").and_then(|r| r.as_array()) else { return Vec::new() };
	rules
		.iter()
		.filter_map(|r| {
			let get = |key: &str| r.get(key).and_then(|v| v.as_str()).map(str::to_string);
			let action = match get("action")?.as_str() {
				"spam" => Action::Spam,
				"move" => Action::Move(get("target")?),
				"label" => Action::Label(get("target")?),
				_ => return None,
			};
			Some(Rule { when: get("when")?, action })
		})
		.collect()
}

pub fn to_toml(rules: &[Rule]) -> String {
	let quote = |s: &str| toml::Value::String(s.to_string()).to_string();
	let mut out = String::new();
	for rule in rules {
		out.push_str(&format!("[[rule]]\nwhen = {}\n", quote(&rule.when)));
		match &rule.action {
			Action::Spam => out.push_str("action = \"spam\"\n"),
			Action::Move(target) => out.push_str(&format!("action = \"move\"\ntarget = {}\n", quote(target))),
			Action::Label(target) => out.push_str(&format!("action = \"label\"\ntarget = {}\n", quote(target))),
		}
		out.push('\n');
	}
	out
}

/// What the buttons under a rule's line put in it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Piece {
	Field(&'static str),
	Word(&'static str),
	Brackets,
}

/// `line` with `piece` put in at char `cursor` (around the chars `selected`, for brackets), spaced
/// from its neighbours, and where the cursor goes after it.
pub fn insert(line: &str, cursor: usize, selected: Option<(usize, usize)>, piece: Piece) -> (String, usize) {
	let chars: Vec<char> = line.chars().collect();
	let (from, to) = selected.filter(|(a, b)| a < b && *b <= chars.len()).unwrap_or((cursor.min(chars.len()), cursor.min(chars.len())));
	let before: String = chars[..from].iter().collect();
	let inside: String = chars[from..to].iter().collect();
	let after: String = chars[to..].iter().collect();
	let gap_before = if before.is_empty() || before.ends_with(char::is_whitespace) || before.ends_with('(') { "" } else { " " };
	let gap_after = if after.is_empty() || after.starts_with(char::is_whitespace) || after.starts_with(')') { "" } else { " " };
	let (text, back) = match piece {
		// The cursor stays after the colon, ready for what to look for.
		Piece::Field(name) => (format!("{gap_before}{name}:"), 0),
		Piece::Word(word) => (format!("{gap_before}{word} "), 0),
		Piece::Brackets if inside.is_empty() => (format!("{gap_before}()"), 1),
		Piece::Brackets => (format!("({inside}){gap_after}"), 0),
	};
	let at = before.chars().count() + text.chars().count() - back;
	(format!("{before}{text}{after}"), at)
}

/// What a rule does, as Preferences shows it.
pub fn describe(action: &Action) -> String {
	match action {
		Action::Spam => "Move to Spam".into(),
		Action::Move(folder) => format!("Move to {folder}"),
		Action::Label(label) => format!("Label {label}"),
	}
}

/// The folder a rule's action moves mail to, if it moves it and the folder exists.
pub fn folder_for<'a>(action: &Action, folders: &'a [Folder]) -> Option<&'a Folder> {
	match action {
		Action::Spam => folders.iter().find(|f| f.kind == FolderKind::Spam),
		Action::Move(name) => folders.iter().find(|f| f.name.eq_ignore_ascii_case(name)),
		Action::Label(_) => None,
	}
}

/// What the rules do to `mails`: where each moves, and the labels each gets. A rule naming a
/// folder or label that is not there does nothing.
#[derive(Debug, Default, PartialEq)]
pub struct Plan {
	pub moves: Vec<(FolderId, Vec<MailId>)>,
	pub labels: Vec<(FolderId, Vec<MailId>)>,
}

fn add(to: &mut Vec<(FolderId, Vec<MailId>)>, target: &FolderId, id: &MailId) {
	match to.iter_mut().find(|(t, _)| t == target) {
		Some((_, ids)) => ids.push(id.clone()),
		None => to.push((target.clone(), vec![id.clone()])),
	}
}

pub fn plan(rules: &[Rule], mails: &[MailSummary], folders: &[Folder], labels: &[Label]) -> Plan {
	let rules: Vec<(&Rule, _)> = rules.iter().filter_map(|r| Some((r, parse(&r.when).ok()?))).collect();
	let mut plan = Plan::default();
	for mail in mails.iter().filter(|m| !m.outgoing) {
		let mut moved = false;
		for (rule, _) in rules.iter().filter(|(_, e)| e.matches(mail)) {
			match &rule.action {
				Action::Label(name) => {
					if let Some(label) = labels.iter().find(|l| l.name.eq_ignore_ascii_case(name)) {
						add(&mut plan.labels, &label.id, &mail.id);
					}
				},
				action if !moved => {
					if let Some(folder) = folder_for(action, folders) {
						add(&mut plan.moves, &folder.id, &mail.id);
						moved = true;
					}
				},
				_ => {},
			}
		}
	}
	plan
}

#[cfg(test)]
mod tests {
	use super::*;
	use tern_mail::Address;

	#[test]
	fn pieces_go_in_at_the_cursor_with_spaces_around() {
		assert_eq!(insert("", 0, None, Piece::Field("from")), ("from:".into(), 5));
		assert_eq!(insert("from:@a.org", 11, None, Piece::Word("or")), ("from:@a.org or ".into(), 15));
		assert_eq!(insert("from:@a.org or ", 15, None, Piece::Field("subject")), ("from:@a.org or subject:".into(), 23));
		assert_eq!(insert("lunch", 0, None, Piece::Word("not")), ("not lunch".into(), 4));
		assert_eq!(insert("a", 1, None, Piece::Brackets), ("a ()".into(), 3), "empty brackets put the cursor inside");
		assert_eq!(insert("a or b c", 0, Some((0, 6)), Piece::Brackets), ("(a or b) c".into(), 8), "a selection is wrapped");
		assert_eq!(insert("x", 0, Some((0, 1)), Piece::Brackets), ("(x)".into(), 3));
		assert_eq!(insert("é", 1, None, Piece::Field("to")), ("é to:".into(), 5), "counted in chars");
	}

	fn rule(when: &str, action: Action) -> Rule {
		Rule { when: when.into(), action }
	}

	#[test]
	fn rules_round_trip_and_broken_entries_are_left_out() {
		let rules = vec![
			rule("from:@deals.example", Action::Spam),
			rule("subject:\"invoice \\\" for\"", Action::Label("Receipts".into())),
			rule("(oops", Action::Move("Archive".into())),
		];
		assert_eq!(from_toml(&to_toml(&rules)), rules, "a line that does not read is kept for fixing");
		assert_eq!(from_toml("[[rule]]\nwhen = \"a\"\naction = \"move\"\n[[rule]]\naction = \"spam\"\n[[rule]]\nwhen = \"b\"\naction = \"spam\""), [rule("b", Action::Spam)]);
		assert_eq!(from_toml("not toml ["), []);
	}

	#[test]
	fn mail_this_account_sent_is_left_alone() {
		let folders = [Folder { id: FolderId("spam".into()), name: "Spam".into(), kind: FolderKind::Spam, unread: 0 }];
		let mut mail = MailSummary {
			sets: Vec::new(),
			id: MailId { list: "l".into(), element: "e".into() },
			from: Address::new("", "you@tuta.com"),
			subject: "coupon".into(),
			received_ms: 0,
			unread: false,
			has_attachments: false,
			to: None,
			outgoing: true,
			draft: false,
		};
		let rules = [rule("coupon", Action::Spam)];
		assert_eq!(plan(&rules, std::slice::from_ref(&mail), &folders, &[]), Plan::default());
		mail.outgoing = false;
		assert_eq!(plan(&rules, std::slice::from_ref(&mail), &folders, &[]).moves, [(FolderId("spam".into()), vec![mail.id])]);
	}
}

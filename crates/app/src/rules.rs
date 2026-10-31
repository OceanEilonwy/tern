//! The mail rules, in `~/.config/tern/rules.toml`, and running them on new mail in the Inbox.
//!
//! A rule catches mail by a line in the language of [`tern_core::rules`] and sends it to Spam, moves
//! it to a folder or labels it. Rules run on mail that arrives after they are made, once each sync
//! has fetched it; mail already there is moved from the Move to Spam dialog instead. Of the rules
//! that catch a mail, the first that moves it wins, and every label applies.

use std::path::PathBuf;

use tern_cache::Cache;
use tern_core::rules::{parse, Action, Rule};
use tern_core::{Backend, Folder, FolderId, FolderKind, Label, MailId, MailSummary};

/// How far new mail in the Inbox has been through the rules: the newest received time looked at.
const MARK: &str = "rules";

fn path() -> PathBuf {
	crate::settings::dir().join("rules.toml")
}

/// The saved rules; none when there are none or the file does not read. A rule missing a part is
/// left out; one whose line does not read is kept, to be fixed in Preferences, and catches nothing.
pub fn load() -> Vec<Rule> {
	std::fs::read_to_string(path()).map(|raw| from_toml(&raw)).unwrap_or_default()
}

pub fn save(rules: &[Rule]) {
	let path = path();
	if let Some(dir) = path.parent() {
		let _ = std::fs::create_dir_all(dir);
	}
	if let Err(e) = std::fs::write(path, to_toml(rules)) {
		eprintln!("tern: could not save rules: {e}");
	}
}

fn from_toml(raw: &str) -> Vec<Rule> {
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

fn to_toml(rules: &[Rule]) -> String {
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
struct Plan {
	moves: Vec<(FolderId, Vec<MailId>)>,
	labels: Vec<(FolderId, Vec<MailId>)>,
}

fn add(to: &mut Vec<(FolderId, Vec<MailId>)>, target: &FolderId, id: &MailId) {
	match to.iter_mut().find(|(t, _)| t == target) {
		Some((_, ids)) => ids.push(id.clone()),
		None => to.push((target.clone(), vec![id.clone()])),
	}
}

fn plan(rules: &[Rule], mails: &[MailSummary], folders: &[Folder], labels: &[Label]) -> Plan {
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

/// Runs the rules on mail that came into the Inbox since they last ran. The first time, nothing
/// there yet is touched. Returns how many mails moved; on an error the same mail is tried next time.
pub async fn run_on_new(backend: &dyn Backend, cache: &Cache, rules: &[Rule]) -> tern_core::Result<usize> {
	let error = |e: tern_cache::CacheError| tern_core::Error::Other(e.to_string());
	let folders = cache.folders().map_err(error)?;
	let Some(inbox) = folders.iter().find(|f| f.kind == FolderKind::Inbox) else { return Ok(0) };
	let newest = |mails: &[MailSummary]| mails.iter().map(|m| m.received_ms).max();
	let Some(mark) = cache.mark(MARK).map_err(error)? else {
		let all = cache.received_after(&inbox.id, i64::MIN).map_err(error)?;
		cache.set_mark(MARK, newest(&all).unwrap_or(0)).map_err(error)?;
		return Ok(0);
	};
	let mails = cache.received_after(&inbox.id, mark).map_err(error)?;
	let Some(newest) = newest(&mails) else { return Ok(0) };
	let any_label = rules.iter().any(|r| matches!(r.action, Action::Label(_)));
	let labels = if any_label { backend.labels().await? } else { Vec::new() };
	let plan = plan(rules, &mails, &folders, &labels);
	for (label, ids) in &plan.labels {
		backend.add_label(ids, label).await?;
	}
	let mut moved = 0;
	for (folder, ids) in &plan.moves {
		backend.move_mails(ids, folder).await?;
		for id in ids {
			cache.move_to(id, folder).map_err(error)?;
		}
		moved += ids.len();
	}
	cache.set_mark(MARK, newest).map_err(error)?;
	Ok(moved)
}

#[cfg(test)]
mod tests {
	use super::*;
	use tern_cache::sync;
	use tern_core::demo::DemoBackend;
	use tern_core::Address;

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

	#[tokio::test]
	async fn new_mail_is_moved_and_labelled_by_the_first_rule_that_moves_it() {
		let demo = DemoBackend::new();
		let cache = Cache::in_memory();
		for f in sync::folders(&demo, &cache).await.unwrap() {
			sync::folder(&demo, &cache, &f.id, 100).await.unwrap();
		}
		let rules = vec![
			rule("from:@deals.example", Action::Label("Travel".into())),
			rule("coupon", Action::Move("Archive".into())),
			rule("from:@deals.example", Action::Spam),
			rule("from:@deals.example", Action::Label("No Such Label".into())),
			rule("(broken", Action::Spam),
		];
		// The first run only marks where new mail starts.
		assert_eq!(run_on_new(&demo, &cache, &rules).await.unwrap(), 0);
		let deal = cache.matching(&parse("from:@deals.example").unwrap(), &[], 10).unwrap().0;
		assert!(deal.iter().all(|(_, f)| f.0 == "inbox"), "mail already there stays");

		// As if the offers came in after that.
		let oldest = deal.iter().map(|(m, _)| m.received_ms).min().unwrap();
		cache.set_mark(MARK, oldest - 1).unwrap();
		assert_eq!(run_on_new(&demo, &cache, &rules).await.unwrap(), 3);
		let (coupon, offer) = (&deal.iter().find(|(m, _)| m.subject.contains("coupon")).unwrap().0, &deal[0].0);
		assert_eq!(demo.place_of(&coupon.id).unwrap().0, FolderKind::Archive, "the first rule that moves wins");
		assert_eq!(demo.place_of(&offer.id).unwrap().0, FolderKind::Spam);
		let travel = FolderId("label-travel".into());
		assert_eq!(demo.place_of(&offer.id).unwrap().1, std::slice::from_ref(&travel), "labels apply as well, missing ones do nothing");
		assert_eq!(demo.place_of(&coupon.id).unwrap().1, [travel]);
		let spam = cache.folders().unwrap().into_iter().find(|f| f.kind == FolderKind::Spam).unwrap().id;
		assert_eq!(cache.list(&spam, 0, 10).unwrap().len(), 2, "the cache shows the moves at once");
		// Done once: the next run finds nothing new.
		assert_eq!(run_on_new(&demo, &cache, &rules).await.unwrap(), 0);
	}
}

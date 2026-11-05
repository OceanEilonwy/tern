//! Rules, run on the mail a sync has brought in.

use tern_cache::Cache;
use tern_mail::{Backend, FolderKind, MailSummary};
use tern_rules::{plan, Action, Rule};

/// How far new mail in the Inbox has been through the rules: the newest received time looked at.
const MARK: &str = "rules";

/// Runs the rules on mail that came into the Inbox since they last ran. The first time, nothing
/// there yet is touched. Returns how many mails moved; on an error the same mail is tried next time.
pub async fn run_on_new(backend: &dyn Backend, cache: &Cache, rules: &[Rule]) -> tern_mail::Result<usize> {
	let error = |e: tern_cache::CacheError| tern_mail::Error::Other(e.to_string());
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
	use crate as sync;
	use tern_demo::DemoBackend;
	use tern_mail::FolderId;
	use tern_rules::parse;

	fn rule(when: &str, action: Action) -> Rule {
		Rule { when: when.into(), action }
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

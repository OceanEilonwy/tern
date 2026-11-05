//! Runs the sync's rounds (see `tern_sync::round`) for the account signed in now, and shows what they
//! find out. A round runs at sign-in, every few minutes after, and at once when something asks for
//! it (F5, a changed setting). After each, the mail's meaning for search when that is turned on, and
//! the offer to clean up.

use std::rc::Weak;
use std::sync::Arc;
use std::time::Duration;

use tern_mail::Error;
use tern_sync::{Event, Job, Stops};
use tokio::sync::mpsc::unbounded_channel;

use crate::activity::Word;
use crate::mailbox::Mailbox;
use crate::{cleanup, rules, runtime, window::Ctx};

const INTERVAL: Duration = Duration::from_secs(5 * 60);

/// What the person is told about the connection, while there is something to tell.
fn problem_word(e: &Error) -> Option<&'static str> {
	match e {
		Error::Offline => Some("Offline"),
		Error::RateLimited(_) => Some("Server busy"),
		Error::SessionExpired => Some("Signed out"),
		_ => None,
	}
}

/// Runs the sync for the account signed in now, until it signs out.
pub fn start(ctx: &Ctx, mailbox: Weak<Mailbox>) {
	let (Some(backend), Some(cache), Some(fetcher)) = (ctx.backend(), ctx.cache(), ctx.fetcher()) else { return };
	let ctx = ctx.clone();
	runtime::spawn_local(async move {
		let signed_in = |ctx: &Ctx| ctx.backend().is_some_and(|now| Arc::ptr_eq(&now, &backend));
		let contacts_done = Arc::default();
		while signed_in(&ctx) {
			let level = ctx.settings().offline;
			let busy = ctx.activity.start(Word::Fetching);
			let on_screen = ctx.on_screen.clone();
			let job = Job {
				backend: backend.clone(),
				cache: cache.clone(),
				fetcher: fetcher.clone(),
				level,
				rules: rules::load(crate::paths()),
				on_screen: Arc::new(move || on_screen.lock().unwrap_or_else(|p| p.into_inner()).clone()),
				// The desktop asks for labels when it needs them.
				labels: false,
				contacts_done: Arc::clone(&contacts_done),
			};
			let (events, mut received) = unbounded_channel();
			let round = runtime::handle().spawn(async move { tern_sync::round(&job, &|event| drop(events.send(event))).await });
			// Signing out drops this and so ends the round, which ends the events.
			ctx.set_sync(Some(Stops(round.abort_handle())));
			while let Some(event) = received.recv().await {
				if !signed_in(&ctx) {
					return;
				}
				let Some(mb) = mailbox.upgrade() else { return };
				match event {
					Event::Folders(_) => {
						ctx.activity.set_problem(None);
						mb.cache_changed(None);
					},
					Event::Labels(_) => {},
					Event::FolderChanged(id) => mb.cache_changed(Some(&id)),
					Event::RulesMoved => mb.cache_changed(None),
					Event::Problem(e) => ctx.activity.set_problem(problem_word(&e)),
					Event::NoFolders(e) => {
						ctx.activity.set_problem(problem_word(&e));
						mb.sync_failed(&e);
					},
				}
			}
			let Ok(finished) = round.await else { return };
			if !signed_in(&ctx) {
				return;
			}
			if finished.listed && ctx.settings().semantic {
				if let Err(e) = runtime::run(crate::meaning().clone().index(cache.clone())).await {
					log::warn!("could not index mail by meaning: {e}");
				}
				// Turned off meanwhile: what was worked out goes too.
				if !ctx.settings().semantic {
					crate::meaning().forget(Some(&cache));
				}
			}
			cleanup::check(&ctx).await;
			drop(busy);
			let wake = ctx.wake();
			runtime::run(async move {
				tokio::select! {
					_ = tokio::time::sleep(INTERVAL) => {},
					_ = wake.notified() => {},
				}
			})
			.await;
		}
	});
}

#[cfg(test)]
mod tests {
	use super::*;

	#[test]
	fn only_the_connection_is_worth_a_word() {
		assert_eq!(problem_word(&Error::Offline), Some("Offline"));
		assert_eq!(problem_word(&Error::RateLimited(None)), Some("Server busy"));
		assert_eq!(problem_word(&Error::SessionExpired), Some("Signed out"));
		assert_eq!(problem_word(&Error::Other("a mail was odd".into())), None);
	}
}

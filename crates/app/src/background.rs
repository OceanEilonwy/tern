//! Keeps the cache up to date while signed in: the folder list, each folder's newest mail, contacts,
//! then the newest mails opened ahead by the fetcher, and their meaning for search when that is
//! turned on. A round runs at sign-in, every few minutes after, and at once when something asks for
//! it (F5, a changed setting).

use std::rc::Weak;
use std::sync::Arc;
use std::time::Duration;

use tern_cache::sync;
use tern_core::{Error, FolderKind};

use crate::activity::Word;
use crate::mailbox::Mailbox;
use crate::{runtime, semantic, window::Ctx};

const INTERVAL: Duration = Duration::from_secs(5 * 60);

/// Runs the sync for the account signed in now, until it signs out.
pub fn start(ctx: &Ctx, mailbox: Weak<Mailbox>) {
	let (Some(backend), Some(cache), Some(fetcher)) = (ctx.backend(), ctx.cache(), ctx.fetcher()) else { return };
	let ctx = ctx.clone();
	runtime::spawn_local(async move {
		let signed_in = |ctx: &Ctx| ctx.backend().is_some_and(|now| Arc::ptr_eq(&now, &backend));
		let mut contacts_done = false;
		loop {
			if !signed_in(&ctx) {
				return;
			}
			let level = ctx.settings().offline;
			let busy = ctx.activity.start(Word::Fetching);
			let (b, c) = (backend.clone(), cache.clone());
			let folders = runtime::run(async move { sync::folders(b.as_ref(), &c).await }).await;
			let Some(mb) = mailbox.upgrade() else { return };
			match folders {
				Ok(mut folders) => {
					if !signed_in(&ctx) {
						return;
					}
					mb.cache_changed(None);
					// The inbox first, then the others in their order.
					folders.sort_by_key(|f| f.kind != FolderKind::Inbox);
					drop(mb);
					for folder in folders {
						let (b, c, id) = (backend.clone(), cache.clone(), folder.id.clone());
						let result = runtime::run(async move { sync::folder(b.as_ref(), &c, &id, level.headers()).await }).await;
						if !signed_in(&ctx) {
							return;
						}
						match result {
							Ok(true) => {
								if let Some(mb) = mailbox.upgrade() {
									mb.cache_changed(Some(&folder.id));
								}
							},
							Ok(false) => {},
							Err(e @ (Error::Offline | Error::SessionExpired)) => {
								eprintln!("tern: sync stopped: {e}");
								break;
							},
							Err(e) => eprintln!("tern: could not sync {}: {e}", folder.name),
						}
					}
					if !contacts_done {
						let (b, c) = (backend.clone(), cache.clone());
						contacts_done = runtime::run(async move { sync::contacts(b.as_ref(), &c).await }).await.is_ok();
					}
					let f = fetcher.clone();
					if let Err(e) = runtime::run(async move { f.ahead(level.bodies()).await }).await {
						eprintln!("tern: could not open mail ahead: {e}");
					}
					if ctx.settings().semantic {
						if let Err(e) = runtime::run(semantic::index(cache.clone())).await {
							eprintln!("tern: could not index mail by meaning: {e}");
						}
						// Turned off meanwhile: what was worked out goes too.
						if !ctx.settings().semantic {
							semantic::forget(Some(&cache));
						}
					}
				},
				Err(e) => mb.sync_failed(&e),
			}
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

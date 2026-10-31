//! Clean Up: how much Tern takes on disk, the sidebar's offer to clean up once that passes the size
//! set in Preferences (or the disk runs low), and the dialog that frees room.
//!
//! Mail past Keep Ready is trimmed after every sync anyway (see `Cache::trim`); this is for what
//! is left: the text of older mail opened, meanings worked out for search, the language model, or
//! everything at once.

use std::cell::RefCell;
use std::rc::Rc;

use adw::prelude::*;
use gtk::{gio, glib};
use tern_core::FolderKind;

use crate::window::Ctx;
use crate::{runtime, semantic};

/// What Tern takes on disk, in bytes, and what each choice in the dialog would free.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Usage {
	/// The cache with its log, and the language model.
	pub total: u64,
	/// Mail past Keep Ready, the text of older mail opened, and room already free in the file.
	pub older: u64,
	/// The mails' meanings for search; 0 when search by meaning is off, as they are gone then.
	pub vectors: u64,
	/// The language model, counted only while search by meaning is off: in use, it would only be
	/// downloaded again.
	pub model: u64,
}

/// With the disk this full (free below 1/20 of it), the sidebar offers to clean up whatever the
/// setting says, once Tern takes at least `LOW_DISK_MIN`.
const LOW_DISK_SHARE: u64 = 20;
const LOW_DISK_MIN: u64 = 100_000_000;

/// Whether the sidebar offers to clean up: Tern takes more than `limit_mb` megabytes (0 for never),
/// or the disk is nearly full and Tern takes enough to matter. `disk` is (free, size) in bytes.
pub fn offer(total: u64, limit_mb: u32, disk: Option<(u64, u64)>) -> bool {
	let past = limit_mb > 0 && total > u64::from(limit_mb) * 1_000_000;
	let low = disk.is_some_and(|(free, size)| size > 0 && free * LOW_DISK_SHARE < size);
	past || (low && total >= LOW_DISK_MIN)
}

/// Free and total bytes of the disk the cache is on.
fn disk() -> Option<(u64, u64)> {
	let path = crate::window::cache_path("x");
	let dir = path.ancestors().find(|p| p.exists())?;
	let info = gio::File::for_path(dir).query_filesystem_info("filesystem::free,filesystem::size", gio::Cancellable::NONE).ok()?;
	Some((info.attribute_uint64("filesystem::free"), info.attribute_uint64("filesystem::size")))
}

/// Measures, off the UI thread, what Tern takes now; None when signed out.
pub async fn measure(ctx: &Ctx) -> Option<Usage> {
	let cache = ctx.cache()?;
	let (level, meaning) = (ctx.settings().offline, ctx.settings().semantic);
	runtime::run(async move {
		tokio::task::spawn_blocking(move || {
			let model = semantic::model_bytes();
			Usage {
				total: cache.disk_bytes() + model,
				older: cache.older_bytes(level.headers(), level.bodies()).unwrap_or(0),
				vectors: cache.vector_bytes().unwrap_or(0),
				model: if meaning { 0 } else { model },
			}
		})
		.await
		.ok()
	})
	.await
}

/// Measures and shows or takes back the sidebar's offer to clean up.
pub async fn check(ctx: &Ctx) {
	let Some(usage) = measure(ctx).await else { return };
	let shown = offer(usage.total, ctx.settings().cleanup_mb, disk());
	ctx.offer_cleanup(shown.then_some(usage.total));
}

/// A choice in the dialog besides everything.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Part {
	Older,
	Vectors,
	Model,
}

/// What to clear.
#[derive(Clone, Copy, Debug, Default)]
struct Pick {
	older: bool,
	vectors: bool,
	model: bool,
	all: bool,
}

/// Clears what was picked, gives the room back to the disk, and has the lists fetch again.
/// Returns the bytes freed.
async fn clean(ctx: &Ctx, pick: Pick) -> u64 {
	let Some(cache) = ctx.cache() else { return 0 };
	let level = ctx.settings().offline;
	let freed = runtime::run(async move {
		tokio::task::spawn_blocking(move || {
			let before = cache.disk_bytes();
			let done = if pick.all {
				cache.clear_all()
			} else {
				(if pick.older { cache.clear_older(level.headers(), level.bodies()) } else { Ok(()) })
					.and_then(|()| if pick.vectors { cache.clear_vectors() } else { Ok(()) })
			};
			if let Err(e) = done.and_then(|()| cache.compact()) {
				eprintln!("tern: could not clean up: {e}");
			}
			before.saturating_sub(cache.disk_bytes())
		})
		.await
		.unwrap_or(0)
	})
	.await;
	let model = if pick.model {
		let bytes = semantic::model_bytes();
		semantic::remove_model();
		bytes
	} else {
		0
	};
	use FolderKind::*;
	ctx.changed(&[Inbox, Drafts, Sent, Archive, Spam, Trash, Custom]);
	ctx.sync_now();
	check(ctx).await;
	freed + model
}

/// A choice in the dialog: a tick, a name with what it means, and its size.
fn choice(title: &str, subtitle: &str, bytes: u64) -> (adw::ActionRow, gtk::CheckButton) {
	let check = gtk::CheckButton::builder().valign(gtk::Align::Center).build();
	let row = adw::ActionRow::builder().title(title).subtitle(subtitle).activatable_widget(&check).build();
	row.add_prefix(&check);
	row.add_suffix(&gtk::Label::builder().label(glib::format_size(bytes).as_str()).css_classes(["dim-label", "numeric"]).build());
	(row, check)
}

/// Opens Clean Up over `parent`; `done` hears how many bytes were freed.
pub fn open(ctx: &Ctx, parent: &gtk::Widget, done: impl Fn(u64) + 'static) {
	let (ctx, parent) = (ctx.clone(), parent.clone());
	runtime::spawn_local(async move {
		let Some(usage) = measure(&ctx).await else { return };
		dialog(&ctx, usage, done).0.present(Some(&parent));
	});
}

/// The dialog for `usage`, with its "All Mail" tick. Only "Older Mail" is ticked to start, as it
/// costs nothing to clear.
pub fn dialog(ctx: &Ctx, usage: Usage, done: impl Fn(u64) + 'static) -> (adw::AlertDialog, gtk::CheckButton) {
	let dialog = adw::AlertDialog::builder()
		.heading("Clean Up")
		.body(format!("Tern takes {} on this computer.", glib::format_size(usage.total)))
		.prefer_wide_layout(true)
		.build();
	let list = gtk::ListBox::builder().css_classes(["boxed-list"]).selection_mode(gtk::SelectionMode::None).build();
	let older = choice("Older Mail", "Mail past Keep Ready, and the text of older mail you opened. It downloads again when needed.", usage.older);
	older.1.set_active(usage.older > 0);
	list.append(&older.0);
	let mut parts = vec![(Part::Older, older.1.clone(), usage.older, older.0.clone())];
	if usage.vectors > 0 {
		let vectors = choice("Search by Meaning Data", "Worked out again in the background.", usage.vectors);
		list.append(&vectors.0);
		parts.push((Part::Vectors, vectors.1, usage.vectors, vectors.0));
	}
	if usage.model > 0 {
		let model = choice("Language Model", "Downloaded again if search by meaning is turned on.", usage.model);
		list.append(&model.0);
		parts.push((Part::Model, model.1, usage.model, model.0));
	}
	let everything = gtk::ListBox::builder().css_classes(["boxed-list"]).selection_mode(gtk::SelectionMode::None).build();
	let all = choice("All Mail on This Computer", "Everything downloads again, like a first sign-in.", usage.total);
	all.0.add_css_class("cleanup-all");
	everything.append(&all.0);
	let note = gtk::Label::builder()
		.label("Pinned conversations, drafts, rules and settings always stay.")
		.css_classes(["dim-label", "caption"])
		.wrap(true)
		.justify(gtk::Justification::Center)
		.build();
	let content = gtk::Box::builder().orientation(gtk::Orientation::Vertical).spacing(12).build();
	content.append(&list);
	content.append(&everything);
	content.append(&note);
	dialog.set_extra_child(Some(&content));
	dialog.add_response("cancel", "_Cancel");
	dialog.add_response("clean", "");
	dialog.set_close_response("cancel");

	let parts = Rc::new(parts);
	let update = {
		let (dialog, parts, all) = (dialog.clone(), parts.clone(), all.1.clone());
		move || {
			let everything = all.is_active();
			for (_, _, _, row) in parts.iter() {
				row.set_sensitive(!everything);
			}
			let bytes: u64 = parts.iter().filter(|(_, c, _, _)| c.is_active()).map(|(_, _, b, _)| b).sum();
			if everything {
				dialog.set_response_label("clean", "_Delete All Mail");
				dialog.set_response_appearance("clean", adw::ResponseAppearance::Destructive);
			} else {
				dialog.set_response_label("clean", &format!("_Free {}", glib::format_size(bytes)));
				dialog.set_response_appearance("clean", adw::ResponseAppearance::Suggested);
			}
			dialog.set_response_enabled("clean", everything || parts.iter().any(|(_, c, _, _)| c.is_active()));
			dialog.set_default_response(Some("clean"));
		}
	};
	update();
	let update = Rc::new(update);
	for check in parts.iter().map(|(_, c, _, _)| c).chain([&all.1]) {
		let update = update.clone();
		check.connect_toggled(move |_| update());
	}

	let pick = {
		let (parts, all) = (parts.clone(), all.1.clone());
		move || {
			let ticked = |part: Part| parts.iter().any(|(p, c, _, _)| *p == part && c.is_active());
			Pick { older: ticked(Part::Older), vectors: ticked(Part::Vectors), model: ticked(Part::Model), all: all.is_active() }
		}
	};
	let ctx = ctx.clone();
	let done = Rc::new(RefCell::new(Some(done)));
	dialog.connect_response(None, move |_, response| {
		if response != "clean" {
			return;
		}
		let (ctx, pick, done) = (ctx.clone(), pick(), done.clone());
		runtime::spawn_local(async move {
			let freed = clean(&ctx, pick).await;
			if let Some(done) = done.borrow_mut().take() {
				done(freed);
			}
		});
	});
	(dialog, all.1)
}

/// The Storage row's line: what Tern takes, and what could go.
pub fn storage_text(usage: Usage) -> String {
	let can_go = usage.older + usage.vectors + usage.model;
	format!("{} on this computer · {} can go", glib::format_size(usage.total), glib::format_size(can_go))
}

/// What the toast says once done.
pub fn freed_toast(bytes: u64) -> String {
	format!("Freed {}", glib::format_size(bytes))
}

#[cfg(test)]
mod tests {
	use super::*;

	#[test]
	fn the_offer_shows_past_the_limit_or_on_a_nearly_full_disk() {
		const GB: u64 = 1_000_000_000;
		let roomy = Some((500 * GB, 1000 * GB));
		assert!(!offer(GB, 1000, roomy), "at the limit is not past it");
		assert!(offer(GB + 1, 1000, roomy));
		assert!(offer(600_000_000, 500, roomy));
		assert!(!offer(9 * GB, 10000, roomy));
		assert!(!offer(50 * GB, 0, roomy), "never");
		// Nearly full: offered whatever the setting, once Tern takes enough to matter.
		let full = Some((40 * GB, 1000 * GB));
		assert!(offer(200_000_000, 0, full));
		assert!(offer(200_000_000, 10000, full));
		assert!(!offer(50_000_000, 1000, full), "too little to help");
		assert!(!offer(200_000_000, 1000, None), "disk unknown");
	}
}

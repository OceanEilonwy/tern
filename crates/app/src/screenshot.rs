//! A hidden debug path that builds a scene, saves a PNG of it and quits.
//!
//! `TERN_SCREENSHOT=/path/out.png TERN_SCREENSHOT_SCENE=login|sign-in|folders-fail|refresh-race|sign-out-race|draft|suggest|keys|arrows|busy|panes|search|focus-first|spam|rules|inbox|reading|invoice|newsletter|compose|spam-dialog|rule-edit|narrow|narrow-reading`
//! (with `TERN_DEMO=1` for all but login and sign-in; sign-in types "demo" on the sign-in page). For dark mode add `ADW_DEBUG_COLOR_SCHEME=prefer-dark`.

use std::cell::Cell;
use std::path::PathBuf;
use std::rc::Rc;
use std::time::Duration;

use std::sync::Arc;

use adw::prelude::*;
use gtk::{glib, graphene};
use tern_core::demo::DemoBackend;
use tern_core::{Backend, NewAttachment};

use crate::activity::Word;
use crate::compose_model::Fields;
use crate::composer::Composer;
use crate::mailbox::RAIL;
use crate::window::Window;
use crate::{rules, spam};
use tern_core::rules::{Action, Rule};
use tern_core::FolderKind;

pub fn scene() -> Option<(PathBuf, String)> {
	let path = std::env::var_os("TERN_SCREENSHOT")?;
	let scene = std::env::var("TERN_SCREENSHOT_SCENE").unwrap_or_else(|_| "inbox".into());
	Some((path.into(), scene))
}

/// The window size for a scene, set before the window first shows.
pub fn size() -> Option<(i32, i32)> {
	let (_, scene) = scene()?;
	Some(if scene.starts_with("narrow") { (400, 780) } else { (1400, 860) })
}

async fn sleep(ms: u64) {
	glib::timeout_future(Duration::from_millis(ms)).await;
}

/// Waits up to five seconds for `ready`.
async fn wait_for(ready: impl Fn() -> bool) {
	for _ in 0..100 {
		if ready() {
			return;
		}
		sleep(50).await;
	}
	eprintln!("tern: screenshot scene did not get ready");
}

/// Signs in from the sign-in page holding only a weak reference, as the app does, so the scene fails
/// if nothing else keeps the window alive.
fn sign_in(win: std::rc::Weak<Window>, path: PathBuf) {
	glib::spawn_future_local(async move {
		sleep(300).await;
		let Some(w) = win.upgrade() else { return eprintln!("tern: screenshot failed: the window was dropped") };
		w.login.sign_in_as("demo");
		let window = w.ctx.window.clone();
		drop(w);
		wait_for(|| win.upgrade().is_some_and(|w| w.mailbox.store.n_items() > 0)).await;
		save_and_quit(window.upcast_ref(), &path, window.application()).await;
	});
}

async fn save_and_quit(target: &gtk::Widget, path: &std::path::Path, app: Option<gtk::Application>) {
	// Text views size themselves from their last layout, so give them a few passes to settle.
	for _ in 0..6 {
		target.queue_resize();
		target.queue_draw();
		sleep(250).await;
	}
	// Broadway draws only while a client watches; try until a frame has been drawn.
	let mut result: std::result::Result<(), String> = Err(String::new());
	for _ in 0..25 {
		target.queue_draw();
		sleep(200).await;
		result = save(target, path);
		if result.is_ok() {
			break;
		}
	}
	if let Err(e) = result {
		eprintln!("tern: screenshot failed: {e}");
	}
	if let Some(app) = app {
		app.quit();
	}
}

/// The scenes that check the window recovers from failing or slow calls.
async fn check(win: &Rc<Window>, scene: &str) -> std::result::Result<(), String> {
	let mailbox = win.mailbox.clone();
	let app = win.ctx.window.application().ok_or("no application")?;
	match scene {
		// Folders that fail to load say so instead of spinning.
		"folders-fail" => {
			wait_for(|| mailbox.status().is_some()).await;
			match mailbox.status() {
				Some(title) if title == "Could Not Load Folders" => Ok(()),
				other => Err(format!("expected the folder error, got {other:?}")),
			}
		}
		// A refresh while the first page loads must not leave the list unable to load more.
		"refresh-race" => {
			wait_for(|| mailbox.is_loading()).await;
			gtk::prelude::WidgetExt::activate_action(&win.ctx.window, "win.refresh", None).map_err(|e| e.to_string())?;
			wait_for(|| mailbox.store.n_items() > 0 && !mailbox.is_loading()).await;
			sleep(1000).await;
			if mailbox.is_loading() { Err("the list still counts a page as loading".into()) } else { Ok(()) }
		}
		// Folders that arrive after signing out must not stop the next sign-in from opening the inbox.
		"sign-out-race" => {
			sleep(100).await;
			app.activate_action("sign-out", None);
			sleep(800).await;
			win.login.sign_in_as("demo");
			wait_for(|| mailbox.store.n_items() > 0).await;
			if mailbox.store.n_items() > 0 { Ok(()) } else { Err("signing in again showed no mail".into()) }
		}
		// Closing a composer keeps the message in Drafts; it reopens with its fields and sends.
		"draft" => {
			wait_for(|| mailbox.store.n_items() > 0).await;
			let fields = Fields { to: "mia@tuta.com".into(), subject: "Trip plan".into(), body: "Day one: the coast.".into(), ..Default::default() };
			let composer = Composer::open(&win.ctx, fields, None);
			let attachment = NewAttachment { name: "map.pdf".into(), mime: "application/pdf".into(), data: b"%PDF map".to_vec() };
			composer.attach(vec![attachment]);
			composer.window.close();
			drop(composer);
			mailbox.select_folder(tern_core::FolderKind::Drafts);
			wait_for(|| mailbox.select_by_subject("Trip plan")).await;
			wait_for(|| mailbox.reader.current.borrow().is_some()).await;
			if !mailbox.reader.mails().iter().any(|m| m.draft) {
				return Err(format!("the saved draft did not show as a draft: {:?}", mailbox.reader.mails().iter().map(|m| (&m.subject, m.draft)).collect::<Vec<_>>()));
			}
			let again = mailbox.reader.edit_current_draft().ok_or("the draft did not open for editing")?;
			if again.subject_and_to() != ("Trip plan".to_string(), "mia@tuta.com".to_string()) {
				return Err(format!("the draft reopened as {:?}", again.subject_and_to()));
			}
			again.send();
			wait_for(|| !again.window.is_visible()).await;
			drop(again);
			wait_for(|| mailbox.store.n_items() == 0).await;
			if mailbox.store.n_items() != 0 {
				return Err("the sent draft stayed in Drafts".into());
			}
			mailbox.select_folder(tern_core::FolderKind::Sent);
			wait_for(|| mailbox.select_by_subject("Trip plan")).await;
			wait_for(|| mailbox.reader.current.borrow().as_ref().is_some_and(|o| o.mail.attachments.len() == 1)).await;
			if mailbox.reader.current.borrow().as_ref().is_some_and(|o| o.mail.attachments.len() == 1) { Ok(()) } else { Err("the sent mail lost its attachment".into()) }
		}
		// A reply carries only the lines picked in the mail until the whole mail is added; a forward
		// carries the mail until it is left out.
		"quote" => {
			wait_for(|| mailbox.store.n_items() > 0).await;
			let backend = win.ctx.backend().ok_or("signed out")?;
			let sent = win.ctx.cache().and_then(|c| c.folders().ok()?.into_iter().find(|f| f.kind == FolderKind::Sent)).ok_or("no Sent")?;
			let newest_sent = || {
				let (backend, sent) = (backend.clone(), sent.clone());
				async move {
					let page = backend.list(&sent.id, None, 50).await.map_err(|e| e.to_string())?;
					let newest = page.mails.into_iter().max_by_key(|m| m.received_ms).ok_or("Sent is empty")?;
					backend.open(&newest.id).await.map(|m| m.body_html).map_err(|e| e.to_string())
				}
			};
			mailbox.select_by_subject("Lunch");
			wait_for(|| mailbox.reader.current.borrow().as_ref().is_some_and(|o| o.mail.summary.subject.starts_with("Re:"))).await;
			sleep(300).await;
			if !mailbox.reader.select_in_current("12:30 at the usual place?") {
				return Err("the reply's text is not in the open mail".into());
			}
			let reply = mailbox.reader.answer(crate::compose_model::Answer::Reply).ok_or("no reply")?;
			let quote = reply.quote();
			if quote.picked != Some(("Sam Okafor".into(), "12:30 at the usual place?".into())) || quote.whole.as_ref().is_none_or(|w| w.included || w.forward) {
				return Err(format!("the reply quotes {quote:?}"));
			}
			reply.send();
			wait_for(|| !reply.window.is_visible()).await;
			let html = newest_sent().await?;
			if !html.starts_with("<div>Sam Okafor wrote:</div><blockquote") || html.contains("Friday works") {
				return Err(format!("a reply with a pick sent {html}"));
			}
			// Nothing picked, the pick left out, the whole mail added.
			let reply = mailbox.reader.answer(crate::compose_model::Answer::Reply).ok_or("no reply")?;
			if reply.quote().picked.is_some() {
				reply.drop_picked();
			}
			reply.include_whole(true);
			reply.send();
			wait_for(|| !reply.window.is_visible()).await;
			let html = newest_sent().await?;
			if html.contains("Sam Okafor wrote:</div>") || !html.contains("wrote:</div><blockquote") || !html.contains("Friday works") {
				return Err(format!("a reply with the whole mail sent {html}"));
			}
			// A forward carries the mail; × leaves it out.
			let forward = mailbox.reader.answer(crate::compose_model::Answer::Forward).ok_or("no forward")?;
			if !forward.quote().whole.is_some_and(|w| w.included && w.forward) {
				return Err(format!("the forward carries {:?}", forward.quote()));
			}
			forward.type_to("mia@tuta.com");
			forward.send();
			wait_for(|| !forward.window.is_visible()).await;
			if !newest_sent().await?.contains("---------- Forwarded message ----------") {
				return Err("the forward went without the mail".into());
			}
			let forward = mailbox.reader.answer(crate::compose_model::Answer::Forward).ok_or("no forward")?;
			forward.include_whole(false);
			forward.type_to("mia@tuta.com");
			forward.send();
			wait_for(|| !forward.window.is_visible()).await;
			if newest_sent().await?.contains("Forwarded message") {
				return Err("the forward left out still carried the mail".into());
			}
			// A reply closed unsent reopens from Drafts with its quotes apart from the text: as kept
			// here, the whole mail left out included; without that, as read back from the draft.
			mailbox.select_by_subject("Lunch");
			wait_for(|| mailbox.reader.current.borrow().is_some()).await;
			sleep(300).await;
			mailbox.reader.select_in_current("12:30 at the usual place?");
			let reply = mailbox.reader.answer(crate::compose_model::Answer::Reply).ok_or("no reply")?;
			let written = reply.quote();
			reply.type_body("Yes, see you then.");
			reply.window.close();
			wait_for(|| !reply.window.is_visible()).await;
			drop(reply);
			mailbox.select_folder(FolderKind::Drafts);
			wait_for(|| mailbox.select_by_subject("Re: Lunch")).await;
			wait_for(|| mailbox.reader.current.borrow().as_ref().is_some_and(|o| o.mail.summary.draft)).await;
			let again = mailbox.reader.edit_current_draft().ok_or("the draft did not open")?;
			if again.body_text() != "Yes, see you then." || again.quote() != written {
				return Err(format!("the draft reopened as {:?} quoting {:?}, written {written:?}", again.body_text(), again.quote()));
			}
			again.window.set_visible(false);
			drop(again);
			let id = mailbox.reader.current.borrow().as_ref().map(|o| o.mail.summary.id.clone()).ok_or("no draft")?;
			win.ctx.cache().ok_or("no cache")?.set_draft_quote(&id, None).map_err(|e| e.to_string())?;
			let again = mailbox.reader.edit_current_draft().ok_or("the draft did not open")?;
			if again.quote().picked != written.picked || again.quote().whole.is_some() {
				return Err(format!("read back from the draft: {:?}", again.quote()));
			}
			again.window.set_visible(false);
			Ok(())
		}
		// A reply goes out from the address the mail came to; a new mail from the main one, unless
		// another is picked.
		"from" => {
			wait_for(|| mailbox.store.n_items() > 0).await;
			mailbox.select_by_subject("five small things");
			wait_for(|| mailbox.reader.current.borrow().is_some()).await;
			let reply = mailbox.reader.answer(crate::compose_model::Answer::Reply).ok_or("no reply")?;
			let picked = reply.from.selected_item().and_downcast::<gtk::StringObject>().map(|s| s.string().to_string());
			if !reply.from.is_visible() || picked.as_deref() != Some("news@you.example") {
				return Err(format!("the reply is from {picked:?}"));
			}
			reply.send();
			wait_for(|| !reply.window.is_visible()).await;
			drop(reply);
			let fresh = Composer::open(&win.ctx, Fields { to: "mia@tuta.com".into(), subject: "From the main address".into(), ..Default::default() }, None);
			if fresh.from.selected() != 0 {
				return Err("a new mail is not from the main address".into());
			}
			fresh.send();
			wait_for(|| !fresh.window.is_visible()).await;
			drop(fresh);
			mailbox.select_folder(tern_core::FolderKind::Sent);
			wait_for(|| mailbox.select_by_subject("Re: This week")).await;
			wait_for(|| mailbox.reader.current.borrow().is_some()).await;
			let senders: Vec<(String, String)> = mailbox.listed().iter().map(|r| (r.latest.subject.clone(), r.latest.from.address.clone())).collect();
			let sent_from = |subject: &str| senders.iter().find(|(s, _)| s.contains(subject)).map(|(_, f)| f.as_str());
			if sent_from("Re: This week") != Some("news@you.example") || sent_from("From the main address") != Some("you@tuta.com") {
				return Err(format!("Sent holds {senders:?}"));
			}
			Ok(())
		}
		// The sidebar's broom past the limit, and Clean Up clearing everything, which then fetches again.
		"cleanup" => {
			wait_for(|| mailbox.store.n_items() > 0).await;
			sleep(500).await;
			crate::cleanup::check(&win.ctx).await;
			if mailbox.cleanup_offered() {
				return Err("the demo's small cache is offered for cleanup".into());
			}
			win.ctx.offer_cleanup(Some(1_300_000_000));
			if !mailbox.cleanup_offered() {
				return Err("no broom past the limit".into());
			}
			let usage = crate::cleanup::measure(&win.ctx).await.ok_or("not signed in")?;
			let freed = Rc::new(Cell::new(None));
			let f = freed.clone();
			let (dialog, all) = crate::cleanup::dialog(&win.ctx, usage, move |bytes| f.set(Some(bytes)));
			dialog.present(Some(&win.ctx.window));
			let older = format!("_Free {}", gtk::glib::format_size(usage.older));
			if dialog.response_label("clean") != older || dialog.is_response_enabled("clean") != (usage.older > 0) {
				return Err(format!("the button reads {:?} for {older:?}", dialog.response_label("clean")));
			}
			all.set_active(true);
			if dialog.response_label("clean") != "_Delete All Mail" || dialog.response_appearance("clean") != adw::ResponseAppearance::Destructive || !dialog.is_response_enabled("clean") {
				return Err(format!("with All Mail ticked the button reads {:?}", dialog.response_label("clean")));
			}
			dialog.emit_by_name::<()>("response", &[&"clean"]);
			dialog.force_close();
			wait_for(|| freed.get().is_some()).await;
			let cache = win.ctx.cache().ok_or("no cache")?;
			wait_for(|| mailbox.store.n_items() > 0).await;
			let inbox = cache.folders().map_err(|e| e.to_string())?.into_iter().find(|f| f.kind == tern_core::FolderKind::Inbox).ok_or("no inbox")?;
			if cache.count(&inbox.id).map_err(|e| e.to_string())? == 0 {
				return Err("the Inbox did not fetch again".into());
			}
			win.ctx.offer_cleanup(Some(1_300_000_000));
			let (dialog, _) = crate::cleanup::dialog(&win.ctx, crate::cleanup::Usage { total: 1_300_000_000, older: 940_000_000, vectors: 210_000_000, model: 30_000_000 }, |_| {});
			dialog.present(Some(&win.ctx.window));
			Ok(())
		}
		// Typing in To suggests contacts and people written to.
		"suggest" => {
			wait_for(|| mailbox.store.n_items() > 0).await;
			sleep(500).await;
			let composer = Composer::open(&win.ctx, Fields::default(), None);
			sleep(200).await;
			let shown = composer.type_to("Pri");
			if shown == 0 {
				return Err("no suggestions for \"Pri\"".into());
			}
			Ok(())
		}
		// Alt+Down moves to the next conversation; Delete trashes it and opens its neighbour.
		"keys" => {
			wait_for(|| mailbox.store.n_items() == 7).await;
			let window = win.ctx.window.clone();
			let act = |name: &str| gtk::prelude::WidgetExt::activate_action(&window, name, None).map_err(|e| e.to_string());
			act("win.next")?;
			act("win.next")?;
			wait_for(|| mailbox.reader.current.borrow().is_some()).await;
			let second = mailbox.reader.mails().first().map(|m| m.subject.clone()).unwrap_or_default();
			if second != "Your invoice for October" {
				return Err(format!("Alt+Down twice opened {second:?}"));
			}
			act("win.trash")?;
			wait_for(|| mailbox.store.n_items() == 6).await;
			wait_for(|| mailbox.reader.current.borrow().is_some()).await;
			let next = mailbox.reader.mails().first().map(|m| m.subject.clone()).unwrap_or_default();
			if mailbox.store.n_items() != 6 || !next.contains("Lunch") {
				return Err(format!("after Delete: {} conversations, showing {next:?}", mailbox.store.n_items()));
			}
			Ok(())
		}
		// F9 folds the folders to a rail of icons and back; dragged edges keep their width.
		"panes" => {
			// The window's icon on the folders' header, as KDE puts it, must not widen the rail.
			gtk::Settings::default().ok_or("no settings")?.set_gtk_decoration_layout(Some("icon:minimize,maximize,close"));
			wait_for(|| mailbox.store.n_items() > 0).await;
			let window = win.ctx.window.clone();
			let act = |name: &str| gtk::prelude::WidgetExt::activate_action(&window, name, None).map_err(|e| e.to_string());
			mailbox.drag_folders(300);
			mailbox.drag_list(500);
			sleep(300).await;
			if mailbox.pane_widths() != (300, 500) {
				return Err(format!("dragged to 300 and 500, the panes are {:?}", mailbox.pane_widths()));
			}
			act("win.toggle-folders")?;
			sleep(300).await;
			if !mailbox.is_rail() || mailbox.pane_widths().0 != RAIL as i32 {
				return Err(format!("F9 did not make a rail: {:?}", mailbox.pane_widths()));
			}
			let middle = mailbox.header_icon_middle().ok_or("no icon on the rail's header")?;
			if (middle - RAIL as f32 / 2.0).abs() > 0.5 {
				return Err(format!("the rail's icon is centred at {middle}, the rail is {RAIL} wide"));
			}
			act("win.toggle-folders")?;
			sleep(300).await;
			if mailbox.pane_widths().0 != 300 {
				return Err(format!("F9 again left the folders {} wide", mailbox.pane_widths().0));
			}
			// Dragged narrow, it snaps to the rail; the shot shows it.
			mailbox.drag_folders(90);
			sleep(300).await;
			if !mailbox.is_rail() || win.ctx.settings().folders_width != 300 {
				return Err(format!("dragged narrow: rail {}, width kept {}", mailbox.is_rail(), win.ctx.settings().folders_width));
			}
			Ok(())
		}
		// One word says what Tern is doing: fetching (the sync opens mail ahead, slowly here), then
		// opening a slow mail over that, then sending over both.
		"busy" => {
			wait_for(|| win.ctx.activity.word() == Some(Word::Fetching)).await;
			wait_for(|| mailbox.store.n_items() > 0).await;
			mailbox.select_by_subject("Photos from Saturday");
			sleep(800).await;
			if win.ctx.activity.word() != Some(Word::Opening) {
				return Err(format!("while opening, the word is {:?}", win.ctx.activity.word()));
			}
			let fields = Fields { to: "mia@tuta.com".into(), subject: "Route".into(), body: "Here.".into(), ..Default::default() };
			let composer = Composer::open(&win.ctx, fields, None);
			composer.send();
			sleep(300).await;
			match win.ctx.activity.word() {
				Some(Word::Sending) => Ok(()),
				other => Err(format!("while sending and opening, the word is {other:?}")),
			}
		}
		// The mail chosen shows at once though a slow batch fetching the list's mail holds it:
		// batches take seconds here, a single mail none.
		"focus-first" => {
			wait_for(|| mailbox.store.n_items() > 0).await;
			sleep(500).await;
			let started = std::time::Instant::now();
			mailbox.select_by_subject("Lunch");
			wait_for(|| mailbox.reader.current.borrow().is_some()).await;
			let shown = mailbox.reader.current.borrow().as_ref().map(|o| o.mail.summary.subject.clone());
			let batched = tern_core::MailId { list: "demo".into(), element: "m001".into() };
			let batch_done = win.ctx.cache().is_some_and(|c| c.has_mail(&batched).unwrap_or(false));
			if shown.as_deref() != Some("Re: Lunch on Friday?") || started.elapsed() > Duration::from_secs(2) || batch_done {
				return Err(format!("showed {shown:?} after {:?}, the slow batch done: {batch_done}", started.elapsed()));
			}
			Ok(())
		}
		// Arrowing onto an unread conversation marks it read without moving the focus off it.
		"arrows" => {
			wait_for(|| mailbox.store.n_items() > 0).await;
			let unread = (1..mailbox.store.n_items()).find(|&i| mailbox.store.item(i).is_some_and(|o| o.downcast_ref::<glib::BoxedAnyObject>().unwrap().borrow::<tern_cache::ThreadRow>().unread));
			let at = unread.ok_or("no unread conversation below the first")?;
			mailbox.arrow_to(0);
			sleep(200).await;
			mailbox.arrow_to(at);
			wait_for(|| mailbox.focused_row().is_some_and(|(_, unread)| !unread)).await;
			sleep(300).await;
			match mailbox.focused_row() {
				Some((row, false)) if row == at => Ok(()),
				other => Err(format!("arrowed onto row {at}, the focus is now on {other:?}")),
			}
		}
		// Ctrl+F searches the mail on this computer across folders; a result opens its conversation,
		// and Escape goes back to the folder.
		"search" => {
			wait_for(|| mailbox.store.n_items() == 7).await;
			// Body words are found once the background sync has opened the mails ahead.
			wait_for(|| win.ctx.cache().is_some_and(|c| c.missing_bodies(10).is_ok_and(|m| m.is_empty()))).await;
			let window = win.ctx.window.clone();
			let act = |name: &str| gtk::prelude::WidgetExt::activate_action(&window, name, None).map_err(|e| e.to_string());
			let shows = |subjects: &[&str]| mailbox.is_searching() && mailbox.shown() == ("Search".to_string(), subjects.iter().map(|s| s.to_string()).collect());
			act("win.search")?;
			sleep(100).await;
			// A filter; then words from a body, the last one a prefix.
			for (query, expected) in [("from:sam", "Re: Lunch on Friday?"), ("usual pla", "Re: Lunch on Friday?"), ("invoic", "Your invoice for October")] {
				mailbox.type_search(query)?;
				wait_for(|| shows(&[expected])).await;
				if !shows(&[expected]) {
					return Err(format!("searching {query:?} shows {:?}", mailbox.shown()));
				}
			}
			mailbox.select_by_subject("invoice");
			let opened = || mailbox.reader.current.borrow().as_ref().is_some_and(|o| o.mail.summary.subject == "Your invoice for October");
			wait_for(opened).await;
			if !opened() {
				return Err("the result did not open".into());
			}
			// Typing on after opening one: nothing opens by itself (which would mark it read).
			mailbox.type_search("in:inbox")?;
			wait_for(|| shows(&["Photos from Saturday", "Your invoice for October", "Re: Lunch on Friday?", "This week: five small things", "50% off everything this weekend", "Last chance: 50% off ends tonight", "Your exclusive coupon inside"])).await;
			mailbox.select_by_subject("invoice");
			wait_for(opened).await;
			mailbox.append_search(" lunch");
			wait_for(|| shows(&["Re: Lunch on Friday?"])).await;
			sleep(300).await;
			if mailbox.reader.current.borrow().is_some() || !shows(&["Re: Lunch on Friday?"]) {
				return Err(format!("typing on opened {:?}", mailbox.reader.mails().first().map(|m| m.subject.clone())));
			}
			mailbox.type_search("zzqx")?;
			wait_for(|| mailbox.status().is_some()).await;
			if mailbox.status().as_deref() != Some("No Matches") {
				return Err(format!("searching nonsense shows {:?}", mailbox.status()));
			}
			mailbox.escape_search();
			wait_for(|| !mailbox.is_searching() && mailbox.store.n_items() == 7).await;
			if mailbox.is_searching() || mailbox.shown().0 != "Inbox" || mailbox.store.n_items() != 7 {
				return Err(format!("Escape left {:?}", mailbox.shown()));
			}
			// A conversation spanning Inbox and Sent opens whole; the shot shows it.
			act("win.search")?;
			sleep(100).await;
			mailbox.type_search("lunch")?;
			wait_for(|| shows(&["Re: Lunch on Friday?"])).await;
			mailbox.select_by_subject("Lunch");
			wait_for(|| mailbox.reader.mails().len() == 2).await;
			match mailbox.reader.mails().len() {
				2 => Ok(()),
				n => Err(format!("the Lunch conversation opened with {n} mails")),
			}
		}
		// Move to Spam: the line finds the sender's other mail, Select All picks it, and the line
		// is kept as a rule. The moves hold after a sync, so the server has them too.
		"spam" => {
			rules::save(&[]);
			wait_for(|| mailbox.store.n_items() > 0).await;
			mailbox.select_by_subject("coupon");
			wait_for(|| mailbox.reader.thread().is_some()).await;
			let picker = mailbox.spam_conversation().ok_or("Move to Spam did not open")?;
			let ticked = |p: &spam::Picker| p.listed().iter().filter(|(_, t)| *t).count();
			if picker.listed().len() != 3 || ticked(&picker) != 1 || picker.listed()[0].0 != "Your exclusive coupon inside" {
				return Err(format!("the sender's mail listed as {:?}", picker.listed()));
			}
			picker.entry.set_text("(from:@deals.example");
			wait_for(|| picker.listed().len() == 1).await;
			if picker.listed().len() != 1 || picker.confirm.label().as_deref() != Some("Move to Spam") {
				return Err(format!("a broken line listed {:?}", picker.listed()));
			}
			picker.entry.set_text("from:@deals.example");
			wait_for(|| picker.listed().len() == 3).await;
			picker.all.set_active(true);
			picker.keep.set_active(true);
			if ticked(&picker) != 3 || picker.confirm.label().as_deref() != Some("Move 3 to Spam") {
				return Err(format!("Select All ticked {:?}", picker.listed()));
			}
			picker.confirm.emit_clicked();
			let in_spam = || win.ctx.cache().and_then(|c| c.folders().ok()?.into_iter().find(|f| f.kind == FolderKind::Spam).and_then(|f| c.count(&f.id).ok()));
			if in_spam() != Some(3) {
				return Err(format!("{:?} in Spam after moving", in_spam()));
			}
			wait_for(|| win.ctx.activity.word().is_none()).await;
			gtk::prelude::WidgetExt::activate_action(&win.ctx.window, "win.refresh", None).map_err(|e| e.to_string())?;
			sleep(300).await;
			wait_for(|| win.ctx.activity.word().is_none()).await;
			if in_spam() != Some(3) {
				return Err(format!("{:?} in Spam after a sync", in_spam()));
			}
			let rule = Rule { when: "from:@deals.example".into(), action: Action::Spam };
			if rules::load() != [rule] {
				return Err(format!("the rules are {:?}", rules::load()));
			}
			Ok(())
		}
		// A conversation shows newest first. Its menu pins it, marks it read and unread, and opens
		// Move to Spam.
		"menu" => {
			wait_for(|| mailbox.store.n_items() == 7).await;
			mailbox.select_by_subject("Lunch");
			wait_for(|| mailbox.reader.mails().len() == 2).await;
			let mails = mailbox.reader.mails();
			if mails.windows(2).any(|w| w[0].received_ms < w[1].received_ms) {
				return Err(format!("the conversation shows {:?}", mails.iter().map(|m| &m.subject).collect::<Vec<_>>()));
			}
			let labels = |popover: &gtk::PopoverMenu| {
				let mut labels = Vec::new();
				let Some(menu) = popover.menu_model() else { return labels };
				for i in 0..menu.n_items() {
					let Some(section) = menu.item_link(i, "section") else { continue };
					for j in 0..section.n_items() {
						labels.extend(section.item_attribute_value(j, "label", None).and_then(|v| v.get::<String>()));
					}
				}
				popover.popdown();
				labels
			};
			let find = |key: &str| mailbox.listed().into_iter().position(|r| r.key == key).ok_or(format!("{key} is not listed"));

			// A right click on the last conversation pins it.
			let last = mailbox.listed().pop().ok_or("nothing listed")?;
			if !mailbox.right_click_row(mailbox.store.n_items() - 1) {
				return Err("a right click on a conversation opened no menu".into());
			}
			mailbox.row_action("pin")?;
			wait_for(|| mailbox.listed().first().is_some_and(|r| r.key == last.key)).await;
			if !mailbox.listed()[0].pinned {
				return Err("the pinned conversation is not marked pinned".into());
			}

			let unread = mailbox.listed().into_iter().find(|r| r.unread).ok_or("nothing unread")?;
			let menu = mailbox.open_row_menu(find(&unread.key)? as u32, None).ok_or("no menu")?;
			if !labels(&menu).contains(&"Mark as _Read".to_string()) {
				return Err(format!("the menu of an unread conversation: {:?}", labels(&menu)));
			}
			mailbox.row_action("read")?;
			wait_for(|| find(&unread.key).is_ok_and(|i| !mailbox.listed()[i].unread)).await;
			mailbox.open_row_menu(find(&unread.key)? as u32, None).ok_or("no menu")?.popdown();
			mailbox.row_action("unread")?;
			wait_for(|| find(&unread.key).is_ok_and(|i| mailbox.listed()[i].unread)).await;
			wait_for(|| win.ctx.activity.word().is_none()).await;
			let backend = win.ctx.backend().ok_or("signed out")?;
			let inbox = win.ctx.cache().and_then(|c| c.folders().ok()?.into_iter().find(|f| f.kind == FolderKind::Inbox)).ok_or("no Inbox")?;
			let on_server = backend.list(&inbox.id, None, 50).await.map_err(|e| e.to_string())?.mails.into_iter().find(|m| m.id == unread.latest.id).map(|m| m.unread);
			if on_server != Some(true) {
				return Err(format!("read then unread left the server at {on_server:?}"));
			}

			let menu = mailbox.open_row_menu(find(&last.key)? as u32, None).ok_or("no menu")?;
			if labels(&menu) != ["Un_pin", "Mark as _Unread", "Move to _Spam…", "Move to _Trash"] {
				return Err(format!("the menu of a pinned read conversation: {:?}", labels(&menu)));
			}

			let deals = mailbox.listed().into_iter().find(|r| r.latest.subject.contains("coupon")).ok_or("no coupon")?;
			mailbox.open_row_menu(find(&deals.key)? as u32, None).ok_or("no menu")?.popdown();
			mailbox.row_action("spam")?;
			wait_for(|| win.ctx.window.visible_dialog().is_some()).await;
			if let Some(dialog) = win.ctx.window.visible_dialog() {
				dialog.force_close();
			}

			mailbox.open_row_menu(0, None).ok_or("no menu")?.popdown();
			mailbox.row_action("pin")?;
			wait_for(|| mailbox.listed().last().is_some_and(|r| r.key == last.key && !r.pinned)).await;
			// The shot shows the newest mail on top.
			mailbox.select_by_subject("Lunch");
			Ok(())
		}
		// A saved rule that catches more than its page lists: the newest few, a count, and its
		// line shown whole, not squeezed to the height worked out before the page showed.
		"rule-edit" => {
			rules::save(&[Rule { when: "(from:@deals.example or subject:invoice) or not to:sam".into(), action: Action::Move("Archive".into()) }]);
			wait_for(|| mailbox.store.n_items() > 0).await;
			let (dialog, section) = crate::window::preferences_dialog(&win.ctx);
			dialog.present(Some(&win.ctx.window));
			let edit = section.edit(Some(0));
			sleep(1500).await;
			if edit.line_height() < 36 {
				return Err(format!("the saved line shows {} px tall", edit.line_height()));
			}
			if edit.preview() != ("Catches 7 mails here already. Rules only sort new mail.".into(), 4, Some("and 3 more, older".into())) {
				return Err(format!("the preview shows {:?}", edit.preview()));
			}
			Ok(())
		}
		// Rules in Preferences: one added, labelled, then changed to move to a folder and deleted; one
		// labels with a label it makes.
		"rules" => {
			rules::save(&[]);
			wait_for(|| mailbox.store.n_items() > 0).await;
			let (dialog, section) = crate::window::preferences_dialog(&win.ctx);
			dialog.present(Some(&win.ctx.window));
			// What a line catches shows as typed: nothing, one mail, or the newest few and a count.
			let edit = section.edit(None);
			edit.put(crate::rule_editor::Piece::Field("from"));
			edit.set_line(&format!("{}@nowhere.example", edit.line()));
			sleep(300).await;
			let (message, mails, more) = edit.preview();
			if edit.line() != "from:@nowhere.example" || !message.starts_with("No mail here matches") || mails != 0 || more.is_some() {
				return Err(format!("no match shows {message:?} {mails} {more:?} for {:?}", edit.line()));
			}
			edit.set_line("subject:coupon");
			sleep(300).await;
			if edit.preview() != ("Catches 1 mail here already. Rules only sort new mail.".into(), 1, None) {
				return Err(format!("one match shows {:?}", edit.preview()));
			}
			edit.set_line("not zzz");
			sleep(300).await;
			if edit.preview() != ("Catches 7 mails here already. Rules only sort new mail.".into(), 4, Some("and 3 more, older".into())) {
				return Err(format!("many matches show {:?}", edit.preview()));
			}
			// Show All lists them in the main window, in place of the folder.
			edit.show_all();
			wait_for(|| mailbox.shown().0 == "Mail the Rule Catches" && mailbox.store.n_items() == 7).await;
			if mailbox.shown().0 != "Mail the Rule Catches" || mailbox.store.n_items() != 7 || dialog.is_mapped() {
				return Err(format!("Show All shows {:?}, dialog open {}", mailbox.shown(), dialog.is_mapped()));
			}
			gtk::prelude::WidgetExt::activate_action(&win.ctx.window, "win.search", None).map_err(|e| e.to_string())?;
			let (dialog, section) = crate::window::preferences_dialog(&win.ctx);
			dialog.present(Some(&win.ctx.window));
			let edit = section.edit(None);
			edit.set_line("subject:invoice or from:@example.org");
			edit.set_does(2);
			wait_for(|| edit.label.model().is_some_and(|m| m.n_items() == 3)).await;
			edit.label.set_selected(0);
			if !edit.save.is_sensitive() {
				return Err("a whole rule cannot be saved".into());
			}
			edit.save.emit_clicked();
			let labelled = Rule { when: "subject:invoice or from:@example.org".into(), action: Action::Label("Receipts".into()) };
			if rules::load() != [labelled.clone()] {
				return Err(format!("saved {:?}", rules::load()));
			}
			let edit = section.edit(Some(0));
			wait_for(|| edit.label.model().is_some_and(|m| m.n_items() == 3)).await;
			if edit.does() != 2 || edit.label.selected() != 0 || edit.line() != labelled.when {
				return Err("the rule did not open as saved".into());
			}
			edit.set_line("subject:(invoice");
			if edit.save.is_sensitive() {
				return Err("a broken line can be saved".into());
			}
			edit.set_line("subject:invoice");
			edit.set_does(1);
			edit.folder.set_selected(1);
			edit.save.emit_clicked();
			let moved = Rule { when: "subject:invoice".into(), action: Action::Move("Archive".into()) };
			if rules::load() != [moved.clone()] {
				return Err(format!("changed to {:?}", rules::load()));
			}
			let edit = section.edit(None);
			edit.set_line("from:news@example.org");
			edit.set_does(2);
			wait_for(|| edit.label.model().is_some_and(|m| m.n_items() == 3)).await;
			edit.label.set_selected(2);
			if !edit.new_label.is_visible() || edit.save.is_sensitive() {
				return Err("New Label asks for no name, or saves without one".into());
			}
			edit.new_label.set_text("Newsletters");
			edit.save.emit_clicked();
			wait_for(|| rules::load().len() == 2).await;
			let made = Rule { when: "from:news@example.org".into(), action: Action::Label("Newsletters".into()) };
			let labels = win.ctx.backend().ok_or("signed out")?.labels().await.map_err(|e| e.to_string())?;
			if rules::load() != [moved.clone(), made.clone()] || !labels.iter().any(|l| l.name == "Newsletters") {
				return Err(format!("a new label left {:?} with labels {labels:?}", rules::load()));
			}
			// A third rule, then the first deleted; the shot shows what is left.
			let edit = section.edit(None);
			edit.set_line("from:@deals.example");
			edit.save.emit_clicked();
			section.delete(0);
			if rules::load() != [made, Rule { when: "from:@deals.example".into(), action: Action::Spam }] {
				return Err(format!("after deleting, {:?}", rules::load()));
			}
			section.expander.set_expanded(true);
			Ok(())
		}
		_ => Ok(()),
	}
}

pub fn drive(win: Rc<Window>) {
	let Some((path, scene)) = scene() else { return };
	if scene == "sign-in" {
		return sign_in(Rc::downgrade(&win), path);
	}
	if matches!(scene.as_str(), "folders-fail" | "refresh-race" | "sign-out-race" | "draft" | "suggest" | "keys" | "arrows" | "busy" | "panes" | "search" | "focus-first" | "spam" | "rules" | "menu" | "from" | "rule-edit" | "quote" | "cleanup") {
		glib::spawn_future_local(async move {
			if let Err(e) = check(&win, &scene).await {
				// No picture, so the check fails.
				eprintln!("tern: screenshot failed: {e}");
				std::process::exit(1);
			}
			let app = win.ctx.window.application();
			save_and_quit(win.ctx.window.upcast_ref(), &path, app).await;
		});
		return;
	}
	glib::spawn_future_local(async move {
		let mailbox = win.mailbox.clone();
		if scene != "login" {
			wait_for(|| mailbox.store.n_items() > 0).await;
		}
		let subject = match scene.as_str() {
			"reading" | "narrow-reading" => Some("Photos from Saturday"),
			"invoice" => Some("Your invoice for October"),
			"newsletter" => Some("five small things"),
			"lunch" | "reply" => Some("Lunch"),
			"forward" => Some("Photos from Saturday"),
			_ => None,
		};
		if let Some(s) = subject {
			mailbox.select_by_subject(s);
			wait_for(|| mailbox.reader.current.borrow().is_some()).await;
		}
		let answer = match scene.as_str() {
			"reply" => Some(crate::compose_model::Answer::ReplyAll),
			"forward" => Some(crate::compose_model::Answer::Forward),
			_ => None,
		};
		if scene == "reply" {
			// The line answered is picked in the mail first.
			sleep(300).await;
			mailbox.reader.select_in_current("12:30 at the usual place?");
		}
		let target: gtk::Widget = if let Some(composer) = answer.and_then(|a| mailbox.reader.answer(a)) {
			composer.window.clone().upcast()
		} else if scene == "sent" {
			// The whole send path: compose with an attachment, send, find it in Sent.
			let fields = Fields { to: "mia@tuta.com".into(), subject: "Route for Sunday".into(), body: "Here is the route.\n\nSee you there!".into(), ..Default::default() };
			let composer = Composer::open(&win.ctx, fields, None);
			composer.attach(vec![NewAttachment { name: "walk-route.pdf".into(), mime: "application/pdf".into(), data: b"%PDF-1.4 route".to_vec() }]);
			composer.send();
			let w = composer.window.clone();
			wait_for(|| !w.is_visible()).await;
			drop(composer);
			mailbox.select_folder(tern_core::FolderKind::Sent);
			wait_for(|| mailbox.select_by_subject("Route for Sunday")).await;
			wait_for(|| mailbox.reader.current.borrow().as_ref().is_some_and(|o| o.mail.attachments.len() == 1)).await;
			win.ctx.window.clone().upcast()
		} else if scene == "rail" {
			// The window's icon on the folders' header, as KDE puts it.
			if let Some(settings) = gtk::Settings::default() {
				settings.set_gtk_decoration_layout(Some("icon:minimize,maximize,close"));
			}
			let _ = gtk::prelude::WidgetExt::activate_action(&win.ctx.window, "win.toggle-folders", None);
			sleep(300).await;
			win.ctx.window.clone().upcast()
		} else if scene == "pinned" {
			let coupon = mailbox.listed().iter().position(|r| r.latest.subject.contains("coupon")).unwrap_or(0) as u32;
			if let Some(menu) = mailbox.open_row_menu(coupon, None) {
				menu.popdown();
			}
			let _ = mailbox.row_action("pin");
			sleep(300).await;
			win.ctx.window.clone().upcast()
		} else if scene == "spam-dialog" {
			mailbox.select_by_subject("coupon");
			wait_for(|| mailbox.reader.thread().is_some()).await;
			if let Some(picker) = mailbox.spam_conversation() {
				picker.entry.set_text("from:@deals.example");
				wait_for(|| picker.listed().len() == 3).await;
				picker.all.set_active(true);
			}
			win.ctx.window.clone().upcast()

		} else if scene == "compose" {
			let fields = Fields {
				to: "Mia Chen <mia@tuta.com>".into(),
				subject: "Saturday photos".into(),
				body: "Hi Mia,\n\nThanks for the photos! Here are mine from the afternoon, and the map of the walk.\n\nSee you soon,\nYou".into(),
				..Default::default()
			};
			let composer = Composer::open(&win.ctx, fields, None);
			composer.attach(vec![
				NewAttachment { name: "harbour.png".into(), mime: "image/png".into(), data: vec![0; 2_340_000] },
				NewAttachment { name: "walk-route.pdf".into(), mime: "application/pdf".into(), data: vec![0; 184_000] },
			]);
			composer.window.clone().upcast()
		} else {
			win.ctx.window.clone().upcast()
		};
		let app = win.ctx.window.application();
		save_and_quit(&target, &path, app).await;
	});
}

fn save(window: &gtk::Widget, path: &std::path::Path) -> std::result::Result<(), String> {
	// A paintable of the window itself draws nothing; its content draws everything but the frame.
	let widget = &window.first_child().ok_or("empty window")?;
	let (w, h) = (widget.width(), widget.height());
	if std::env::var_os("TERN_SCREENSHOT_DEBUG").is_some() {
		let monitor = gtk::gdk::Display::default().and_then(|d| d.monitors().item(0)).and_downcast::<gtk::gdk::Monitor>().map(|m| m.geometry());
		eprintln!("tern: {w}×{h} on {monitor:?}");
	}
	let paintable = gtk::WidgetPaintable::new(Some(widget));
	let snapshot = gtk::Snapshot::new();
	// The window's own background is not part of its content; paint Adwaita's window colour under it.
	let bg = if adw::StyleManager::default().is_dark() { "#222226" } else { "#fafafb" };
	snapshot.append_color(&gtk::gdk::RGBA::parse(bg).unwrap(), &graphene::Rect::new(0.0, 0.0, w as f32, h as f32));
	paintable.snapshot(&snapshot, w as f64, h as f64);
	let node = snapshot.to_node().ok_or("nothing drawn")?;
	let renderer = widget.native().and_then(|n| n.renderer()).ok_or("no renderer")?;
	let texture = renderer.render_texture(&node, Some(&graphene::Rect::new(0.0, 0.0, w as f32, h as f32)));
	texture.save_to_png(path).map_err(|e| e.to_string())
}

/// The demo account the screenshots are taken from.
pub fn backend() -> Arc<dyn Backend> {
	let scene = scene().map(|(_, s)| s).unwrap_or_default();
	let demo = DemoBackend::new();
	match scene.as_str() {
		"folders-fail" => Arc::new(Staged { demo, folders_fail: true, delay: Duration::ZERO, delay_folders: false, slow_mail: Duration::ZERO, slow_batches: false }),
		"refresh-race" => Arc::new(Staged { demo, folders_fail: false, delay: Duration::from_millis(500), delay_folders: false, slow_mail: Duration::ZERO, slow_batches: false }),
		"sign-out-race" => Arc::new(Staged { demo, folders_fail: false, delay: Duration::from_millis(500), delay_folders: true, slow_mail: Duration::ZERO, slow_batches: false }),
		"busy" => Arc::new(Staged { demo, folders_fail: false, delay: Duration::ZERO, delay_folders: false, slow_mail: Duration::from_secs(3), slow_batches: false }),
		// Batches are slow, single mails quick.
		"focus-first" => Arc::new(Staged { demo, folders_fail: false, delay: Duration::ZERO, delay_folders: false, slow_mail: Duration::from_secs(5), slow_batches: true }),
		_ => Arc::new(demo),
	}
}

/// The demo account with slow or failing calls, for the scenes that check the window copes.
struct Staged {
	demo: DemoBackend,
	folders_fail: bool,
	/// How long `list` (or `folders`, with `delay_folders`) takes.
	delay: Duration,
	delay_folders: bool,
	/// How long opening and sending a mail take.
	slow_mail: Duration,
	/// Only opening several mails at once is slow.
	slow_batches: bool,
}

impl Staged {
	async fn opening(&self, ids: &[tern_core::MailId]) {
		if !self.slow_batches || ids.len() > 1 {
			tokio::time::sleep(self.slow_mail).await;
		}
	}
}

#[async_trait::async_trait]
impl Backend for Staged {
	fn account(&self) -> tern_core::Account {
		self.demo.account()
	}
	async fn folders(&self) -> tern_core::Result<Vec<tern_core::Folder>> {
		if self.folders_fail {
			return Err(tern_core::Error::Offline);
		}
		if self.delay_folders {
			tokio::time::sleep(self.delay).await;
		}
		self.demo.folders().await
	}
	async fn list(&self, folder: &tern_core::FolderId, next: Option<String>, count: usize) -> tern_core::Result<tern_core::MailPage> {
		if !self.delay_folders {
			tokio::time::sleep(self.delay).await;
		}
		self.demo.list(folder, next, count).await
	}
	async fn open(&self, id: &tern_core::MailId) -> tern_core::Result<tern_core::Mail> {
		self.opening(std::slice::from_ref(id)).await;
		self.demo.open(id).await
	}
	async fn open_many(&self, ids: &[tern_core::MailId]) -> tern_core::Result<Vec<(tern_core::MailId, tern_core::Result<tern_core::Mail>)>> {
		self.opening(ids).await;
		self.demo.open_many(ids).await
	}
	async fn attachment(&self, mail: &tern_core::MailId, attachment: &tern_core::Attachment) -> tern_core::Result<Vec<u8>> {
		self.demo.attachment(mail, attachment).await
	}
	async fn send(&self, draft: tern_core::Draft) -> tern_core::Result<()> {
		tokio::time::sleep(self.slow_mail).await;
		self.demo.send(draft).await
	}
	async fn save_draft(&self, draft: tern_core::Draft) -> tern_core::Result<tern_core::MailId> {
		self.demo.save_draft(draft).await
	}
	async fn contacts(&self) -> tern_core::Result<Vec<tern_core::Address>> {
		self.demo.contacts().await
	}
	async fn set_unread(&self, id: &tern_core::MailId, unread: bool) -> tern_core::Result<()> {
		self.demo.set_unread(id, unread).await
	}
	async fn trash(&self, id: &tern_core::MailId) -> tern_core::Result<()> {
		self.demo.trash(id).await
	}
	async fn move_mails(&self, ids: &[tern_core::MailId], folder: &tern_core::FolderId) -> tern_core::Result<()> {
		self.demo.move_mails(ids, folder).await
	}
	async fn labels(&self) -> tern_core::Result<Vec<tern_core::Label>> {
		self.demo.labels().await
	}
	async fn create_label(&self, name: &str) -> tern_core::Result<tern_core::Label> {
		self.demo.create_label(name).await
	}
	async fn add_label(&self, ids: &[tern_core::MailId], label: &tern_core::FolderId) -> tern_core::Result<()> {
		self.demo.add_label(ids, label).await
	}
	async fn sign_out(&self) -> tern_core::Result<()> {
		self.demo.sign_out().await
	}
}

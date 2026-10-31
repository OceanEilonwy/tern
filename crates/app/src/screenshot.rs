//! A hidden debug path that builds a scene, saves a PNG of it and quits.
//!
//! `TERN_SCREENSHOT=/path/out.png TERN_SCREENSHOT_SCENE=login|sign-in|folders-fail|refresh-race|sign-out-race|draft|suggest|keys|arrows|busy|panes|search|focus-first|spam|rules|inbox|reading|invoice|newsletter|compose|spam-dialog|rule-edit|narrow|narrow-reading`
//! (with `TERN_DEMO=1` for all but login and sign-in; sign-in types "demo" on the sign-in page). For dark mode add `ADW_DEBUG_COLOR_SCHEME=prefer-dark`.

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
			let second = mailbox.reader.mails().last().map(|m| m.subject.clone()).unwrap_or_default();
			if second != "Your invoice for October" {
				return Err(format!("Alt+Down twice opened {second:?}"));
			}
			act("win.trash")?;
			wait_for(|| mailbox.store.n_items() == 6).await;
			wait_for(|| mailbox.reader.current.borrow().is_some()).await;
			let next = mailbox.reader.mails().last().map(|m| m.subject.clone()).unwrap_or_default();
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
				return Err(format!("typing on opened {:?}", mailbox.reader.mails().last().map(|m| m.subject.clone())));
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
		// Rules in Preferences: one added, labelled, then changed to move to a folder and deleted.
		"rules" => {
			rules::save(&[]);
			wait_for(|| mailbox.store.n_items() > 0).await;
			let (dialog, section) = crate::window::preferences_dialog(&win.ctx);
			dialog.present(Some(&win.ctx.window));
			let edit = section.edit(None);
			edit.line.set_text("subject:invoice or from:@example.org");
			edit.does.set_selected(2);
			wait_for(|| edit.label.model().is_some_and(|m| m.n_items() == 2)).await;
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
			wait_for(|| edit.label.model().is_some_and(|m| m.n_items() == 2)).await;
			if edit.does.selected() != 2 || edit.label.selected() != 0 || edit.line.text() != labelled.when {
				return Err("the rule did not open as saved".into());
			}
			edit.line.set_text("subject:(invoice");
			if edit.save.is_sensitive() {
				return Err("a broken line can be saved".into());
			}
			edit.line.set_text("subject:invoice");
			edit.does.set_selected(1);
			edit.folder.set_selected(1);
			edit.save.emit_clicked();
			let moved = Rule { when: "subject:invoice".into(), action: Action::Move("Archive".into()) };
			if rules::load() != [moved.clone()] {
				return Err(format!("changed to {:?}", rules::load()));
			}
			// A second rule, then the first deleted; the shot shows what is left.
			let edit = section.edit(None);
			edit.line.set_text("from:@deals.example");
			edit.save.emit_clicked();
			section.delete(0);
			if rules::load() != [Rule { when: "from:@deals.example".into(), action: Action::Spam }] {
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
	if matches!(scene.as_str(), "folders-fail" | "refresh-race" | "sign-out-race" | "draft" | "suggest" | "keys" | "arrows" | "busy" | "panes" | "search" | "focus-first" | "spam" | "rules") {
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
		} else if scene == "spam-dialog" {
			mailbox.select_by_subject("coupon");
			wait_for(|| mailbox.reader.thread().is_some()).await;
			if let Some(picker) = mailbox.spam_conversation() {
				picker.entry.set_text("from:@deals.example");
				wait_for(|| picker.listed().len() == 3).await;
				picker.all.set_active(true);
			}
			win.ctx.window.clone().upcast()
		} else if scene == "rule-edit" {
			let (dialog, section) = crate::window::preferences_dialog(&win.ctx);
			dialog.present(Some(&win.ctx.window));
			let edit = section.edit(None);
			edit.line.set_text("(from:@example.org or subject:invoice) and not lunch");
			edit.does.set_selected(1);
			edit.folder.set_selected(1);
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
	async fn add_label(&self, ids: &[tern_core::MailId], label: &tern_core::FolderId) -> tern_core::Result<()> {
		self.demo.add_label(ids, label).await
	}
	async fn sign_out(&self) -> tern_core::Result<()> {
		self.demo.sign_out().await
	}
}

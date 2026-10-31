//! A hidden debug path that builds a scene, saves a PNG of it and quits.
//!
//! `TERN_SCREENSHOT=/path/out.png TERN_SCREENSHOT_SCENE=login|sign-in|folders-fail|refresh-race|sign-out-race|inbox|reading|invoice|newsletter|compose|narrow|narrow-reading`
//! (with `TERN_DEMO=1` for all but login and sign-in; sign-in types "demo" on the sign-in page). For dark mode add `ADW_DEBUG_COLOR_SCHEME=prefer-dark`.

use std::path::PathBuf;
use std::rc::Rc;
use std::time::Duration;

use std::sync::Arc;

use adw::prelude::*;
use gtk::{glib, graphene};
use tern_core::demo::DemoBackend;
use tern_core::{Backend, NewAttachment};

use crate::compose_model::Fields;
use crate::composer::Composer;
use crate::window::Window;

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
	sleep(1200).await;
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
		_ => Ok(()),
	}
}

pub fn drive(win: Rc<Window>) {
	let Some((path, scene)) = scene() else { return };
	if scene == "sign-in" {
		return sign_in(Rc::downgrade(&win), path);
	}
	if matches!(scene.as_str(), "folders-fail" | "refresh-race" | "sign-out-race") {
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
		"folders-fail" => Arc::new(Staged { demo, folders_fail: true, delay: Duration::ZERO, delay_folders: false }),
		"refresh-race" => Arc::new(Staged { demo, folders_fail: false, delay: Duration::from_millis(500), delay_folders: false }),
		"sign-out-race" => Arc::new(Staged { demo, folders_fail: false, delay: Duration::from_millis(500), delay_folders: true }),
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
		self.demo.open(id).await
	}
	async fn attachment(&self, mail: &tern_core::MailId, attachment: &tern_core::Attachment) -> tern_core::Result<Vec<u8>> {
		self.demo.attachment(mail, attachment).await
	}
	async fn send(&self, draft: tern_core::Draft) -> tern_core::Result<()> {
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
	async fn sign_out(&self) -> tern_core::Result<()> {
		self.demo.sign_out().await
	}
}

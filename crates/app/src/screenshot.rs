//! A hidden debug path that builds a scene, saves a PNG of it and quits.
//!
//! `TERN_SCREENSHOT=/path/out.png TERN_SCREENSHOT_SCENE=login|inbox|reading|invoice|newsletter|compose|narrow|narrow-reading`
//! (with `TERN_DEMO=1` for all but login). For dark mode add `ADW_DEBUG_COLOR_SCHEME=prefer-dark`.

use std::path::PathBuf;
use std::rc::Rc;
use std::time::Duration;

use std::sync::Arc;

use adw::prelude::*;
use async_trait::async_trait;
use gtk::{glib, graphene};
use tern_core::demo::DemoBackend;
use tern_core::{Account, Attachment, Backend, Draft, Folder, FolderId, Mail, MailId, MailPage, NewAttachment, Result};

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

pub fn drive(win: Rc<Window>) {
	let Some((path, scene)) = scene() else { return };
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
		sleep(1200).await;
		// Broadway draws only while a client watches; try until a frame has been drawn.
		let mut result: std::result::Result<(), String> = Err(String::new());
		for _ in 0..25 {
			target.queue_draw();
			sleep(200).await;
			result = save(&target, &path);
			if result.is_ok() {
				break;
			}
		}
		if let Err(e) = result {
			eprintln!("tern: screenshot failed: {e}");
		}
		if let Some(app) = win.ctx.window.application() {
			app.quit();
		}
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

/// The demo account, but with a real photo behind the "Photos from Saturday" mail's tiny inline PNG,
/// so the screenshot shows what an inline image looks like.
pub fn backend() -> Arc<dyn Backend> {
	Arc::new(Shot(DemoBackend::new()))
}

struct Shot(DemoBackend);

fn sunset_png() -> Vec<u8> {
	let (w, h) = (960u32, 600u32);
	let img = image::RgbImage::from_fn(w, h, |x, y| {
		let t = y as f32 / h as f32;
		let horizon = 0.62;
		let (dx, dy) = (x as f32 - w as f32 * 0.5, y as f32 - h as f32 * horizon);
		let sun = (dx * dx + dy * dy).sqrt() < 70.0 && t < horizon;
		let px = if sun {
			[255, 214, 120]
		} else if t < horizon {
			let k = t / horizon;
			[(80.0 + 170.0 * k) as u8, (60.0 + 80.0 * k) as u8, (140.0 - 60.0 * k) as u8]
		} else {
			let k = (t - horizon) / (1.0 - horizon);
			let glint = if dx.abs() < 60.0 * (1.0 - k) && (y / 6) % 2 == 0 { 60.0 } else { 0.0 };
			[(40.0 + glint * 2.0) as u8, (60.0 + glint * 1.5) as u8, (110.0 - 40.0 * k + glint) as u8]
		};
		image::Rgb(px)
	});
	let mut out = std::io::Cursor::new(Vec::new());
	img.write_to(&mut out, image::ImageFormat::Png).expect("encode");
	out.into_inner()
}

#[async_trait]
impl Backend for Shot {
	fn account(&self) -> Account {
		self.0.account()
	}
	async fn folders(&self) -> Result<Vec<Folder>> {
		self.0.folders().await
	}
	async fn list(&self, folder: &FolderId, next: Option<String>, count: usize) -> Result<MailPage> {
		self.0.list(folder, next, count).await
	}
	async fn open(&self, id: &MailId) -> Result<Mail> {
		self.0.open(id).await
	}
	async fn attachment(&self, mail: &MailId, attachment: &Attachment) -> Result<Vec<u8>> {
		if attachment.cid.as_deref() == Some("sunset") {
			return Ok(sunset_png());
		}
		self.0.attachment(mail, attachment).await
	}
	async fn send(&self, draft: Draft) -> Result<()> {
		self.0.send(draft).await
	}
	async fn set_unread(&self, id: &MailId, unread: bool) -> Result<()> {
		self.0.set_unread(id, unread).await
	}
	async fn trash(&self, id: &MailId) -> Result<()> {
		self.0.trash(id).await
	}
	async fn sign_out(&self) -> Result<()> {
		self.0.sign_out().await
	}
}

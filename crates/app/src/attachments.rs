//! Opening and saving a received mail's attachments.

use std::io::Write;
use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt};
use std::path::PathBuf;

use adw::prelude::*;
use gtk::{gio, glib};
use tern_core::{Attachment, Error, MailId};

use crate::compose_model::safe_file_name;
use crate::{format, runtime, window::Ctx};

/// A symbolic icon for a MIME type.
pub fn icon_for(mime: &str) -> gio::Icon {
	gio::content_type_get_symbolic_icon(mime)
}

/// A button showing an attachment, with a save button beside it.
pub fn chip(ctx: &Ctx, mail: &MailId, att: &Attachment) -> gtk::Widget {
	let icon = gtk::Image::from_gicon(&icon_for(&att.mime));
	icon.set_pixel_size(24);
	let name = gtk::Label::builder().label(&att.name).xalign(0.0).ellipsize(gtk::pango::EllipsizeMode::Middle).max_width_chars(28).build();
	let size = gtk::Label::builder().label(format::size(att.size)).xalign(0.0).css_classes(["caption", "dim-label"]).build();
	let text = gtk::Box::new(gtk::Orientation::Vertical, 0);
	text.append(&name);
	text.append(&size);
	let inner = gtk::Box::new(gtk::Orientation::Horizontal, 10);
	inner.append(&icon);
	inner.append(&text);
	let open = gtk::Button::builder().child(&inner).tooltip_text(format!("Open {}", att.name)).build();
	let save = gtk::Button::builder().icon_name("document-save-symbolic").tooltip_text(format!("Save {}…", att.name)).valign(gtk::Align::Fill).build();
	let chip = gtk::Box::builder().css_classes(["linked"]).build();
	chip.append(&open);
	chip.append(&save);

	let (c, m, a) = (ctx.clone(), mail.clone(), att.clone());
	open.connect_clicked(move |b| open_attachment(&c, &m, &a, b));
	let (c, m, a) = (ctx.clone(), mail.clone(), att.clone());
	save.connect_clicked(move |_| save_attachment(&c, &m, &a));
	chip.upcast()
}

async fn download(ctx: &Ctx, mail: &MailId, att: &Attachment) -> Result<Vec<u8>, Error> {
	let backend = ctx.backend().ok_or(Error::SessionExpired)?;
	let (mail, att) = (mail.clone(), att.clone());
	runtime::run(async move { backend.attachment(&mail, &att).await }).await
}

/// A fresh private directory under the user cache for one opened file.
fn private_path(name: &str) -> std::io::Result<PathBuf> {
	let base = glib::user_cache_dir().join("tern").join("open");
	std::fs::DirBuilder::new().recursive(true).mode(0o700).create(&base)?;
	let dir = base.join(format!("{:x}", glib::random_int() as u64 ^ (glib::real_time() as u64)));
	std::fs::DirBuilder::new().mode(0o700).create(&dir)?;
	Ok(dir.join(safe_file_name(name)))
}

fn write_private(name: &str, data: &[u8]) -> std::io::Result<PathBuf> {
	let path = private_path(name)?;
	let mut f = std::fs::OpenOptions::new().write(true).create_new(true).mode(0o600).open(&path)?;
	f.write_all(data)?;
	Ok(path)
}

fn open_attachment(ctx: &Ctx, mail: &MailId, att: &Attachment, button: &gtk::Button) {
	let (ctx, mail, att, button) = (ctx.clone(), mail.clone(), att.clone(), button.clone());
	button.set_sensitive(false);
	runtime::spawn_local(async move {
		let result = download(&ctx, &mail, &att).await;
		button.set_sensitive(true);
		let data = match result {
			Ok(d) => d,
			Err(e) => return ctx.error(&e),
		};
		let name = att.name.clone();
		let path = match runtime::run(async move { write_private(&name, &data) }).await {
			Ok(p) => p,
			Err(e) => return ctx.toast(&format!("Could not open {}: {e}", att.name)),
		};
		let launcher = gtk::FileLauncher::new(Some(&gio::File::for_path(&path)));
		if let Err(e) = launcher.launch_future(Some(&ctx.window)).await {
			if !e.matches(gtk::DialogError::Dismissed) && !e.matches(gtk::DialogError::Cancelled) {
				ctx.toast(&format!("No app could open {}", att.name));
			}
		}
	});
}

fn save_attachment(ctx: &Ctx, mail: &MailId, att: &Attachment) {
	let (ctx, mail, att) = (ctx.clone(), mail.clone(), att.clone());
	runtime::spawn_local(async move {
		let dialog = gtk::FileDialog::builder().title("Save Attachment").initial_name(safe_file_name(&att.name)).modal(true).build();
		if let Some(dir) = glib::user_special_dir(glib::UserDirectory::Downloads) {
			dialog.set_initial_folder(Some(&gio::File::for_path(dir)));
		}
		let Ok(file) = dialog.save_future(Some(&ctx.window)).await else { return };
		let data = match download(&ctx, &mail, &att).await {
			Ok(d) => d,
			Err(e) => return ctx.error(&e),
		};
		match file.replace_contents_future(data, None, false, gio::FileCreateFlags::REPLACE_DESTINATION).await {
			Ok(_) => ctx.toast(&format!("Saved {}", att.name)),
			Err((_, e)) => ctx.toast(&format!("Could not save {}: {}", att.name, e.message())),
		}
	});
}

/// Removes files opened more than a day ago from the private cache directory.
pub fn clean_old_files() {
	let base = glib::user_cache_dir().join("tern").join("open");
	runtime::spawn_local(async move {
		runtime::run(async move {
			let Ok(dirs) = std::fs::read_dir(&base) else { return };
			let day = std::time::Duration::from_secs(24 * 3600);
			for dir in dirs.flatten() {
				let old = dir.metadata().and_then(|m| m.modified()).ok().and_then(|t| t.elapsed().ok()).is_some_and(|age| age > day);
				if old {
					let _ = std::fs::remove_dir_all(dir.path());
				}
			}
		})
		.await
	});
}

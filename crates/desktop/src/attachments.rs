//! A received mail's attachments: a button to open each and one to save it. Fetching is the core's;
//! handing the file to another app, or asking where to save it, is this platform's.

use adw::prelude::*;
use gtk::gio;
use tern_app::view::{Action, AttachmentView, MailRef};
use tern_compose::safe_file_name;

use crate::{runtime, window::Ctx};

/// A symbolic icon for a MIME type.
pub fn icon_for(mime: &str) -> gio::Icon {
	gio::content_type_get_symbolic_icon(mime)
}

/// A button showing an attachment, with a save button beside it.
pub fn chip(ctx: &Ctx, mail: &MailRef, att: &AttachmentView) -> gtk::Widget {
	let icon = gtk::Image::from_gicon(&icon_for(&att.mime));
	icon.set_pixel_size(24);
	let name = gtk::Label::builder().label(&att.name).xalign(0.0).ellipsize(gtk::pango::EllipsizeMode::Middle).max_width_chars(28).build();
	let size = gtk::Label::builder().label(&att.size).xalign(0.0).css_classes(["caption", "dim-label"]).build();
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

	let (engine, m, a) = (ctx.engine.clone(), mail.clone(), att.id.clone());
	open.connect_clicked(move |_| engine.send(Action::OpenAttachment { mail: m.clone(), attachment: a.clone() }));
	let (engine, m, a) = (ctx.engine.clone(), mail.clone(), att.id.clone());
	save.connect_clicked(move |_| engine.send(Action::SaveAttachment { mail: m.clone(), attachment: a.clone() }));
	chip.upcast()
}

/// Opens a file the core has fetched in the app that handles it.
pub fn open_file(ctx: &Ctx, path: &str) {
	if crate::screenshot::scene().is_some() {
		return ctx.handed_over.borrow_mut().push(path.to_string());
	}
	let (ctx, file) = (ctx.clone(), gio::File::for_path(path));
	let name = file.basename().map(|n| n.display().to_string()).unwrap_or_default();
	runtime::spawn_local(async move {
		let launcher = gtk::FileLauncher::new(Some(&file));
		if let Err(e) = launcher.launch_future(Some(&ctx.window)).await {
			if !e.matches(gtk::DialogError::Dismissed) && !e.matches(gtk::DialogError::Cancelled) {
				ctx.toast(&format!("No app could open {name}"));
			}
		}
	});
}

/// Asks where to keep a file the core has fetched, and copies it there.
pub fn save_file(ctx: &Ctx, path: &str, name: &str) {
	let (ctx, from, name) = (ctx.clone(), gio::File::for_path(path), name.to_string());
	runtime::spawn_local(async move {
		let to = if crate::screenshot::scene().is_some() {
			// No one is there to answer a dialog.
			gio::File::for_path(crate::paths().config_dir().join(safe_file_name(&name)))
		} else {
			let dialog = gtk::FileDialog::builder().title("Save Attachment").initial_name(safe_file_name(&name)).modal(true).build();
			if let Some(dir) = gtk::glib::user_special_dir(gtk::glib::UserDirectory::Downloads) {
				dialog.set_initial_folder(Some(&gio::File::for_path(dir)));
			}
			let Ok(to) = dialog.save_future(Some(&ctx.window)).await else { return };
			to
		};
		match from.copy_future(&to, gio::FileCopyFlags::OVERWRITE, gtk::glib::Priority::DEFAULT).0.await {
			Ok(()) => ctx.toast(&format!("Saved {name}")),
			Err(e) => ctx.toast(&format!("Could not save {name}: {}", e.message())),
		}
	});
}

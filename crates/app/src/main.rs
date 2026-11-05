//! Tern, a small GNOME mail client for Tuta.

// async_trait marks the boxed futures it returns #[must_use], which clippy on stable Rust 1.99 calls doubled.
#![allow(clippy::double_must_use)]

mod activity;
mod attachments;
mod background;
mod cleanup;
mod composer;
mod default_app;
mod format;
mod grip;
mod keyring;
mod login;
mod mailbox;
mod reader;
mod render;
mod rule_editor;
mod runtime;
mod screenshot;
mod search;
mod spam;
mod suggest;
mod window;

use std::cell::RefCell;
use std::rc::{Rc, Weak};

use adw::prelude::*;
use gtk::{gio, glib};
use tern_engine::{compose_model, fetcher, html_doc, rules, semantic, settings};

pub const APP_ID: &str = "xyz.hollingworth.Tern";

/// Desktops such as KDE Plasma write GTK's old `gtk-application-prefer-dark-theme` into
/// settings.ini, and libadwaita warns about it at every start. Libadwaita takes the light or dark
/// choice from the desktop's colour scheme instead, so clearing the old setting before libadwaita
/// starts changes nothing but the warning.
fn quiet_old_dark_setting() {
	if gtk::init().is_ok() {
		if let Some(settings) = gtk::Settings::default() {
			settings.set_property("gtk-application-prefer-dark-theme", false);
		}
	}
}

/// The main window, opened now if it is not open.
fn main_window(app: &adw::Application, slot: &RefCell<Weak<window::Window>>) -> Rc<window::Window> {
	if let Some(win) = slot.borrow().upgrade() {
		return win;
	}
	let win = window::Window::new(app);
	*slot.borrow_mut() = Rc::downgrade(&win);
	attachments::clean_old_files();
	win
}

/// What to hand GApplication: Tern takes no arguments of its own, only the mailto links the desktop
/// gives it.
fn command_line() -> Vec<String> {
	let links = std::env::args_os().skip(1).filter_map(|arg| arg.into_string().ok()).filter(|arg| compose_model::mailto_fields(arg).is_some());
	std::iter::once("tern".to_string()).chain(links).collect()
}

fn main() -> glib::ExitCode {
	gio::resources_register_include!("tern.gresource").expect("register resources");
	glib::set_application_name("Tern");
	quiet_old_dark_setting();

	let app = adw::Application::builder().application_id(APP_ID).resource_base_path("/xyz/hollingworth/Tern").flags(gio::ApplicationFlags::HANDLES_OPEN).build();
	if screenshot::scene().is_some() {
		// Screenshot runs must not hand off to a Tern that is already open.
		app.set_flags(app.flags() | gio::ApplicationFlags::NON_UNIQUE);
	}
	app.connect_startup(|app| {
		gtk::Window::set_default_icon_name(APP_ID);
		app.set_accels_for_action("app.quit", &["<Control>q"]);
		app.set_accels_for_action("app.shortcuts", &["<Control>question"]);
		app.set_accels_for_action("win.compose", &["<Control>n"]);
		app.set_accels_for_action("win.reply", &["<Control>r"]);
		app.set_accels_for_action("win.reply-all", &["<Control><Shift>r"]);
		app.set_accels_for_action("win.forward", &["<Control>l"]);
		app.set_accels_for_action("win.trash", &["Delete"]);
		app.set_accels_for_action("win.spam", &["<Control>j"]);
		app.set_accels_for_action("win.refresh", &["F5"]);
		app.set_accels_for_action("win.mark-unread", &["<Control>u"]);
		app.set_accels_for_action("win.next", &["<Alt>Down"]);
		app.set_accels_for_action("win.previous", &["<Alt>Up"]);
		app.set_accels_for_action("win.toggle-folders", &["F9"]);
		app.set_accels_for_action("win.search", &["<Control>f"]);
		app.set_accels_for_action("app.preferences", &["<Control>comma"]);
		for i in 0..9 {
			app.set_accels_for_action(&format!("win.folder({i})"), &[&format!("<Control>{}", i + 1)]);
		}
		let quit = gio::ActionEntry::builder("quit").activate(|app: &adw::Application, _, _| app.quit()).build();
		app.add_action_entries([quit]);
	});
	let main = Rc::new(RefCell::new(Weak::new()));
	app.connect_activate({
		let main = main.clone();
		move |app| {
			if let Some(w) = app.active_window() {
				w.present();
				return;
			}
			main_window(app, &main);
		}
	});
	// A mailto link, from another program or from a second `tern` passing it on to this one.
	app.connect_open(move |app, files, _| {
		let win = main_window(app, &main);
		for fields in files.iter().filter_map(|file| compose_model::mailto_fields(&file.uri())) {
			win.compose(fields);
		}
	});
	app.run_with_args(&command_line())
}

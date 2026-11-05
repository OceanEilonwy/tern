//! Being the desktop's mail app, so that email links other programs open come to Tern.

use adw::prelude::*;
use gtk::gio;

const MAILTO: &str = "x-scheme-handler/mailto";

/// In a Flatpak GIO reads and writes the sandbox's own settings, which the desktop never sees.
fn in_flatpak() -> bool {
	std::path::Path::new("/.flatpak-info").exists()
}

fn is_tern(app: &gio::AppInfo) -> bool {
	app.id().is_some_and(|id| id.strip_suffix(".desktop") == Some(crate::APP_ID))
}

/// Tern's installed menu entry, which says it takes email links.
fn installed() -> Option<gio::AppInfo> {
	gio::AppInfo::all_for_type(MAILTO).into_iter().find(is_tern)
}

/// Says who opens email links now, and offers the button when Tern could.
fn show(row: &adw::ActionRow, button: &gtk::Button) {
	let current = gio::AppInfo::default_for_type(MAILTO, false);
	let is_default = current.as_ref().is_some_and(is_tern);
	let can_set = !is_default && installed().is_some();
	button.set_visible(can_set);
	row.set_subtitle(&if is_default {
		"Email links open in Tern.".to_string()
	} else if !can_set {
		"Install Tern to open email links with it.".to_string()
	} else if let Some(app) = current {
		format!("Email links open in {}.", app.display_name())
	} else {
		"No app opens email links.".to_string()
	});
}

/// Whether email links open in Tern, with a button to make them.
pub fn row() -> (adw::ActionRow, gtk::Button) {
	let row = adw::ActionRow::builder().title("Email Links").build();
	let button = gtk::Button::builder().label("Use Tern").valign(gtk::Align::Center).build();
	row.add_suffix(&button);
	if in_flatpak() {
		row.set_subtitle("To open email links in Tern, choose it as the mail app in your system’s Default Applications settings.");
		button.set_visible(false);
		return (row, button);
	}
	show(&row, &button);
	let this = row.clone();
	button.connect_clicked(move |button| {
		let result = installed().map(|app| app.set_as_default_for_type(MAILTO));
		show(&this, button);
		if let Some(Err(e)) = result {
			this.set_subtitle(&format!("Could not make Tern the default: {e}"));
		} else if button.is_visible() {
			// A list only for this desktop can outrank the one GIO writes.
			this.set_subtitle("Your system kept its choice. Choose Tern as the mail app in its Default Applications settings.");
		}
	});
	(row, button)
}

pub fn group() -> adw::PreferencesGroup {
	let group = adw::PreferencesGroup::new();
	group.add(&row().0);
	group
}

//! The search bar over the conversation list. Search looks only at mail on this computer, and running
//! it is the core's; results replace the list until Escape.

use adw::prelude::*;

/// The search button for the list's header, and the bar it opens under it.
pub fn widgets() -> (gtk::ToggleButton, gtk::SearchBar, gtk::SearchEntry) {
	let entry = gtk::SearchEntry::builder().placeholder_text("Search mail").hexpand(true).build();
	entry.update_property(&[gtk::accessible::Property::Label("Search mail")]);
	entry.set_tooltip_text(Some("Words, or from: to: subject: in:folder has:attachment"));
	let bar = gtk::SearchBar::builder().child(&entry).show_close_button(false).build();
	bar.connect_entry(&entry);
	let b = bar.clone();
	entry.connect_stop_search(move |_| b.set_search_mode(false));
	let button = gtk::ToggleButton::builder().icon_name("system-search-symbolic").tooltip_text("Search (Ctrl+F)").build();
	button.bind_property("active", &bar, "search-mode-enabled").sync_create().bidirectional().build();
	(button, bar, entry)
}

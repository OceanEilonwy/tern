//! The search bar over the conversation list, and running a search on the cache.
//!
//! Search looks only at mail on this computer: words anywhere in the cached mail, and with search
//! by meaning turned on, mails about what was typed. Results replace the list until Escape.

use std::collections::HashMap;
use std::sync::Arc;

use adw::prelude::*;
use tern_cache::search::Query;
use tern_cache::{Cache, SearchHit, ThreadRow};
use tern_core::FolderId;

use crate::{runtime, semantic};

/// Conversations a search shows at most.
const RESULTS: usize = 100;

/// The search on show instead of a folder.
pub struct Searching {
	pub query: Query,
	/// A rule's line shown from its page: the mail it catches, instead of what the words find.
	pub rule: Option<tern_core::rules::Expr>,
	/// The folder each conversation found opens in: its best match's.
	pub folders: HashMap<String, FolderId>,
}

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

/// The conversations with mail `rule` catches, off the UI thread.
pub async fn find_rule(cache: Arc<Cache>, rule: tern_core::rules::Expr) -> (Vec<ThreadRow>, HashMap<String, FolderId>) {
	let hits = runtime::run(async move {
		cache.rule_hits(&rule, RESULTS).unwrap_or_else(|e| {
			eprintln!("tern: rule search failed: {e}");
			Vec::new()
		})
	})
	.await;
	let folders = hits.iter().map(|h| (h.row.key.clone(), h.folder.clone())).collect();
	(hits.into_iter().map(|h| h.row).collect(), folders)
}

/// Runs `query` on the cache off the UI thread; by meaning too when `semantic` is on and the model is loaded.
pub async fn find(cache: Arc<Cache>, query: Query, semantic: bool) -> (Vec<ThreadRow>, HashMap<String, FolderId>) {
	let model = if semantic { semantic::loaded() } else { None };
	let hits: Vec<SearchHit> = runtime::run(async move {
		let vector = model.map(|m| m.embed(&query.text()));
		cache.search(&query, vector.as_deref().map(|v| (semantic::NAME, v)), RESULTS).unwrap_or_else(|e| {
			eprintln!("tern: search failed: {e}");
			Vec::new()
		})
	})
	.await;
	let folders = hits.iter().map(|h| (h.row.key.clone(), h.folder.clone())).collect();
	(hits.into_iter().map(|h| h.row).collect(), folders)
}

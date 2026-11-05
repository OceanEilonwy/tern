//! The whole app against the demo account, as the screen sees it: actions in, views out.

use std::collections::HashMap;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use crate::paths::Paths;
use crate::settings::Settings;

use super::*;

#[derive(Default)]
struct Seen {
	shell: Option<ShellView>,
	signin: Option<SignInView>,
	list: Option<ListView>,
	conversation: Option<ConversationView>,
	composer: Option<ComposerView>,
	composers: Vec<ComposerView>,
	search: Option<SearchView>,
	settings: Option<SettingsView>,
	rules: Option<RulesView>,
	rule_editor: Option<RuleEditorView>,
	sheet: Option<SheetView>,
	effects: Vec<Effect>,
	updates: u32,
}

struct Screen_(Arc<Mutex<Seen>>);

impl EngineListener for Screen_ {
	fn on_update(&self, update: Update) {
		let mut seen = self.0.lock().unwrap();
		seen.updates += 1;
		match update {
			Update::Shell { view } => seen.shell = Some(view),
			Update::SignIn { view } => seen.signin = Some(view),
			Update::List { view } => seen.list = Some(view),
			Update::Conversation { view } => seen.conversation = view,
			Update::Composer { view } => seen.composer = view,
			Update::Composers { view } => seen.composers = view.open,
			Update::Search { view } => seen.search = Some(view),
			Update::Settings { view } => seen.settings = Some(view),
			Update::Rules { view } => seen.rules = Some(view),
			Update::RuleEditor { view } => seen.rule_editor = view,
			Update::Sheet { view } => seen.sheet = view,
		}
	}
	fn on_effect(&self, effect: Effect) {
		self.0.lock().unwrap().effects.push(effect);
	}
	fn on_log(&self, _: String) {}
}

#[derive(Default)]
struct Secrets(Mutex<HashMap<String, Vec<u8>>>);

impl SecretStore for Secrets {
	fn get(&self, name: String) -> Result<Option<Vec<u8>>, SecretError> {
		Ok(self.0.lock().unwrap().get(&name).cloned())
	}
	fn put(&self, name: String, value: Vec<u8>) -> Result<(), SecretError> {
		self.0.lock().unwrap().insert(name, value);
		Ok(())
	}
	fn remove(&self, name: String) {
		self.0.lock().unwrap().remove(&name);
	}
}

struct Harness {
	engine: Arc<Engine>,
	seen: Arc<Mutex<Seen>>,
	dir: std::path::PathBuf,
	paths: Paths,
}

impl Drop for Harness {
	fn drop(&mut self) {
		let _ = std::fs::remove_dir_all(&self.dir);
	}
}

fn harness() -> Harness {
	harness_with(Arc::default())
}

fn harness_with(secrets: Arc<Secrets>) -> Harness {
	static NEXT: AtomicU32 = AtomicU32::new(0);
	let dir = std::env::temp_dir().join(format!("tern-mobile-test-{}-{}", std::process::id(), NEXT.fetch_add(1, Ordering::Relaxed)));
	std::fs::create_dir_all(&dir).unwrap();
	let paths = Paths::new(&dir, &dir);
	let seen: Arc<Mutex<Seen>> = Arc::default();
	let engine = Engine::open_with(paths.clone(), Options::default(), secrets, Arc::new(Screen_(seen.clone())));
	Harness { engine, seen, dir, paths }
}

impl Harness {
	fn send(&self, action: Action) {
		self.engine.send(action);
	}

	/// Waits for `ready` to hold of what the screen has been told.
	fn until(&self, what: &str, ready: impl Fn(&Seen) -> bool) {
		let start = Instant::now();
		loop {
			if ready(&self.seen.lock().unwrap()) {
				return;
			}
			if start.elapsed() > Duration::from_secs(10) {
				let seen = self.seen.lock().unwrap();
				panic!("timed out waiting for {what}\nshell {:?}\nlist {:?}", seen.shell, seen.list.as_ref().map(|l| (l.state, l.rows.len())));
			}
			std::thread::sleep(Duration::from_millis(5));
		}
	}

	fn with<T>(&self, f: impl FnOnce(&Seen) -> T) -> T {
		f(&self.seen.lock().unwrap())
	}

	fn top(&self) -> Option<Screen> {
		self.with(|s| s.shell.as_ref().and_then(|v| v.stack.last().copied()))
	}

	fn row(&self, subject: &str) -> RowView {
		self.with(|s| s.list.as_ref().and_then(|l| l.rows.iter().find(|r| r.subject == subject).cloned()).unwrap_or_else(|| panic!("no row {subject}")))
	}

	fn until_screen(&self, screen: Screen) {
		self.until(&format!("{screen:?}"), |s| s.shell.as_ref().and_then(|v| v.stack.last()) == Some(&screen));
	}

	fn until_row(&self, subject: &str) {
		self.until(&format!("the row {subject}"), |s| s.list.as_ref().is_some_and(|l| l.rows.iter().any(|r| r.subject == subject)));
	}

	/// Signed in to the demo account with its inbox listed.
	fn demo(&self) {
		self.until_screen(Screen::SignIn);
		self.send(Action::SignIn { email: "demo".into(), password: String::new() });
		self.until_row("Photos from Saturday");
	}

	fn open(&self, subject: &str) {
		let row = self.row(subject);
		self.send(Action::OpenThread { key: row.key, folder_id: row.folder_id });
		self.until("a card opened", |s| s.conversation.as_ref().is_some_and(|c| c.cards.iter().any(|c| c.expanded && !c.blocks.is_empty())));
	}

	fn open_folder(&self, name: &str) {
		let id = self.with(|s| s.list.as_ref().unwrap().folders.iter().find(|f| f.name == name).unwrap().id.clone());
		self.send(Action::OpenFolder { id });
		self.until(&format!("the folder {name}"), |s| s.list.as_ref().is_some_and(|l| l.title == name));
	}
}

fn text(to: &str, subject: &str, body: &str) -> ComposeText {
	ComposeText { to: to.into(), cc: String::new(), bcc: String::new(), subject: subject.into(), body: body.into(), from_index: 0 }
}

#[test]
fn starts_at_sign_in_and_the_demo_opens_the_inbox() {
	let h = harness();
	h.until_screen(Screen::SignIn);
	h.send(Action::SignIn { email: "someone@tuta.com".into(), password: String::new() });
	h.until("asking for the password", |s| s.signin.as_ref().is_some_and(|v| v.error.is_some() && !v.busy));
	h.send(Action::SignIn { email: " DEMO ".into(), password: String::new() });
	h.until_row("Photos from Saturday");
	assert_eq!(h.top(), Some(Screen::Inbox));
	h.with(|s| {
		let list = s.list.as_ref().unwrap();
		assert_eq!(list.title, "Inbox");
		assert_eq!(list.state, ListState::Rows);
		assert!(list.folders.iter().any(|f| f.selected && f.icon == FolderIcon::Inbox));
		assert_eq!(s.shell.as_ref().unwrap().address, "you@tuta.com");
	});
}

#[test]
fn the_header_says_the_unread_count_once_the_work_is_done() {
	let h = harness();
	h.demo();
	h.until("the unread count", |s| s.shell.as_ref().is_some_and(|v| v.status.kind == ConnectionKind::Connected && v.status.line.ends_with("unread")));
}

#[test]
fn reading_marks_read_and_back_returns_with_the_row_updated() {
	let h = harness();
	h.demo();
	h.until("the unread row", |s| s.list.as_ref().unwrap().rows.iter().any(|r| r.unread));
	let row = h.with(|s| s.list.as_ref().unwrap().rows.iter().find(|r| r.unread).cloned().unwrap());
	h.send(Action::OpenThread { key: row.key.clone(), folder_id: row.folder_id.clone() });
	h.until_screen(Screen::Conversation);
	h.until("the body", |s| s.conversation.as_ref().is_some_and(|c| c.cards.iter().any(|c| !c.blocks.is_empty())));
	h.until("the row read", |s| s.list.as_ref().unwrap().rows.iter().find(|r| r.key == row.key).is_some_and(|r| !r.unread && r.open));
	h.send(Action::Back);
	h.until_screen(Screen::Inbox);
	h.until("the conversation closed", |s| s.conversation.is_none());
}

#[test]
fn pins_come_first_and_selection_acts_on_them_together() {
	let h = harness();
	h.demo();
	let keys: Vec<String> = h.with(|s| s.list.as_ref().unwrap().rows.iter().rev().take(2).map(|r| r.key.clone()).collect());
	h.send(Action::ToggleSelect { key: keys[0].clone() });
	h.until("selected", |s| s.list.as_ref().unwrap().selection.as_ref().is_some_and(|v| v.count == 1 && v.pin));
	h.send(Action::SelectionPin);
	h.until("pinned first", |s| {
		let list = s.list.as_ref().unwrap();
		list.selection.is_none() && list.rows[0].key == keys[0] && list.rows[0].pinned
	});
	// One pinned and one not: pin them all.
	h.send(Action::ToggleSelect { key: keys[0].clone() });
	h.send(Action::ToggleSelect { key: keys[1].clone() });
	h.until("a mix", |s| s.list.as_ref().unwrap().selection.as_ref().is_some_and(|v| v.count == 2 && v.pin));
	h.send(Action::SelectionPin);
	h.until("both pinned", |s| s.list.as_ref().unwrap().rows.iter().filter(|r| r.pinned).count() == 2);
	// All pinned: unpin them.
	h.send(Action::ToggleSelect { key: keys[0].clone() });
	h.send(Action::ToggleSelect { key: keys[1].clone() });
	h.until("all pinned", |s| s.list.as_ref().unwrap().selection.as_ref().is_some_and(|v| !v.pin));
	h.send(Action::SelectionPin);
	h.until("none pinned", |s| s.list.as_ref().unwrap().rows.iter().all(|r| !r.pinned));
	// Back clears a selection before it leaves anything.
	h.send(Action::SelectAll);
	h.until("all selected", |s| s.list.as_ref().unwrap().selection.is_some());
	h.send(Action::Back);
	h.until("selection cleared", |s| s.list.as_ref().unwrap().selection.is_none());
	assert_eq!(h.top(), Some(Screen::Inbox));
}

#[test]
fn labels_show_on_rows_and_come_off_again() {
	let h = harness();
	h.demo();
	let row = h.row("Photos from Saturday");
	h.send(Action::ToggleSelect { key: row.key.clone() });
	h.until("selected", |s| s.list.as_ref().unwrap().selection.is_some());
	h.send(Action::SelectionLabels);
	h.until("the label sheet", |s| matches!(&s.sheet, Some(SheetView::Labels { choices, .. }) if choices.iter().any(|c| c.name == "Travel" && !c.on)));
	let id = h.with(|s| match &s.sheet {
		Some(SheetView::Labels { choices, .. }) => choices.iter().find(|c| c.name == "Travel").unwrap().id.clone(),
		_ => unreachable!(),
	});
	h.send(Action::ToggleLabel { id: id.clone() });
	h.until("the label on the row", |s| s.list.as_ref().unwrap().rows.iter().any(|r| r.key == row.key && r.labels == vec!["Travel".to_string()]));
	h.until("the choice ticked", |s| matches!(&s.sheet, Some(SheetView::Labels { choices, .. }) if choices.iter().any(|c| c.name == "Travel" && c.on)));
	// A new label is made and put on at once.
	h.send(Action::CreateLabel { name: "Trips".into() });
	h.until("the new label on the row", |s| s.list.as_ref().unwrap().rows.iter().any(|r| r.key == row.key && r.labels.contains(&"Trips".to_string())));
	h.send(Action::ToggleLabel { id });
	h.until("the label off the row", |s| s.list.as_ref().unwrap().rows.iter().any(|r| r.key == row.key && !r.labels.contains(&"Travel".to_string())));
	h.send(Action::Back);
	h.until("the sheet closed", |s| s.sheet.is_none());
}

#[test]
fn a_reply_quotes_what_was_picked_and_lands_in_sent() {
	let h = harness();
	h.demo();
	h.open("Photos from Saturday");
	let (mail, block, words) = h.with(|s| {
		let card = s.conversation.as_ref().unwrap().cards.iter().find(|c| c.expanded).unwrap().clone();
		let block = card.blocks.iter().find(|b| matches!(b.kind, BlockKind::Text { .. })).unwrap().clone();
		(card.id, block.index, crate::doc::block_text(&block))
	});
	h.send(Action::TogglePick { id: mail.clone(), block });
	h.until("picked", |s| s.conversation.as_ref().unwrap().cards.iter().any(|c| c.blocks.iter().any(|b| b.picked)));
	h.send(Action::Answer { mail, kind: AnswerKind::Reply });
	h.until_screen(Screen::Composer);
	h.until("the composer", |s| s.composer.is_some());
	let view = h.with(|s| s.composer.clone().unwrap());
	assert!(view.text.to.contains("mia@tuta.com"), "{}", view.text.to);
	assert_eq!(view.text.subject, "Re: Photos from Saturday");
	assert!(view.picked_who.as_deref().is_some_and(|w| w.contains("Mia")));
	assert_eq!(view.picked_text.as_deref(), Some(words.trim()));
	assert!(view.whole_label.is_some() && !view.whole_included, "the whole mail is offered, not sent");
	h.send(Action::ComposeSend { text: ComposeText { body: "Lovely!".into(), ..view.text } });
	h.until("the message sent", |s| s.shell.as_ref().unwrap().snack.as_ref().is_some_and(|m| m.text.starts_with("Sent")));
	h.until("back from the composer", |s| s.composer.is_none());
	assert_eq!(h.top(), Some(Screen::Conversation));
	h.send(Action::Back);
	h.until_screen(Screen::Inbox);
	h.open_folder("Sent");
	h.until_row("Re: Photos from Saturday");
}

#[test]
fn bad_recipients_stay_in_the_composer_with_the_reason() {
	let h = harness();
	h.demo();
	h.send(Action::ComposeNew);
	h.until("the composer", |s| s.composer.is_some());
	h.send(Action::ComposeSend { text: text("not an address", "Hi", "x") });
	h.until("the reason", |s| s.composer.as_ref().is_some_and(|c| c.bad_to && c.error.as_deref().is_some_and(|e| e.contains("not an address"))));
	assert_eq!(h.top(), Some(Screen::Composer));
	h.send(Action::ComposeSend { text: text("", "Hi", "x") });
	h.until("asking for a recipient", |s| s.composer.as_ref().is_some_and(|c| c.error.as_deref() == Some("Add a recipient")));
	h.send(Action::ComposeEdited);
	h.until("cleared", |s| s.composer.as_ref().is_some_and(|c| c.error.is_none() && !c.bad_to));
}

#[test]
fn closing_keeps_a_draft_and_opening_it_goes_on_writing() {
	let h = harness();
	h.demo();
	h.send(Action::ComposeNew);
	h.until("the composer", |s| s.composer.is_some());
	// Nothing written, nothing kept.
	h.send(Action::ComposeClose { text: text("", "", "") });
	h.until_screen(Screen::Inbox);
	assert!(h.with(|s| s.shell.as_ref().unwrap().snack.is_none()));
	h.send(Action::ComposeNew);
	h.until("the composer", |s| s.composer.is_some());
	h.send(Action::ComposeClose { text: text("mia@tuta.com", "Trip plan", "Day one") });
	h.until_screen(Screen::Inbox);
	h.until("saved", |s| s.shell.as_ref().unwrap().snack.as_ref().is_some_and(|m| m.text == "Saved to Drafts"));
	h.open_folder("Drafts");
	h.until_row("Trip plan");
	let row = h.row("Trip plan");
	assert!(row.draft);
	h.send(Action::OpenThread { key: row.key, folder_id: row.folder_id });
	h.until("the draft in the composer", |s| s.composer.as_ref().is_some_and(|c| c.text.subject == "Trip plan" && c.text.body == "Day one"));
	assert_eq!(h.top(), Some(Screen::Composer));
	// Discarding asks first, then the draft goes.
	h.send(Action::ComposeDiscard { text: text("mia@tuta.com", "Trip plan", "Day one") });
	h.until("asking", |s| s.sheet == Some(SheetView::Discard));
	h.send(Action::ComposeDiscardConfirmed);
	h.until_screen(Screen::Inbox);
	h.until("the draft gone", |s| !s.list.as_ref().unwrap().rows.iter().any(|r| r.subject == "Trip plan"));
}

#[test]
fn a_link_fills_a_composer_and_attachments_are_refused_from_it() {
	let h = harness();
	// Before signing in the link waits.
	h.until_screen(Screen::SignIn);
	h.send(Action::OpenLink { url: "mailto:sam@tuta.com?subject=Hello%20there&attach=/etc/passwd".into() });
	std::thread::sleep(Duration::from_millis(50));
	assert_eq!(h.top(), Some(Screen::SignIn));
	h.send(Action::SignIn { email: "demo".into(), password: String::new() });
	h.until_screen(Screen::Composer);
	h.until("the fields", |s| s.composer.as_ref().is_some_and(|c| c.text.to == "sam@tuta.com" && c.text.subject == "Hello there"));
	assert!(h.with(|s| s.composer.as_ref().unwrap().attachments.is_empty()));
	h.send(Action::ComposeClose { text: text("sam@tuta.com", "Hello there", "") });
	h.until_screen(Screen::Inbox);
	// Other links go to the browser, and only web links.
	h.send(Action::OpenLink { url: "https://example.org/".into() });
	h.send(Action::OpenLink { url: "file:///etc/passwd".into() });
	h.until("the browser", |s| s.effects.iter().any(|e| matches!(e, Effect::OpenUrl { url } if url == "https://example.org/")));
	assert!(!h.with(|s| s.effects.iter().any(|e| matches!(e, Effect::OpenUrl { url } if url.starts_with("file:")))));
}

#[test]
fn attachments_too_big_are_refused_and_small_ones_listed() {
	let h = harness();
	h.demo();
	h.send(Action::ComposeNew);
	h.until("the composer", |s| s.composer.is_some());
	h.send(Action::ComposeAttach { name: "../../a.pdf".into(), mime: String::new(), data: vec![0; 2000] });
	h.until("listed", |s| s.composer.as_ref().is_some_and(|c| c.attachments.len() == 1 && c.attachments[0].name == "a.pdf" && c.attachments[0].size == "2.0 kB"));
	h.send(Action::ComposeAttach { name: "big.bin".into(), mime: String::new(), data: vec![0; 27_000_000] });
	h.until("refused", |s| s.composer.as_ref().is_some_and(|c| c.attachments.len() == 1 && c.error.as_deref().is_some_and(|e| e.contains("25 MB"))));
	h.send(Action::ComposeRemoveAttachment { index: 0 });
	h.until("removed", |s| s.composer.as_ref().is_some_and(|c| c.attachments.is_empty()));
}

#[test]
fn search_finds_mail_and_opens_it_from_its_folder() {
	let h = harness();
	h.demo();
	h.send(Action::OpenSearch);
	h.until_screen(Screen::Search);
	h.send(Action::SearchText { text: "invoice".into() });
	h.until("a hit", |s| s.search.as_ref().is_some_and(|v| v.rows.iter().any(|r| r.subject.contains("invoice"))));
	let row = h.with(|s| s.search.as_ref().unwrap().rows[0].clone());
	assert!(!row.folder_id.is_empty());
	h.send(Action::OpenThread { key: row.key, folder_id: row.folder_id });
	h.until_screen(Screen::Conversation);
	h.send(Action::Back);
	h.until_screen(Screen::Search);
	h.send(Action::SearchText { text: "zzzznothing".into() });
	h.until("nothing", |s| s.search.as_ref().is_some_and(|v| v.empty));
	h.send(Action::Back);
	h.until_screen(Screen::Inbox);
}

#[test]
fn trash_and_archive_move_a_conversation_and_leave_it() {
	let h = harness();
	h.demo();
	h.open("Photos from Saturday");
	h.send(Action::ConversationArchive);
	h.until_screen(Screen::Inbox);
	h.until("said", |s| s.shell.as_ref().unwrap().snack.as_ref().is_some_and(|m| m.text == "Moved to Archive"));
	h.until("gone from the list", |s| !s.list.as_ref().unwrap().rows.iter().any(|r| r.subject == "Photos from Saturday"));
	h.open_folder("Archive");
	h.until_row("Photos from Saturday");
	h.open("Photos from Saturday");
	h.send(Action::ConversationTrash);
	h.until_screen(Screen::Inbox);
	h.open_folder("Trash");
	h.until_row("Photos from Saturday");
}

#[test]
fn move_to_spam_offers_mail_like_it_and_can_keep_a_rule() {
	let h = harness();
	h.demo();
	let row = h.row("50% off everything this weekend");
	h.send(Action::ToggleSelect { key: row.key });
	h.until("selected", |s| s.list.as_ref().unwrap().selection.is_some());
	h.send(Action::SelectionSpam);
	h.until("the matches", |s| matches!(&s.sheet, Some(SheetView::Spam { rows, .. }) if rows.len() >= 3));
	let (line, picked) = h.with(|s| match &s.sheet {
		Some(SheetView::Spam { line, rows, .. }) => (line.clone(), rows.iter().filter(|r| r.picked).count()),
		_ => unreachable!(),
	});
	assert!(line.starts_with("from:"), "{line}");
	assert_eq!(picked, 1, "only the conversation's own mail starts ticked");
	h.send(Action::SpamLine { line: "from:".into() });
	h.send(Action::SpamLine { line: "nonsense:(".into() });
	h.until("the problem", |s| matches!(&s.sheet, Some(SheetView::Spam { problem: Some(_), .. })));
	h.send(Action::SpamLine { line: line.clone() });
	h.until("back to matches", |s| matches!(&s.sheet, Some(SheetView::Spam { problem: None, rows, .. }) if rows.len() >= 3));
	h.send(Action::SpamSelectAll { on: true });
	h.send(Action::SpamKeepRule { on: true });
	h.until("all ticked", |s| matches!(&s.sheet, Some(SheetView::Spam { rows, keep_rule: true, .. }) if rows.iter().all(|r| r.picked)));
	h.send(Action::SpamConfirm);
	h.until("moved", |s| s.shell.as_ref().unwrap().snack.as_ref().is_some_and(|m| m.text.contains("and added a rule")));
	h.until("gone", |s| !s.list.as_ref().unwrap().rows.iter().any(|r| r.subject.contains("50% off")));
	h.send(Action::OpenRules);
	h.until("the rule", |s| s.rules.as_ref().is_some_and(|r| r.rows.len() == 1 && r.rows[0].when == line && !r.rows[0].broken));
}

#[test]
fn a_rule_is_written_previewed_saved_and_deleted() {
	let h = harness();
	h.demo();
	h.send(Action::OpenRules);
	h.until_screen(Screen::Rules);
	h.send(Action::OpenRule { index: None });
	h.until("the page", |s| s.rule_editor.is_some());
	h.send(Action::RulePut { piece: RulePiece::Subject, cursor: 0, selection_end: 0 });
	h.until("the field", |s| s.rule_editor.as_ref().is_some_and(|e| e.line == "subject:" && e.cursor == 8 && !e.can_save));
	h.send(Action::RuleLine { line: "subject:invoice".into() });
	h.until("caught", |s| s.rule_editor.as_ref().is_some_and(|e| e.tone == PreviewTone::Good && e.message.starts_with("Catches") && !e.preview.is_empty() && e.can_save));
	assert!(h.with(|s| s.rule_editor.as_ref().unwrap().spans.iter().any(|p| p.part == LinePart::Subject)));
	h.send(Action::RuleLine { line: "subject:(".into() });
	h.until("the problem", |s| s.rule_editor.as_ref().is_some_and(|e| e.tone == PreviewTone::Bad && !e.can_save));
	h.send(Action::RuleLine { line: "subject:invoice".into() });
	h.send(Action::RuleSetDoes { does: RuleDoes::Label });
	h.send(Action::RuleSetLabel { name: "Bills".into() });
	h.until("saveable", |s| s.rule_editor.as_ref().is_some_and(|e| e.can_save && e.does == RuleDoes::Label));
	h.send(Action::RuleSave);
	h.until_screen(Screen::Rules);
	h.until("listed", |s| s.rules.as_ref().is_some_and(|r| r.rows.len() == 1 && r.rows[0].does == "Label Bills" && !r.rows[0].broken));
	h.send(Action::OpenRule { index: Some(0) });
	h.until("reopened", |s| s.rule_editor.as_ref().is_some_and(|e| e.editing && e.line == "subject:invoice" && e.label == "Bills"));
	h.send(Action::RuleDelete);
	h.until_screen(Screen::Rules);
	h.until("deleted", |s| s.rules.as_ref().is_some_and(|r| r.rows.is_empty()));
}

#[test]
fn settings_are_saved_and_published() {
	let h = harness();
	h.demo();
	h.send(Action::OpenSettings);
	h.until_screen(Screen::Settings);
	h.until("measured", |s| s.settings.as_ref().is_some_and(|v| v.storage_line.contains("on this phone")));
	h.send(Action::SetKeepReady { value: "high".into() });
	h.send(Action::SetCleanupLimit { value: "5000".into() });
	h.send(Action::SetCleanupLimit { value: "3".into() });
	h.send(Action::SetForget { on: true });
	h.send(Action::SetPureBlack { on: false });
	h.send(Action::SetMatchWallpaper { on: true });
	h.until("published", |s| {
		let v = s.settings.as_ref().unwrap();
		v.keep_ready_line.starts_with("High") && v.cleanup_limit_line == "5.0 GB" && v.forget_on_sign_out && !v.pure_black && v.match_wallpaper && !s.shell.as_ref().unwrap().pure_black
	});
	let saved = Settings::load(&h.paths);
	assert_eq!(saved.cleanup_mb, 5000, "a size not offered is ignored");
	assert!(saved.forget_on_sign_out && !saved.pure_black && saved.match_wallpaper);
}

#[test]
fn the_connection_shows_in_the_header_and_comes_back() {
	let h = harness();
	h.demo();
	h.until("settled", |s| s.shell.as_ref().unwrap().status.kind == ConnectionKind::Connected);
	h.send(Action::NetworkChanged { online: false });
	h.until("offline", |s| {
		let status = &s.shell.as_ref().unwrap().status;
		status.kind == ConnectionKind::Offline && status.line == "Offline · saved mail"
	});
	h.send(Action::NetworkChanged { online: true });
	h.until("back", |s| s.shell.as_ref().unwrap().snack.as_ref().is_some_and(|m| m.text == "Back online"));
	h.until("connected", |s| s.shell.as_ref().unwrap().status.kind == ConnectionKind::Connected);
	let id = h.with(|s| s.shell.as_ref().unwrap().snack.as_ref().unwrap().id);
	h.send(Action::SnackGone { id });
	h.until("the message gone", |s| s.shell.as_ref().unwrap().snack.is_none());
}

#[test]
fn signing_out_returns_to_sign_in_and_forgets_the_screens() {
	let h = harness();
	h.demo();
	h.open("Photos from Saturday");
	h.send(Action::SignOut);
	h.until_screen(Screen::SignIn);
	h.until("cleared", |s| s.conversation.is_none() && s.list.as_ref().is_some_and(|l| l.rows.is_empty()));
	assert_eq!(h.with(|s| s.signin.as_ref().unwrap().email.clone()), "demo");
	h.send(Action::SignIn { email: "demo".into(), password: String::new() });
	h.until_row("Photos from Saturday");
}

#[test]
fn publishing_sends_only_what_changed() {
	let h = harness();
	h.demo();
	h.until("settled", |s| s.shell.as_ref().unwrap().status.kind == ConnectionKind::Connected);
	std::thread::sleep(Duration::from_millis(100));
	let before = h.with(|s| s.updates);
	h.send(Action::CloseSheet);
	h.send(Action::ClearSelection);
	std::thread::sleep(Duration::from_millis(100));
	assert_eq!(h.with(|s| s.updates), before, "nothing changed, nothing sent");
}

#[test]
fn a_saved_account_opens_at_once_from_the_encrypted_cache_with_no_server() {
	// The session is refused or out of reach: the mailbox still opens, on what is kept here.
	let secrets: Arc<Secrets> = Arc::default();
	let session = serde_json::to_vec(&("me@tuta.com", "not a session the server knows")).unwrap();
	secrets.put("session".into(), session).unwrap();
	let h = harness_with(secrets.clone());
	h.until_screen(Screen::Inbox);
	h.until("the address", |s| s.shell.as_ref().unwrap().address == "me@tuta.com");
	// A key was made and kept for the cache, and the cache on disk opens only with it.
	let key = secrets.get("cache-key:me@tuta.com".into()).unwrap().expect("a cache key is kept");
	assert_eq!(key.len(), 32);
	let path = h.paths.mail_cache("me@tuta.com");
	h.until("the cache file", |_| path.exists());
	let key: [u8; 32] = key.try_into().unwrap();
	assert!(tern_cache::Cache::open(&path, &key).is_ok(), "opens with its key");
	let mut wrong = key;
	wrong[0] ^= 1;
	assert!(tern_cache::Cache::open(&path, &wrong).is_err() || std::fs::read(&path).unwrap().windows(15).all(|w| w != b"SQLite format 3"), "and is not plain SQLite");
	assert!(!std::fs::read(&path).unwrap().windows(15).any(|w| w == b"SQLite format 3"), "the file on disk is encrypted");
}

#[test]
fn the_certificates_come_from_the_first_directory_there_is() {
	let dir = std::env::temp_dir();
	let dir = dir.to_str().unwrap();
	assert_eq!(first_dir(&format!("/no/such/place:{dir}:/also/not")), Some(dir));
	assert_eq!(first_dir(""), None);
	assert_eq!(first_dir("/no/such/place"), None);
}

fn check(h: &Harness) -> BackgroundResult {
	let engine = h.engine.clone();
	std::thread::spawn(move || engine.background_check()).join().unwrap()
}

#[test]
fn notifications_and_background_checks_are_off_until_asked_for_and_depend_on_each_other() {
	let h = harness();
	h.demo();
	h.send(Action::OpenSettings);
	h.until("the settings", |s| s.settings.as_ref().is_some_and(|v| v.background_line.contains("only") && !v.notifications));
	assert_eq!(h.with(|s| s.shell.as_ref().unwrap().background_minutes), 0);
	// Asking for notifications turns on the checks that find the mail.
	h.send(Action::SetNotifications { on: true });
	h.until("both on", |s| s.settings.as_ref().unwrap().notifications && s.shell.as_ref().unwrap().background_minutes == 30);
	h.send(Action::SetBackground { value: "15".into() });
	h.send(Action::SetBackground { value: "7".into() });
	h.until("the pace", |s| s.shell.as_ref().unwrap().background_minutes == 15 && s.settings.as_ref().unwrap().background_line.starts_with("Every 15 minutes"));
	assert!(h.with(|s| s.settings.as_ref().unwrap().notifications));
	// Turning off the checks turns off what depends on them.
	h.send(Action::SetBackground { value: "0".into() });
	h.until("both off", |s| !s.settings.as_ref().unwrap().notifications && s.shell.as_ref().unwrap().background_minutes == 0);
	let saved = Settings::load(&h.paths);
	assert_eq!((saved.background_minutes, saved.notifications), (0, false));
	// Signed out there is nothing to check.
	h.send(Action::SetNotifications { on: true });
	h.until("on again", |s| s.shell.as_ref().unwrap().background_minutes == 30);
	h.send(Action::SignOut);
	h.until_screen(Screen::SignIn);
	h.until("no checks", |s| s.shell.as_ref().unwrap().background_minutes == 0);
}

#[test]
fn a_check_says_what_is_new_once_and_not_while_on_screen_or_when_off() {
	let h = harness();
	h.demo();
	// Off: the mail arrives quietly.
	let quiet = check(&h);
	assert!(quiet.notices.is_empty() && !quiet.retry, "{quiet:?}");
	h.send(Action::SetNotifications { on: true });
	h.until("on", |s| s.settings.as_ref().unwrap().notifications);
	h.send(Action::Hidden);
	let first = check(&h);
	assert_eq!(first.notices.len(), 1, "{first:?}");
	let notice = &first.notices[0];
	assert_eq!((notice.who.as_str(), notice.subject.as_str()), ("Sam Okafor", "Table booked for 12:30"));
	assert!(notice.id >= 0);
	assert_eq!(first.summary, None);
	h.until_row("Table booked for 12:30");
	// The same conversation has more: one notice for it, under the same id.
	let again = check(&h);
	assert_eq!(again.notices.len(), 1);
	assert_eq!(again.notices[0].id, notice.id, "news of a conversation replaces what was shown");
	// On screen nothing is said, and it does not come up later either.
	h.send(Action::Foreground);
	h.until("told to clear", |s| s.effects.contains(&Effect::ClearNotifications));
	assert!(check(&h).notices.is_empty());
	h.send(Action::Hidden);
	std::thread::sleep(Duration::from_millis(50));
	let after = check(&h);
	assert_eq!(after.notices.len(), 1, "only what came after: {after:?}");
	// Tapping a notice opens its conversation.
	h.send(Action::OpenThread { key: notice.key.clone(), folder_id: notice.folder_id.clone() });
	h.until_screen(Screen::Conversation);
}

#[test]
fn a_check_with_no_account_answers_at_once() {
	let h = harness();
	h.until_screen(Screen::SignIn);
	let result = check(&h);
	assert_eq!(result, BackgroundResult { notices: Vec::new(), summary: None, retry: false });
}

#[test]
fn a_conversation_asked_for_before_the_account_opens_waits_for_it() {
	let h = harness();
	h.until_screen(Screen::SignIn);
	h.send(Action::OpenThread { key: "lunch on friday?".into(), folder_id: "inbox".into() });
	std::thread::sleep(Duration::from_millis(50));
	assert_eq!(h.top(), Some(Screen::SignIn));
}

#[test]
fn the_wording_says_which_device_this_is_and_parts_follow_the_session() {
	let dir = std::env::temp_dir().join(format!("tern-host-test-{}", std::process::id()));
	let seen: Arc<Mutex<Seen>> = Arc::default();
	let options = Options { device: "computer", ..Options::default() };
	let engine = Engine::open_with(Paths::new(&dir, &dir), options, Arc::<Secrets>::default(), Arc::new(Screen_(seen.clone())));
	let h = Harness { engine, seen, paths: Paths::new(&dir, &dir), dir };
	assert!(h.engine.session_parts().is_none());
	h.demo();
	assert_eq!(h.engine.session_parts().map(|p| p.backend.account().address).as_deref(), Some("you@tuta.com"));
	h.send(Action::OpenSettings);
	h.until("the wording for this device", |s| s.settings.as_ref().is_some_and(|v| v.semantic_line.contains("this computer")));
	h.send(Action::SignOut);
	h.until_screen(Screen::SignIn);
	h.until("the parts gone", |_| h.engine.session_parts().is_none());
}

#[test]
fn an_account_given_at_the_start_opens_at_once_and_is_kept_nowhere() {
	let dir = std::env::temp_dir().join(format!("tern-given-test-{}", std::process::id()));
	let secrets: Arc<Secrets> = Arc::default();
	let seen: Arc<Mutex<Seen>> = Arc::default();
	let options = Options { signed_in: Some(Arc::new(tern_demo::DemoBackend::new())), ..Options::default() };
	let engine = Engine::open_with(Paths::new(&dir, &dir), options, secrets.clone(), Arc::new(Screen_(seen.clone())));
	let h = Harness { engine, seen, paths: Paths::new(&dir, &dir), dir };
	h.until_screen(Screen::Inbox);
	h.until_row("Photos from Saturday");
	assert!(secrets.0.lock().unwrap().is_empty());
}

#[test]
fn dropping_the_engine_ends_its_account_and_keeps_it_signed_in() {
	let dir = std::env::temp_dir().join(format!("tern-drop-test-{}", std::process::id()));
	let secrets: Arc<Secrets> = Arc::default();
	secrets.put(SESSION_SECRET.into(), serde_json::to_vec(&("me@tuta.com", "not a session the server knows")).unwrap()).unwrap();
	let engine = Engine::open_with(Paths::new(&dir, &dir), Options::default(), secrets.clone(), Arc::new(Screen_(Arc::default())));
	let start = Instant::now();
	while engine.session_parts().is_none() {
		assert!(start.elapsed() < Duration::from_secs(10), "no session opened");
		std::thread::sleep(Duration::from_millis(5));
	}
	let parts = engine.parts.clone();
	drop(engine);
	while parts.lock().unwrap().is_some() {
		assert!(start.elapsed() < Duration::from_secs(10), "the account went on after the engine");
		std::thread::sleep(Duration::from_millis(5));
	}
	assert!(secrets.get(SESSION_SECRET.into()).unwrap().is_some(), "still signed in for next time");
	let _ = std::fs::remove_dir_all(dir);
}

/// A list and a conversation on screen together, as on the desktop.
fn side_by_side() -> Harness {
	static NEXT: AtomicU32 = AtomicU32::new(0);
	let dir = std::env::temp_dir().join(format!("tern-beside-test-{}-{}", std::process::id(), NEXT.fetch_add(1, Ordering::Relaxed)));
	let seen: Arc<Mutex<Seen>> = Arc::default();
	let options = Options { follow_neighbour: true, lone_draft_composes: false, retry_hint: "Press F5 to try again.", device: "computer", ..Options::default() };
	let engine = Engine::open_with(Paths::new(&dir, &dir), options, Arc::<Secrets>::default(), Arc::new(Screen_(seen.clone())));
	Harness { engine, seen, paths: Paths::new(&dir, &dir), dir }
}

#[test]
fn beside_the_list_the_next_conversation_opens_when_one_leaves() {
	let h = side_by_side();
	h.demo();
	h.open("Your invoice for October");
	let keys = |h: &Harness| h.with(|s| s.list.as_ref().unwrap().rows.iter().map(|r| r.key.clone()).collect::<Vec<_>>());
	let before = keys(&h);
	let at = h.with(|s| s.list.as_ref().unwrap().rows.iter().position(|r| r.subject == "Your invoice for October")).unwrap();
	h.send(Action::ConversationTrash);
	h.until("the row gone", |s| s.list.as_ref().unwrap().rows.iter().all(|r| r.subject != "Your invoice for October"));
	let next = h.with(|s| s.list.as_ref().unwrap().rows[at].subject.clone());
	assert_ne!(before.len(), keys(&h).len());
	h.until("the neighbour open", |s| s.conversation.as_ref().is_some_and(|c| c.subject == next));
	h.until("its row marked open", |s| s.list.as_ref().unwrap().rows[at].open);
	assert_eq!(h.top(), Some(Screen::Conversation));
}

#[test]
fn on_a_phone_a_conversation_that_leaves_goes_back_to_the_list() {
	let h = harness();
	h.demo();
	h.open("Your invoice for October");
	h.send(Action::ConversationTrash);
	h.until_screen(Screen::Inbox);
	h.until("nothing open", |s| s.conversation.is_none());
}

#[test]
fn a_row_menu_acts_without_a_selection_or_the_conversation_open() {
	let h = side_by_side();
	h.demo();
	let row = h.row("Your invoice for October");
	h.send(Action::RowPin { key: row.key.clone() });
	h.until("pinned on top", |s| s.list.as_ref().unwrap().rows[0].key == row.key && s.list.as_ref().unwrap().rows[0].pinned);
	h.send(Action::RowRead { key: row.key.clone(), folder_id: row.folder_id.clone(), read: true });
	h.until("read", |s| !s.list.as_ref().unwrap().rows[0].unread);
	h.send(Action::RowRead { key: row.key.clone(), folder_id: row.folder_id.clone(), read: false });
	h.until("unread", |s| s.list.as_ref().unwrap().rows[0].unread);
	h.send(Action::RowSpam { key: row.key.clone(), folder_id: row.folder_id.clone() });
	h.until("the spam sheet", |s| matches!(s.sheet, Some(SheetView::Spam { .. })));
	h.send(Action::CloseSheet);
	h.send(Action::RowTrash { key: row.key.clone(), folder_id: row.folder_id });
	h.until("gone", |s| s.list.as_ref().unwrap().rows.iter().all(|r| r.key != row.key));
}

#[test]
fn marking_the_open_conversation_unread_from_its_row_leaves_it_unread() {
	let h = side_by_side();
	h.demo();
	h.open("Your invoice for October");
	let row = h.row("Your invoice for October");
	h.until("read by opening", |s| s.list.as_ref().unwrap().rows.iter().any(|r| r.key == row.key && !r.unread));
	h.send(Action::RowRead { key: row.key.clone(), folder_id: row.folder_id, read: false });
	h.until("unread and closed", |s| s.conversation.is_none() && s.list.as_ref().unwrap().rows.iter().any(|r| r.key == row.key && r.unread));
}

#[test]
fn typing_on_in_a_search_closes_what_was_open_and_closing_goes_back_to_the_folder() {
	let h = side_by_side();
	h.demo();
	h.send(Action::OpenSearch);
	h.send(Action::SearchText { text: "invoice".into() });
	h.until("a result", |s| s.search.as_ref().is_some_and(|v| v.rows.len() == 1));
	let (key, folder_id) = h.with(|s| { let r = &s.search.as_ref().unwrap().rows[0]; (r.key.clone(), r.folder_id.clone()) });
	h.send(Action::OpenThread { key, folder_id });
	h.until("open", |s| s.conversation.is_some());
	h.send(Action::SearchText { text: "invoice october".into() });
	h.until("closed by typing", |s| s.conversation.is_none());
	h.send(Action::CloseSearch);
	h.until("back to the folder", |s| s.shell.as_ref().unwrap().stack == [Screen::Inbox]);
}

#[test]
fn a_lone_draft_can_open_as_a_conversation_and_be_discarded() {
	let h = side_by_side();
	h.demo();
	h.send(Action::ComposeNew);
	h.until_screen(Screen::Composer);
	h.send(Action::ComposeClose { text: ComposeText { to: "mia@tuta.com".into(), cc: String::new(), bcc: String::new(), subject: "Plan".into(), body: "Soon".into(), from_index: 0 } });
	h.until("not composing", |s| s.composer.is_none());
	let drafts = h.with(|s| s.list.as_ref().unwrap().folders.iter().find(|f| f.icon == FolderIcon::Drafts).unwrap().id.clone());
	h.send(Action::OpenFolder { id: drafts });
	h.until_row("Plan");
	h.open("Plan");
	h.until("it opens as a draft", |s| s.conversation.as_ref().is_some_and(|c| c.cards[0].draft));
	assert!(h.with(|s| s.composer.is_none()));
	let mail = h.with(|s| s.conversation.as_ref().unwrap().cards[0].id.clone());
	h.send(Action::DiscardDraft { mail });
	h.until("gone from Drafts", |s| s.list.as_ref().unwrap().rows.is_empty());
	h.until("nothing left open", |s| s.conversation.is_none());
}

#[test]
fn a_card_says_its_subject_and_where_it_shows_on_the_web() {
	let h = side_by_side();
	h.demo();
	h.open("Photos from Saturday");
	h.until("its web link once open", |s| s.conversation.as_ref().is_some_and(|c| c.cards.iter().any(|c| c.open_url.is_some())));
	assert_eq!(h.with(|s| s.conversation.as_ref().unwrap().cards[0].subject.clone()), "Photos from Saturday");
}


#[test]
fn several_composers_stay_apart() {
	let h = side_by_side();
	h.demo();
	h.send(Action::ComposeNew);
	h.send(Action::ComposeNew);
	h.until("two composers", |s| s.composers.len() == 2);
	let (first, second) = h.with(|s| (s.composers[0].id, s.composers[1].id));
	h.send(Action::ComposeFocus { id: first });
	h.send(Action::ComposeAttach { name: "a.txt".into(), mime: "text/plain".into(), data: b"a".to_vec() });
	h.until("the attachment on the first", |s| s.composers[0].attachments.len() == 1 && s.composers[1].attachments.is_empty());
	// Sending one leaves the other, which actions then apply to.
	h.send(Action::ComposeFocus { id: second });
	h.send(Action::ComposeSend { text: text("mia@tuta.com", "Second", "Hi") });
	h.until("the second gone", |s| s.composers.len() == 1 && s.composers[0].id == first);
	assert_eq!(h.with(|s| s.composer.as_ref().map(|c| c.id)), Some(first));
	h.send(Action::ComposeClose { text: text("", "", "") });
	h.until("the first saved and gone", |s| s.composers.is_empty());
}

#[test]
fn a_failed_send_stays_with_its_own_composer() {
	let h = side_by_side();
	h.demo();
	h.send(Action::ComposeNew);
	h.send(Action::ComposeNew);
	h.until("two composers", |s| s.composers.len() == 2);
	let first = h.with(|s| s.composers[0].id);
	h.send(Action::ComposeFocus { id: first });
	h.send(Action::ComposeSend { text: text("not an address", "One", "Hi") });
	h.until("the error on the first", |s| s.composers[0].bad_to && s.composers[0].error.is_some());
	assert!(h.with(|s| !s.composers[1].bad_to && s.composers[1].error.is_none()));
}

#[test]
fn the_lines_a_platform_selected_are_what_a_reply_quotes() {
	let h = side_by_side();
	h.demo();
	h.open("Re: Lunch on Friday?");
	h.until("it opens", |s| s.conversation.as_ref().is_some_and(|c| c.cards.iter().any(|c| c.current && c.open_url.is_some())));
	let mail = h.with(|s| s.conversation.as_ref().unwrap().cards.iter().find(|c| c.current).unwrap().id.clone());
	h.send(Action::AnswerPicked { mail, kind: AnswerKind::Reply, picked: Some("just these words".into()) });
	h.until("a composer", |s| s.composers.len() == 1);
	assert_eq!(h.with(|s| s.composers[0].picked_text.clone()).as_deref(), Some("just these words"));
}

#[test]
fn a_mail_link_from_outside_waits_for_an_account() {
	let dir = std::env::temp_dir().join(format!("tern-link-test-{}", std::process::id()));
	let seen: Arc<Mutex<Seen>> = Arc::default();
	let engine = Engine::open_with(Paths::new(&dir, &dir), Options::default(), Arc::<Secrets>::default(), Arc::new(Screen_(seen.clone())));
	let h = Harness { engine, seen, paths: Paths::new(&dir, &dir), dir };
	h.engine.compose(tern_compose::mailto_fields("mailto:mia@tuta.com?subject=Hello").unwrap());
	h.until_screen(Screen::SignIn);
	assert!(h.with(|s| s.composers.is_empty()));
	h.demo();
	h.until("the composer", |s| s.composers.len() == 1 && s.composers[0].text.subject == "Hello");
}

#[test]
fn labels_go_on_a_row_and_its_conversation_and_can_be_browsed_across_folders() {
	let h = side_by_side();
	h.demo();
	let row = h.row("Your invoice for October");
	h.until("the labels", |s| !s.list.as_ref().unwrap().labels.is_empty());
	let label = h.with(|s| s.list.as_ref().unwrap().labels.iter().find(|l| l.name == "Travel").unwrap().clone());
	assert_eq!(label.icon, FolderIcon::Label);

	h.send(Action::RowLabels { key: row.key.clone(), folder_id: row.folder_id.clone() });
	h.until("the sheet", |s| matches!(&s.sheet, Some(SheetView::Labels { choices, .. }) if choices.iter().all(|c| !c.on)));
	h.send(Action::ToggleLabel { id: label.id.clone() });
	h.until("ticked", |s| matches!(&s.sheet, Some(SheetView::Labels { choices, .. }) if choices.iter().any(|c| c.on)));
	h.until("on the row", |s| s.list.as_ref().unwrap().rows.iter().any(|r| r.key == row.key && r.labels == ["Travel"]));
	h.send(Action::CreateLabel { name: "Bills".into() });
	h.until("a new label on the row too", |s| s.list.as_ref().unwrap().rows.iter().any(|r| r.key == row.key && r.labels.iter().any(|l| l == "Bills")));
	h.send(Action::CloseSheet);

	h.send(Action::OpenThread { key: row.key.clone(), folder_id: row.folder_id.clone() });
	h.until("on the conversation", |s| s.conversation.as_ref().is_some_and(|c| c.labels.iter().any(|l| l == "Travel")));

	h.send(Action::OpenLabel { id: label.id.clone() });
	h.until("only what carries it", |s| s.search.as_ref().is_some_and(|v| v.browsing && v.title == "Travel" && v.rows.len() == 1 && v.rows[0].key == row.key));
	h.until("the label marked on show", |s| s.list.as_ref().unwrap().labels.iter().any(|l| l.id == label.id && l.selected) && s.list.as_ref().unwrap().folders.iter().all(|f| !f.selected));

	// Taking it off empties the list it is on.
	h.send(Action::RowLabels { key: row.key.clone(), folder_id: row.folder_id.clone() });
	h.until("the sheet", |s| matches!(s.sheet, Some(SheetView::Labels { .. })));
	h.send(Action::ToggleLabel { id: label.id.clone() });
	h.until("nothing carries it", |s| s.search.as_ref().is_some_and(|v| v.rows.is_empty() && v.empty && v.empty_title == "No mail with this label"));
	h.send(Action::CloseSheet);

	let inbox = h.with(|s| s.list.as_ref().unwrap().folders[0].id.clone());
	h.send(Action::OpenFolder { id: inbox });
	h.until("back in the folder", |s| s.shell.as_ref().unwrap().stack == [Screen::Inbox] && s.list.as_ref().unwrap().folders[0].selected);
}

#[test]
fn each_try_at_sending_is_told_even_with_the_same_problem() {
	let h = side_by_side();
	h.demo();
	h.send(Action::ComposeNew);
	h.until("a composer", |s| s.composers.len() == 1);
	h.send(Action::ComposeSend { text: text("not an address", "One", "Hi") });
	h.until("the problem", |s| s.composers[0].tries == 1 && s.composers[0].error.is_some());
	let said = h.with(|s| s.composers[0].error.clone());
	h.send(Action::ComposeSend { text: text("not an address", "One", "Hi") });
	h.until("told again", |s| s.composers[0].tries == 2);
	assert_eq!(h.with(|s| s.composers[0].error.clone()), said);
}

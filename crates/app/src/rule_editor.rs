//! The Rules section of Preferences: the rules in a row that opens up, each edited on a page of its
//! own with the mail it would catch counted when typing pauses.

use std::cell::RefCell;
use std::rc::Rc;
use std::time::Duration;

use adw::prelude::*;
use gtk::glib;
use tern_core::rules::{parse, parts, Action, Field, Part, Rule};
use tern_core::{Folder, FolderKind};

use crate::window::Ctx;
use crate::{rules, runtime};

const HELP: &str = "from:, to: and subject: look at the sender, the first recipient and the subject; a word on its own looks at the subject and the sender. @example.com is a whole domain. Join terms with and, or and not, and group them in brackets: (from:@example.com or subject:\"offer\") and not to:me@tuta.com";

/// What a rule can do, in the order offered.
const DOES: [&str; 3] = ["Move to Spam", "Move to Folder", "Add Label"];
/// The last choice of label, which makes one.
const NEW_LABEL: &str = "New Label…";

pub struct Rules {
	ctx: Ctx,
	dialog: adw::PreferencesDialog,
	pub group: adw::PreferencesGroup,
	pub expander: adw::ExpanderRow,
	rows: RefCell<Vec<gtk::Widget>>,
	rules: RefCell<Vec<Rule>>,
}

impl Rules {
	pub fn new(ctx: &Ctx, dialog: &adw::PreferencesDialog) -> Rc<Rules> {
		let group = adw::PreferencesGroup::builder().title("Rules").description("Rules sort new mail in the Inbox as it arrives.").build();
		let expander = adw::ExpanderRow::builder().title("Mail Rules").build();
		group.add(&expander);
		let this = Rc::new(Rules {
			ctx: ctx.clone(),
			dialog: dialog.clone(),
			group,
			expander,
			rows: RefCell::default(),
			rules: RefCell::new(rules::load()),
		});
		this.fill();
		this
	}

	/// One row per rule, then Add Rule.
	fn fill(self: &Rc<Self>) {
		for row in self.rows.borrow_mut().drain(..) {
			self.expander.remove(&row);
		}
		let rules = self.rules.borrow().clone();
		self.expander.set_subtitle(&match rules.len() {
			0 => "None yet".to_string(),
			1 => "1 rule".to_string(),
			n => format!("{n} rules"),
		});
		let mut rows = Vec::new();
		for (i, rule) in rules.iter().enumerate() {
			let broken = parse(&rule.when).is_err();
			let row = adw::ActionRow::builder()
				.title(glib::markup_escape_text(&rule.when))
				.title_lines(2)
				.subtitle(if broken { "Does nothing until fixed".to_string() } else { rules::describe(&rule.action) })
				.activatable(true)
				.css_classes(["rule"])
				.build();
			row.add_suffix(&gtk::Image::from_icon_name("go-next-symbolic"));
			if broken {
				row.add_prefix(&gtk::Image::builder().icon_name("dialog-warning-symbolic").css_classes(["warning"]).build());
			}
			let weak = Rc::downgrade(self);
			row.connect_activated(move |_| {
				if let Some(this) = weak.upgrade() {
					this.edit(Some(i));
				}
			});
			self.expander.add_row(&row);
			rows.push(row.upcast());
		}
		let add = adw::ButtonRow::builder().title("Add Rule").start_icon_name("list-add-symbolic").build();
		let weak = Rc::downgrade(self);
		add.connect_activated(move |_| {
			if let Some(this) = weak.upgrade() {
				this.edit(None);
			}
		});
		self.expander.add_row(&add);
		rows.push(add.upcast());
		*self.rows.borrow_mut() = rows;
	}

	fn store(self: &Rc<Self>, rules: Vec<Rule>) {
		rules::save(&rules);
		*self.rules.borrow_mut() = rules;
		self.fill();
	}

	/// Deletes rule `i`, from its page.
	pub fn delete(self: &Rc<Self>, i: usize) {
		let mut all = self.rules.borrow().clone();
		if i < all.len() {
			all.remove(i);
			self.store(all);
		}
		self.dialog.pop_subpage();
	}

	/// Opens rule `i`, or a new one, on a page of its own.
	pub fn edit(self: &Rc<Self>, i: Option<usize>) -> Rc<Page> {
		let rule = i.and_then(|i| self.rules.borrow().get(i).cloned());
		let page = Page::new(self, i, rule);
		self.dialog.push_subpage(&page.page);
		page.line.grab_focus();
		page
	}
}

/// Mails the page lists from those a rule catches; the rest are counted.
const PREVIEW: usize = 4;

/// What the buttons under a rule's line put in it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Piece {
	Field(&'static str),
	Word(&'static str),
	Brackets,
}

/// `line` with `piece` put in at char `cursor` (around the chars `selected`, for brackets), spaced
/// from its neighbours, and where the cursor goes after it.
fn insert(line: &str, cursor: usize, selected: Option<(usize, usize)>, piece: Piece) -> (String, usize) {
	let chars: Vec<char> = line.chars().collect();
	let (from, to) = selected.filter(|(a, b)| a < b && *b <= chars.len()).unwrap_or((cursor.min(chars.len()), cursor.min(chars.len())));
	let before: String = chars[..from].iter().collect();
	let inside: String = chars[from..to].iter().collect();
	let after: String = chars[to..].iter().collect();
	let gap_before = if before.is_empty() || before.ends_with(char::is_whitespace) || before.ends_with('(') { "" } else { " " };
	let gap_after = if after.is_empty() || after.starts_with(char::is_whitespace) || after.starts_with(')') { "" } else { " " };
	let (text, back) = match piece {
		// The cursor stays after the colon, ready for what to look for.
		Piece::Field(name) => (format!("{gap_before}{name}:"), 0),
		Piece::Word(word) => (format!("{gap_before}{word} "), 0),
		Piece::Brackets if inside.is_empty() => (format!("{gap_before}()"), 1),
		Piece::Brackets => (format!("({inside}){gap_after}"), 0),
	};
	let at = before.chars().count() + text.chars().count() - back;
	(format!("{before}{text}{after}"), at)
}

/// The page editing one rule.
pub struct Page {
	ctx: Ctx,
	dialog: adw::PreferencesDialog,
	page: adw::NavigationPage,
	line: gtk::TextView,
	/// What the line catches: a message, then up to [`PREVIEW`] mails and how many more.
	preview: gtk::ListBox,
	does: [gtk::ToggleButton; 3],
	pub folder: adw::ComboRow,
	pub label: adw::ComboRow,
	/// The name of the label to make, when the label chosen is New Label.
	pub new_label: adw::EntryRow,
	pub save: gtk::Button,
	folders: Vec<Folder>,
	/// The account's labels, as the label choices list them before New Label.
	labels: RefCell<Vec<String>>,
	/// The count of mail caught, waiting for typing to pause.
	pending: RefCell<Option<glib::SourceId>>,
}

impl Page {
	fn new(rules: &Rc<Rules>, i: Option<usize>, rule: Option<Rule>) -> Rc<Page> {
		let ctx = rules.ctx.clone();
		// Mail can move to any folder but the ones it only gets into by being written.
		let folders: Vec<Folder> = ctx
			.cache()
			.and_then(|c| c.folders().ok())
			.unwrap_or_default()
			.into_iter()
			.filter(|f| !matches!(f.kind, FolderKind::Drafts | FolderKind::Sent | FolderKind::Spam))
			.collect();

		let line = gtk::TextView::builder()
			.wrap_mode(gtk::WrapMode::WordChar)
			.accepts_tab(false)
			.top_margin(12)
			.bottom_margin(12)
			.left_margin(14)
			.right_margin(14)
			.overflow(gtk::Overflow::Hidden)
			.css_classes(["rule-line", "monospace"])
			.build();
		line.update_property(&[gtk::accessible::Property::Label("Mail like")]);
		// Its height is worked out from text set before it showed; work it out again once shown.
		line.connect_map(|line| {
			let line = line.clone();
			glib::idle_add_local_once(move || line.queue_resize());
		});
		for tag in ["rule-from", "rule-to", "rule-subject", "rule-operator", "rule-bracket"] {
			line.buffer().create_tag(Some(tag), &[]);
		}
		let help = gtk::Label::builder().label(HELP).wrap(true).max_width_chars(44).xalign(0.0).margin_top(6).margin_bottom(6).margin_start(6).margin_end(6).build();
		let help_button = gtk::MenuButton::builder().label("How to Write a Rule").css_classes(["flat", "caption"]).popover(&gtk::Popover::builder().child(&help).build()).build();
		let chips = gtk::FlowBox::builder().selection_mode(gtk::SelectionMode::None).column_spacing(6).row_spacing(6).max_children_per_line(8).margin_top(10).build();
		let preview = gtk::ListBox::builder().selection_mode(gtk::SelectionMode::None).css_classes(["boxed-list"]).margin_top(12).build();
		let which = adw::PreferencesGroup::builder().title("Which Mail").header_suffix(&help_button).build();
		which.add(&line);
		which.add(&chips);
		which.add(&preview);

		let does = DOES.map(|label| gtk::ToggleButton::builder().label(label).hexpand(true).build());
		for b in &does[1..] {
			b.set_group(Some(&does[0]));
		}
		let switcher = gtk::Box::builder().css_classes(["linked", "rule-does"]).homogeneous(true).build();
		for b in &does {
			switcher.append(b);
		}
		let names: Vec<&str> = folders.iter().map(|f| f.name.as_str()).collect();
		let folder = adw::ComboRow::builder().title("Folder").model(&gtk::StringList::new(&names)).build();
		let label = adw::ComboRow::builder().title("Label").model(&gtk::StringList::new(&[NEW_LABEL])).build();
		let new_label = adw::EntryRow::builder().title("New Label Name").build();
		let then = adw::PreferencesGroup::builder().title("What Happens").build();
		then.add(&switcher);
		let target = adw::PreferencesGroup::new();
		target.add(&folder);
		target.add(&label);
		target.add(&new_label);

		let save = gtk::Button::builder().label("Save").css_classes(["suggested-action"]).build();
		let header = adw::HeaderBar::new();
		header.pack_end(&save);
		let content = adw::PreferencesPage::new();
		content.add(&which);
		content.add(&then);
		content.add(&target);
		if let Some(i) = i {
			let delete = gtk::Button::builder()
				.child(&adw::ButtonContent::builder().icon_name("user-trash-symbolic").label("Delete Rule").build())
				.css_classes(["delete-rule"])
				.build();
			let group = adw::PreferencesGroup::new();
			group.add(&delete);
			content.add(&group);
			let weak = Rc::downgrade(rules);
			delete.connect_clicked(move |_| {
				if let Some(rules) = weak.upgrade() {
					rules.delete(i);
				}
			});
		}
		let view = adw::ToolbarView::new();
		view.add_top_bar(&header);
		view.set_content(Some(&content));
		let page = adw::NavigationPage::builder().title(if i.is_some() { "Edit Rule" } else { "New Rule" }).child(&view).build();

		let this = Rc::new(Page {
			ctx,
			dialog: rules.dialog.clone(),
			page,
			line,
			preview,
			does,
			folder,
			label,
			new_label,
			save,
			folders,
			labels: RefCell::default(),
			pending: RefCell::default(),
		});
		for (text, piece, class) in [
			("+ From", Piece::Field("from"), "rule-from"),
			("+ To", Piece::Field("to"), "rule-to"),
			("+ Subject", Piece::Field("subject"), "rule-subject"),
			("and", Piece::Word("and"), ""),
			("or", Piece::Word("or"), ""),
			("not", Piece::Word("not"), ""),
			("( … )", Piece::Brackets, ""),
		] {
			let chip = gtk::Button::builder().label(text).css_classes(["pill", "rule-chip"]).build();
			if !class.is_empty() {
				chip.add_css_class(class);
			}
			let weak = Rc::downgrade(&this);
			chip.connect_clicked(move |_| {
				if let Some(this) = weak.upgrade() {
					this.put(piece);
				}
			});
			chips.append(&chip);
		}
		this.set_line(rule.as_ref().map_or("", |r| r.when.as_str()));
		match rule.as_ref().map(|r| &r.action) {
			None | Some(Action::Spam) => this.set_does(0),
			Some(Action::Move(name)) => {
				this.set_does(1);
				if let Some(at) = this.folders.iter().position(|f| f.name.eq_ignore_ascii_case(name)) {
					this.folder.set_selected(at as u32);
				}
			},
			Some(Action::Label(_)) => this.set_does(2),
		}
		this.load_labels(rule.and_then(|r| match r.action {
			Action::Label(name) => Some(name),
			_ => None,
		}));
		for b in &this.does {
			let weak = Rc::downgrade(&this);
			b.connect_toggled(move |_| {
				if let Some(this) = weak.upgrade() {
					this.update();
				}
			});
		}
		for row in [&this.folder, &this.label] {
			let weak = Rc::downgrade(&this);
			row.connect_selected_notify(move |_| {
				if let Some(this) = weak.upgrade() {
					this.update();
				}
			});
		}
		// One line: Enter saves instead of breaking it.
		let keys = gtk::EventControllerKey::new();
		let weak = Rc::downgrade(&this);
		keys.connect_key_pressed(move |_, key, _, _| {
			if !matches!(key, gtk::gdk::Key::Return | gtk::gdk::Key::KP_Enter) {
				return glib::Propagation::Proceed;
			}
			if let Some(this) = weak.upgrade() {
				if this.save.is_sensitive() {
					this.save.emit_clicked();
				}
			}
			glib::Propagation::Stop
		});
		this.line.add_controller(keys);
		let weak = Rc::downgrade(&this);
		this.line.buffer().connect_changed(move |buffer| {
			let Some(this) = weak.upgrade() else { return };
			let text = buffer.text(&buffer.start_iter(), &buffer.end_iter(), false);
			if text.contains(['\n', '\r']) {
				// Pasted lines join into one.
				buffer.set_text(&text.replace(['\n', '\r'], " "));
				return;
			}
			this.colour();
			this.update();
			this.count_soon();
		});
		let weak = Rc::downgrade(&this);
		adw::StyleManager::default().connect_dark_notify(move |_| {
			if let Some(this) = weak.upgrade() {
				this.colour();
			}
		});
		let weak = Rc::downgrade(&this);
		this.new_label.connect_changed(move |_| {
			if let Some(this) = weak.upgrade() {
				this.update();
			}
		});
		let (weak, rules) = (Rc::downgrade(&this), Rc::downgrade(rules));
		this.save.connect_clicked(move |_| {
			let (Some(this), Some(rules)) = (weak.upgrade(), rules.upgrade()) else { return };
			this.save(&rules, i);
		});
		this.colour();
		this.update();
		this.count();
		this
	}

	/// The rule's line as typed.
	pub fn line(&self) -> String {
		let buffer = self.line.buffer();
		buffer.text(&buffer.start_iter(), &buffer.end_iter(), false).to_string()
	}

	/// For the screenshot scenes: how tall the line shows, in pixels.
	pub fn line_height(&self) -> i32 {
		self.line.height()
	}

	pub fn set_line(&self, text: &str) {
		self.line.buffer().set_text(text);
	}

	/// What the rule does: 0 Spam, 1 a folder, 2 a label.
	pub fn does(&self) -> usize {
		self.does.iter().position(|b| b.is_active()).unwrap_or(0)
	}

	pub fn set_does(&self, i: usize) {
		if let Some(b) = self.does.get(i) {
			b.set_active(true);
		}
	}

	/// Puts what a button under the line stands for in it, at the cursor.
	pub fn put(&self, piece: Piece) {
		let buffer = self.line.buffer();
		let cursor = buffer.iter_at_mark(&buffer.get_insert()).offset() as usize;
		let selected = buffer.selection_bounds().map(|(a, b)| (a.offset() as usize, b.offset() as usize));
		let (text, at) = insert(&self.line(), cursor, selected, piece);
		buffer.set_text(&text);
		buffer.place_cursor(&buffer.iter_at_offset(at as i32));
		self.line.grab_focus();
	}

	/// Colours the line's fields, operators and brackets.
	fn colour(&self) {
		let buffer = self.line.buffer();
		let dark = adw::StyleManager::default().is_dark();
		let table = buffer.tag_table();
		let set = |name: &str, colour: &str, italic: bool| {
			if let Some(tag) = table.lookup(name) {
				tag.set_foreground(Some(colour));
				tag.set_weight(if italic { 400 } else { 700 });
				tag.set_style(if italic { gtk::pango::Style::Italic } else { gtk::pango::Style::Normal });
			}
		};
		set("rule-from", if dark { "#78aeed" } else { "#1c71d8" }, false);
		set("rule-to", if dark { "#dc8add" } else { "#9141ac" }, false);
		set("rule-subject", if dark { "#ffa348" } else { "#c64600" }, false);
		set("rule-operator", if dark { "#9a9a9e" } else { "#77767b" }, true);
		set("rule-bracket", if dark { "#9a9a9e" } else { "#77767b" }, false);
		buffer.remove_all_tags(&buffer.start_iter(), &buffer.end_iter());
		for (range, part) in parts(&self.line()) {
			let tag = match part {
				Part::Field(Field::From) => "rule-from",
				Part::Field(Field::To) => "rule-to",
				Part::Field(Field::Subject | Field::Any) => "rule-subject",
				Part::Operator => "rule-operator",
				Part::Bracket => "rule-bracket",
				Part::Value => continue,
			};
			buffer.apply_tag_by_name(tag, &buffer.iter_at_offset(range.start as i32), &buffer.iter_at_offset(range.end as i32));
		}
	}

	fn count_soon(self: &Rc<Self>) {
		if let Some(id) = self.pending.borrow_mut().take() {
			id.remove();
		}
		let weak = Rc::downgrade(self);
		let id = glib::timeout_add_local_once(Duration::from_millis(150), move || {
			if let Some(this) = weak.upgrade() {
				this.pending.borrow_mut().take();
				this.count();
			}
		});
		*self.pending.borrow_mut() = Some(id);
	}

	/// Shows what the line catches among the mail here (the newest few, and how many in all), or
	/// why it does not read.
	fn count(self: &Rc<Self>) {
		self.preview.remove_all();
		let line = self.line();
		if line.trim().is_empty() {
			self.preview.set_visible(false);
			return;
		}
		self.preview.set_visible(true);
		let (icon, class, message, found, total) = match parse(&line) {
			Ok(expr) => {
				let skip = [FolderKind::Spam, FolderKind::Trash, FolderKind::Sent, FolderKind::Drafts];
				let (found, total) = self.ctx.cache().and_then(|c| c.matching(&expr, &skip, PREVIEW).ok()).unwrap_or_default();
				match total {
					0 => ("dialog-information-symbolic", "dim-label", "No mail here matches yet. The rule will sort new mail that does.".to_string(), found, total),
					1 => ("object-select-symbolic", "success", "Catches 1 mail here already. Rules only sort new mail.".to_string(), found, total),
					n => ("object-select-symbolic", "success", format!("Catches {n} mails here already. Rules only sort new mail."), found, total),
				}
			},
			Err(e) => ("dialog-warning-symbolic", "error", e.message, Vec::new(), 0),
		};
		let head = gtk::Box::builder().spacing(8).margin_top(10).margin_bottom(10).margin_start(12).margin_end(12).css_classes([class]).build();
		head.append(&gtk::Image::from_icon_name(icon));
		head.append(&gtk::Label::builder().label(&message).wrap(true).xalign(0.0).hexpand(true).css_classes(["caption"]).build());
		self.preview.append(&gtk::ListBoxRow::builder().child(&head).activatable(false).build());
		for (mail, _) in &found {
			let row = gtk::Box::builder().spacing(12).margin_top(8).margin_bottom(8).margin_start(12).margin_end(12).build();
			let sender = if mail.from.display().is_empty() { "(Unknown Sender)" } else { mail.from.display() };
			row.append(&gtk::Label::builder().label(sender).xalign(0.0).width_chars(16).max_width_chars(16).ellipsize(gtk::pango::EllipsizeMode::End).css_classes(["caption"]).build());
			let subject = if mail.subject.trim().is_empty() { "(No Subject)" } else { mail.subject.as_str() };
			row.append(&gtk::Label::builder().label(subject).xalign(0.0).hexpand(true).ellipsize(gtk::pango::EllipsizeMode::End).css_classes(["caption", "dim-label"]).build());
			self.preview.append(&gtk::ListBoxRow::builder().child(&row).activatable(false).build());
		}
		if total > found.len() {
			let more = gtk::Box::builder().spacing(12).margin_top(4).margin_bottom(4).margin_start(12).margin_end(6).build();
			more.append(&gtk::Label::builder().label(format!("and {} more, older", total - found.len())).xalign(0.0).hexpand(true).css_classes(["caption", "dim-label"]).build());
			let show = gtk::Button::builder().label("Show All in Search").css_classes(["flat", "caption", "accent"]).build();
			let weak = Rc::downgrade(self);
			show.connect_clicked(move |_| {
				if let Some(this) = weak.upgrade() {
					this.show_all();
				}
			});
			more.append(&show);
			self.preview.append(&gtk::ListBoxRow::builder().child(&more).activatable(false).build());
		}
	}

	/// Lists everything the line catches in the main window's search, leaving Preferences.
	pub fn show_all(&self) {
		let line = self.line();
		self.dialog.close();
		self.ctx.show_rule(line.trim());
	}

	/// For the screenshot scenes: the preview's message, the mails it lists, and the line under them.
	pub fn preview(&self) -> (String, usize, Option<String>) {
		let texts: Vec<Vec<String>> = std::iter::successors(self.preview.first_child(), |w| w.next_sibling())
			.filter_map(|row| row.downcast::<gtk::ListBoxRow>().ok()?.child())
			.map(|b| std::iter::successors(b.first_child(), |w| w.next_sibling()).filter_map(|w| w.downcast::<gtk::Label>().ok()).map(|l| l.label().to_string()).collect())
			.collect();
		let message = texts.first().and_then(|t| t.first().cloned()).unwrap_or_default();
		let more = texts.last().filter(|t| texts.len() > 1 && t.first().is_some_and(|l| l.starts_with("and "))).and_then(|t| t.first().cloned());
		let mails = texts.len().saturating_sub(1) - usize::from(more.is_some());
		(message, mails, more)
	}

	/// Keeps the rule as rule `i`, or a new one, after making its label if it is a new one.
	fn save(self: &Rc<Self>, rules: &Rc<Rules>, i: Option<usize>) {
		let Some(rule) = self.rule() else { return };
		let store = {
			let (rules, rule) = (rules.clone(), rule.clone());
			move || {
				let mut all = rules.rules.borrow().clone();
				match i {
					Some(i) if i < all.len() => all[i] = rule,
					_ => all.push(rule),
				}
				rules.store(all);
				rules.dialog.pop_subpage();
			}
		};
		let make = match &rule.action {
			Action::Label(name) if !self.labels.borrow().iter().any(|l| l.eq_ignore_ascii_case(name)) => name.clone(),
			_ => return store(),
		};
		let Some(backend) = self.ctx.backend() else { return };
		self.save.set_sensitive(false);
		let (weak, ctx) = (Rc::downgrade(self), self.ctx.clone());
		runtime::spawn_local(async move {
			let made = runtime::run(async move { backend.create_label(&make).await }).await;
			let Some(this) = weak.upgrade() else { return };
			match made {
				Ok(label) => {
					this.labels.borrow_mut().push(label.name);
					store();
				},
				Err(e) => {
					ctx.error(&e);
					this.update();
				},
			}
		});
	}

	/// The account's labels, fetched as the page opens; `keep` is selected once they are in.
	fn load_labels(self: &Rc<Self>, keep: Option<String>) {
		let Some(backend) = self.ctx.backend() else { return };
		let weak = Rc::downgrade(self);
		runtime::spawn_local(async move {
			let labels = runtime::run(async move { backend.labels().await }).await;
			let Some(this) = weak.upgrade() else { return };
			let mut names: Vec<String> = labels.unwrap_or_default().into_iter().map(|l| l.name).collect();
			// A label the rule names that is gone stays offered, so saving does not change it.
			if let Some(keep) = keep.as_ref().filter(|k| !names.iter().any(|n| n.eq_ignore_ascii_case(k))) {
				names.push(keep.clone());
			}
			let choices: Vec<&str> = names.iter().map(String::as_str).chain([NEW_LABEL]).collect();
			this.label.set_model(Some(&gtk::StringList::new(&choices)));
			if let Some(at) = keep.and_then(|k| names.iter().position(|n| n.eq_ignore_ascii_case(&k))) {
				this.label.set_selected(at as u32);
			}
			*this.labels.borrow_mut() = names;
			this.update();
		});
	}

	/// The rule as the page has it, if it is whole.
	fn rule(&self) -> Option<Rule> {
		let when = self.line().trim().to_string();
		parse(&when).ok()?;
		let action = match self.does() {
			0 => Action::Spam,
			1 => Action::Move(self.folders.get(self.folder.selected() as usize)?.name.clone()),
			_ => Action::Label(match self.labels.borrow().get(self.label.selected() as usize) {
				Some(name) => name.clone(),
				None => Some(self.new_label.text().trim().to_string()).filter(|n| !n.is_empty())?,
			}),
		};
		Some(Rule { when, action })
	}

	/// Shows which choices apply, and whether the rule can be saved.
	fn update(&self) {
		let does = self.does();
		self.folder.set_visible(does == 1);
		self.label.set_visible(does == 2);
		self.new_label.set_visible(does == 2 && self.label.selected() as usize >= self.labels.borrow().len());
		self.save.set_sensitive(self.rule().is_some());
	}
}

#[cfg(test)]
mod tests {
	use super::*;

	#[test]
	fn pieces_go_in_at_the_cursor_with_spaces_around() {
		assert_eq!(insert("", 0, None, Piece::Field("from")), ("from:".into(), 5));
		assert_eq!(insert("from:@a.org", 11, None, Piece::Word("or")), ("from:@a.org or ".into(), 15));
		assert_eq!(insert("from:@a.org or ", 15, None, Piece::Field("subject")), ("from:@a.org or subject:".into(), 23));
		assert_eq!(insert("lunch", 0, None, Piece::Word("not")), ("not lunch".into(), 4));
		assert_eq!(insert("a", 1, None, Piece::Brackets), ("a ()".into(), 3), "empty brackets put the cursor inside");
		assert_eq!(insert("a or b c", 0, Some((0, 6)), Piece::Brackets), ("(a or b) c".into(), 8), "a selection is wrapped");
		assert_eq!(insert("x", 0, Some((0, 1)), Piece::Brackets), ("(x)".into(), 3));
		assert_eq!(insert("é", 1, None, Piece::Field("to")), ("é to:".into(), 5), "counted in chars");
	}
}

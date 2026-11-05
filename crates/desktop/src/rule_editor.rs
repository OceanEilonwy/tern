//! The Rules section of Preferences, drawn from what the core publishes: the rules in a row that opens
//! up, each edited on a page of its own with the mail it would catch.

use std::cell::{Cell, RefCell};
use std::rc::{Rc, Weak};

use adw::prelude::*;
use gtk::glib;
use tern_app::view::{Action, LinePart, PreviewTone, RuleDoes, RuleEditorView, RulePiece, RulesView};

use crate::window::Ctx;

const HELP: &str = "from:, to: and subject: look at the sender, the first recipient and the subject; a word on its own looks at the subject and the sender. @example.com is a whole domain. Join terms with and, or and not, and group them in brackets: (from:@example.com or subject:\"offer\") and not to:me@tuta.com";

/// What a rule can do, in the order offered.
const DOES: [(&str, RuleDoes); 3] = [("Move to Spam", RuleDoes::Spam), ("Move to Folder", RuleDoes::Folder), ("Add Label", RuleDoes::Label)];
/// The last choice of label, which makes one.
const NEW_LABEL: &str = "New Label…";

pub struct Rules {
	ctx: Ctx,
	dialog: adw::PreferencesDialog,
	pub group: adw::PreferencesGroup,
	pub expander: adw::ExpanderRow,
	rows: RefCell<Vec<gtk::Widget>>,
	/// The rule open on a page, if any.
	page: RefCell<Option<Rc<Page>>>,
}

impl Rules {
	pub fn new(ctx: &Ctx, dialog: &adw::PreferencesDialog) -> Rc<Rules> {
		let group = adw::PreferencesGroup::builder().title("Rules").description("Rules sort new mail in the Inbox as it arrives.").build();
		let expander = adw::ExpanderRow::builder().title("Mail Rules").build();
		group.add(&expander);
		let this = Rc::new(Rules { ctx: ctx.clone(), dialog: dialog.clone(), group, expander, rows: RefCell::default(), page: RefCell::default() });
		let weak = Rc::downgrade(&this);
		ctx.connect_rules(move |view| {
			if let Some(this) = weak.upgrade() {
				this.fill(view);
			}
		});
		let weak = Rc::downgrade(&this);
		ctx.connect_rule_editor(move |view| {
			if let Some(this) = weak.upgrade() {
				this.show_editor(view);
			}
		});
		ctx.engine.send(Action::OpenSettings);
		ctx.engine.send(Action::OpenRules);
		this
	}

	/// One row per rule, then Add Rule.
	fn fill(self: &Rc<Self>, view: &RulesView) {
		for row in self.rows.borrow_mut().drain(..) {
			self.expander.remove(&row);
		}
		if let Some(settings) = self.ctx.settings_view() {
			self.expander.set_subtitle(&settings.rules_line);
		}
		let mut rows = Vec::new();
		for rule in &view.rows {
			let row = adw::ActionRow::builder().title(glib::markup_escape_text(&rule.when)).title_lines(2).subtitle(&rule.does).activatable(true).css_classes(["rule"]).build();
			row.add_suffix(&gtk::Image::from_icon_name("go-next-symbolic"));
			if rule.broken {
				row.add_prefix(&gtk::Image::builder().icon_name("dialog-warning-symbolic").css_classes(["warning"]).build());
			}
			let (weak, index) = (Rc::downgrade(self), rule.index);
			row.connect_activated(move |_| {
				if let Some(this) = weak.upgrade() {
					this.edit(Some(index));
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

	fn show_editor(&self, view: Option<&RuleEditorView>) {
		match view {
			Some(view) => {
				if let Some(page) = self.page.borrow().as_ref() {
					page.show(view);
				}
			},
			// Saved or deleted, so the core has finished with it.
			None => {
				if self.page.borrow_mut().take().is_some() {
					self.dialog.pop_subpage();
				}
			},
		}
	}

	/// Opens rule `index`, or a new one, on a page of its own.
	pub fn edit(self: &Rc<Self>, index: Option<u32>) -> Rc<Page> {
		let page = Page::new(self, index);
		*self.page.borrow_mut() = Some(page.clone());
		self.dialog.push_subpage(&page.page);
		page.line.grab_focus();
		self.ctx.engine.send(Action::OpenRule { index });
		page
	}

	/// The back button left the page without saving.
	fn left(&self, page: &Rc<Page>) {
		let open = self.page.borrow().as_ref().is_some_and(|p| Rc::ptr_eq(p, page));
		if open {
			self.page.borrow_mut().take();
			self.ctx.engine.send(Action::Back);
		}
	}
}

/// The page editing one rule.
pub struct Page {
	ctx: Ctx,
	dialog: adw::PreferencesDialog,
	page: adw::NavigationPage,
	line: gtk::TextView,
	/// What the line catches: a message, then the first few mails and how many more.
	preview: gtk::ListBox,
	does: [gtk::ToggleButton; 3],
	pub folder: adw::ComboRow,
	pub label: adw::ComboRow,
	/// The name of the label to make, when the label chosen is New Label.
	pub new_label: adw::EntryRow,
	pub save: gtk::Button,
	delete: gtk::Button,
	delete_group: adw::PreferencesGroup,
	/// What the core last said.
	shown: RefCell<Option<RuleEditorView>>,
	/// The choices as listed, New Label not counted, to see which is which.
	folders: RefCell<Vec<String>>,
	labels: RefCell<Vec<String>>,
	/// New Label is chosen: the label named is the one being typed.
	naming: Cell<bool>,
	/// Drawing what the core said, which is not the person doing anything.
	drawing: Cell<bool>,
}

impl Page {
	fn new(rules: &Rc<Rules>, index: Option<u32>) -> Rc<Page> {
		let ctx = rules.ctx.clone();
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

		let does = DOES.map(|(label, _)| gtk::ToggleButton::builder().label(label).hexpand(true).build());
		for b in &does[1..] {
			b.set_group(Some(&does[0]));
		}
		let switcher = gtk::Box::builder().css_classes(["linked", "rule-does"]).homogeneous(true).build();
		for b in &does {
			switcher.append(b);
		}
		let folder = adw::ComboRow::builder().title("Folder").model(&gtk::StringList::new(&[])).build();
		let label = adw::ComboRow::builder().title("Label").model(&gtk::StringList::new(&[NEW_LABEL])).build();
		let new_label = adw::EntryRow::builder().title("New Label Name").build();
		let then = adw::PreferencesGroup::builder().title("What Happens").build();
		then.add(&switcher);
		let target = adw::PreferencesGroup::new();
		target.add(&folder);
		target.add(&label);
		target.add(&new_label);

		let save = gtk::Button::builder().label("Save").css_classes(["suggested-action"]).sensitive(false).build();
		let header = adw::HeaderBar::new();
		header.pack_end(&save);
		let content = adw::PreferencesPage::new();
		content.add(&which);
		content.add(&then);
		content.add(&target);
		let delete = gtk::Button::builder().child(&adw::ButtonContent::builder().icon_name("user-trash-symbolic").label("Delete Rule").build()).css_classes(["delete-rule"]).build();
		let delete_group = adw::PreferencesGroup::builder().visible(index.is_some()).build();
		delete_group.add(&delete);
		content.add(&delete_group);
		let view = adw::ToolbarView::new();
		view.add_top_bar(&header);
		view.set_content(Some(&content));
		let page = adw::NavigationPage::builder().title(if index.is_some() { "Edit Rule" } else { "New Rule" }).child(&view).build();

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
			delete,
			delete_group,
			shown: RefCell::default(),
			folders: RefCell::default(),
			labels: RefCell::default(),
			naming: Cell::new(false),
			drawing: Cell::new(false),
		});
		for (text, piece, class) in [
			("+ From", RulePiece::From, "rule-from"),
			("+ To", RulePiece::To, "rule-to"),
			("+ Subject", RulePiece::Subject, "rule-subject"),
			("and", RulePiece::And, ""),
			("or", RulePiece::Or, ""),
			("not", RulePiece::Not, ""),
			("( … )", RulePiece::Brackets, ""),
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
		for (b, (_, does)) in this.does.iter().zip(DOES) {
			let weak = Rc::downgrade(&this);
			b.connect_toggled(move |b| {
				let Some(this) = weak.upgrade() else { return };
				if b.is_active() && !this.drawing.get() {
					this.ctx.engine.send(Action::RuleSetDoes { does });
				}
			});
		}
		let weak = Rc::downgrade(&this);
		this.folder.connect_selected_notify(move |row| {
			let Some(this) = weak.upgrade() else { return };
			if !this.drawing.get() {
				if let Some(name) = this.folders.borrow().get(row.selected() as usize) {
					this.ctx.engine.send(Action::RuleSetFolder { name: name.clone() });
				}
			}
		});
		let weak = Rc::downgrade(&this);
		this.label.connect_selected_notify(move |_| {
			if let Some(this) = weak.upgrade().filter(|this| !this.drawing.get()) {
				this.label_chosen();
			}
		});
		let weak = Rc::downgrade(&this);
		this.new_label.connect_changed(move |_| {
			if let Some(this) = weak.upgrade().filter(|this| !this.drawing.get() && this.naming.get()) {
				this.ctx.engine.send(Action::RuleSetLabel { name: this.new_label.text().to_string() });
			}
		});
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
			if this.drawing.get() {
				return;
			}
			let text = buffer.text(&buffer.start_iter(), &buffer.end_iter(), false);
			if text.contains(['\n', '\r']) {
				// Pasted lines join into one.
				buffer.set_text(&text.replace(['\n', '\r'], " "));
				return;
			}
			this.ctx.engine.send(Action::RuleLine { line: text.to_string() });
		});
		let weak = Rc::downgrade(&this);
		adw::StyleManager::default().connect_dark_notify(move |_| {
			if let Some(this) = weak.upgrade() {
				this.colour();
			}
		});
		let engine = this.ctx.engine.clone();
		this.save.connect_clicked(move |_| engine.send(Action::RuleSave));
		let engine = this.ctx.engine.clone();
		this.delete.connect_clicked(move |_| engine.send(Action::RuleDelete));
		let (rules, weak) = (Rc::downgrade(rules), Rc::downgrade(&this));
		this.page.connect_hiding(move |_| {
			if let (Some(rules), Some(this)) = (Weak::upgrade(&rules), weak.upgrade()) {
				rules.left(&this);
			}
		});
		this
	}

	/// Draws what the core says about this rule.
	fn show(&self, view: &RuleEditorView) {
		self.drawing.set(true);
		let before = self.shown.borrow_mut().replace(view.clone());
		// The line is the person's while they type; the core only sets it when it changes it itself.
		if before.as_ref().map(|b| b.revision) != Some(view.revision) {
			let buffer = self.line.buffer();
			buffer.set_text(&view.line);
			buffer.place_cursor(&buffer.iter_at_offset(view.cursor as i32));
		}
		self.colour();
		self.delete_group.set_visible(view.editing);
		self.page.set_title(if view.editing { "Edit Rule" } else { "New Rule" });
		self.draw_preview(view);
		for (b, (_, does)) in self.does.iter().zip(DOES) {
			if does == view.does {
				b.set_active(true);
			}
		}
		self.draw_choices(view);
		self.folder.set_visible(view.does == RuleDoes::Folder);
		self.label.set_visible(view.does == RuleDoes::Label);
		self.new_label.set_visible(view.does == RuleDoes::Label && self.naming.get());
		self.save.set_sensitive(view.can_save);
		self.drawing.set(false);
	}

	fn draw_choices(&self, view: &RuleEditorView) {
		// A choice the rule names that has gone stays offered, so saving does not change it.
		let with = |names: &[String], named: &str| {
			let mut all = names.to_vec();
			if !named.is_empty() && !all.iter().any(|n| n.eq_ignore_ascii_case(named)) {
				all.push(named.to_string());
			}
			all
		};
		let folders = with(&view.folders, &view.folder);
		if *self.folders.borrow() != folders {
			let names: Vec<&str> = folders.iter().map(String::as_str).collect();
			self.folder.set_model(Some(&gtk::StringList::new(&names)));
			*self.folders.borrow_mut() = folders;
		}
		if let Some(at) = self.folders.borrow().iter().position(|n| n.eq_ignore_ascii_case(&view.folder)) {
			self.folder.set_selected(at as u32);
		}
		let labels = if self.naming.get() { view.labels.clone() } else { with(&view.labels, &view.label) };
		if *self.labels.borrow() != labels {
			let names: Vec<&str> = labels.iter().map(String::as_str).chain([NEW_LABEL]).collect();
			self.label.set_model(Some(&gtk::StringList::new(&names)));
			*self.labels.borrow_mut() = labels;
		}
		let chosen = if self.naming.get() { None } else { self.labels.borrow().iter().position(|n| n.eq_ignore_ascii_case(&view.label)) };
		self.label.set_selected(chosen.unwrap_or(self.labels.borrow().len()) as u32);
		if chosen.is_none() && !self.naming.get() {
			self.naming.set(true);
		}
	}

	/// A label, or New Label, was chosen from the list.
	fn label_chosen(&self) {
		let name = self.labels.borrow().get(self.label.selected() as usize).cloned();
		self.naming.set(name.is_none());
		self.new_label.set_visible(name.is_none());
		self.ctx.engine.send(Action::RuleSetLabel { name: name.unwrap_or_else(|| self.new_label.text().to_string()) });
	}

	/// The core has drawn this rule at least once, so the line and its choices are what it says.
	pub fn is_ready(&self) -> bool {
		self.shown.borrow().is_some()
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
	pub fn put(&self, piece: RulePiece) {
		let buffer = self.line.buffer();
		let cursor = buffer.iter_at_mark(&buffer.get_insert()).offset() as u32;
		let (from, to) = buffer.selection_bounds().map_or((cursor, cursor), |(a, b)| (a.offset() as u32, b.offset() as u32));
		self.ctx.engine.send(Action::RulePut { piece, cursor: from, selection_end: to });
		self.line.grab_focus();
	}

	/// Colours the line's fields, operators and brackets, as the core has read them.
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
		let shown = self.shown.borrow();
		// What it read is of the line as it was: colours wait for the core to catch up with typing.
		let Some(view) = shown.as_ref().filter(|v| v.line == self.line()) else { return };
		for span in &view.spans {
			let tag = match span.part {
				LinePart::From => "rule-from",
				LinePart::To => "rule-to",
				LinePart::Subject => "rule-subject",
				LinePart::Operator => "rule-operator",
				LinePart::Bracket => "rule-bracket",
				LinePart::Value => continue,
			};
			buffer.apply_tag_by_name(tag, &buffer.iter_at_offset(span.start as i32), &buffer.iter_at_offset(span.end as i32));
		}
	}

	/// Shows what the line catches among the mail here, or why it does not read.
	fn draw_preview(&self, view: &RuleEditorView) {
		self.preview.remove_all();
		let (icon, class) = match view.tone {
			PreviewTone::Neutral => ("dialog-information-symbolic", "dim-label"),
			PreviewTone::Good => ("object-select-symbolic", "success"),
			PreviewTone::Bad => ("dialog-warning-symbolic", "error"),
		};
		let head = gtk::Box::builder().spacing(8).margin_top(10).margin_bottom(10).margin_start(12).margin_end(12).css_classes([class]).build();
		head.append(&gtk::Image::from_icon_name(icon));
		head.append(&gtk::Label::builder().label(&view.message).wrap(true).xalign(0.0).hexpand(true).css_classes(["caption"]).build());
		self.preview.append(&gtk::ListBoxRow::builder().child(&head).activatable(false).build());
		for mail in &view.preview {
			let row = gtk::Box::builder().spacing(12).margin_top(8).margin_bottom(8).margin_start(12).margin_end(12).build();
			row.append(&gtk::Label::builder().label(&mail.who).xalign(0.0).width_chars(16).max_width_chars(16).ellipsize(gtk::pango::EllipsizeMode::End).css_classes(["caption"]).build());
			row.append(&gtk::Label::builder().label(&mail.subject).xalign(0.0).hexpand(true).ellipsize(gtk::pango::EllipsizeMode::End).css_classes(["caption", "dim-label"]).build());
			self.preview.append(&gtk::ListBoxRow::builder().child(&row).activatable(false).build());
		}
		if let Some(more) = &view.more {
			let row = gtk::Box::builder().spacing(12).margin_top(4).margin_bottom(4).margin_start(12).margin_end(6).build();
			row.append(&gtk::Label::builder().label(more).xalign(0.0).hexpand(true).css_classes(["caption", "dim-label"]).build());
			let show = gtk::Button::builder().label("Show All in Search").css_classes(["flat", "caption", "accent"]).build();
			let (engine, dialog) = (self.ctx.engine.clone(), self.dialog.clone());
			show.connect_clicked(move |_| {
				engine.send(Action::RuleInSearch);
				dialog.close();
			});
			row.append(&show);
			self.preview.append(&gtk::ListBoxRow::builder().child(&row).activatable(false).build());
		}
	}

	/// Lists everything the line catches in the main window's search, leaving Preferences.
	pub fn show_all(&self) {
		self.ctx.engine.send(Action::RuleInSearch);
		self.dialog.close();
	}

	/// Deletes this rule.
	pub fn delete(&self) {
		self.delete.emit_clicked();
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
}

//! The Rules section of Preferences, drawn from what the core publishes: the rules in a row that opens
//! up, each edited on a page of its own with the mail it would catch.

use std::cell::Cell;
use std::rc::Rc;

use adw::prelude::*;
use gtk::glib;
use relm4::prelude::*;
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
	rows: Vec<gtk::Widget>,
	/// The rule open on a page, if any.
	page: Option<Rc<Page>>,
	/// How many pages have been opened: the number of the newest.
	opened: u64,
}

#[derive(Debug)]
pub enum Input {
	View(RulesView),
	Editor(Option<Box<RuleEditorView>>),
	/// Opens rule `index`, or a new one, on a page of its own.
	Edit(Option<u32>),
	/// The page numbered this was left.
	Left(u64),
}

#[relm4::component(pub)]
impl Component for Rules {
	type Init = (Ctx, adw::PreferencesDialog);
	type Input = Input;
	type Output = ();
	type CommandOutput = ();

	view! {
		adw::PreferencesGroup {
			set_title: "Rules",
			set_description: Some("Rules sort new mail in the Inbox as it arrives."),
			#[name = "expander"]
			add = &adw::ExpanderRow {
				set_title: "Mail Rules",
			},
		}
	}

	fn init(init: Self::Init, root: Self::Root, sender: ComponentSender<Self>) -> ComponentParts<Self> {
		let (ctx, dialog) = init;
		let mut model = Rules { ctx, dialog, rows: Vec::new(), page: None, opened: 0 };
		let widgets = view_output!();
		if let Some(view) = model.ctx.rules_view() {
			model.fill(&widgets.expander, &view, &sender);
		}
		model.ctx.engine.send(Action::OpenSettings);
		model.ctx.engine.send(Action::OpenRules);
		ComponentParts { model, widgets }
	}

	fn update_with_view(&mut self, widgets: &mut Self::Widgets, input: Input, sender: ComponentSender<Self>, _: &Self::Root) {
		match input {
			Input::View(view) => self.fill(&widgets.expander, &view, &sender),
			Input::Editor(Some(view)) => {
				if let Some(page) = &self.page {
					page.controller.emit(EditorInput::View(view));
				}
			},
			// Saved or deleted, so the core has finished with it.
			Input::Editor(None) => {
				if self.page.take().is_some() {
					self.dialog.pop_subpage();
				}
			},
			Input::Edit(index) => {
				self.opened += 1;
				let number = self.opened;
				let page = Editor::builder().launch((self.ctx.clone(), self.dialog.clone(), index.is_some())).forward(sender.input_sender(), move |_| Input::Left(number));
				self.dialog.push_subpage(page.widget());
				page.widgets().line.grab_focus();
				self.page = Some(Rc::new(Page::new(page, self.ctx.clone(), self.dialog.clone())));
				self.ctx.engine.send(Action::OpenRule { index });
			},
			// The back button left the page without saving.
			Input::Left(number) => {
				if number == self.opened && self.page.take().is_some() {
					self.ctx.engine.send(Action::Back);
				}
			},
		}
	}
}

impl Rules {
	/// One row per rule, then Add Rule.
	fn fill(&mut self, expander: &adw::ExpanderRow, view: &RulesView, sender: &ComponentSender<Self>) {
		for row in self.rows.drain(..) {
			expander.remove(&row);
		}
		if let Some(settings) = self.ctx.settings_view() {
			expander.set_subtitle(&settings.rules_line);
		}
		for rule in &view.rows {
			relm4::view! {
				row = adw::ActionRow {
					set_title: &glib::markup_escape_text(&rule.when),
					set_title_lines: 2,
					set_subtitle: &rule.does,
					set_activatable: true,
					add_css_class: "rule",
					add_suffix = &gtk::Image {
						set_icon_name: Some("go-next-symbolic"),
					},
					connect_activated[sender, index = rule.index] => move |_| sender.input(Input::Edit(Some(index))),
				}
			}
			if rule.broken {
				row.add_prefix(&gtk::Image::builder().icon_name("dialog-warning-symbolic").css_classes(["warning"]).build());
			}
			expander.add_row(&row);
			self.rows.push(row.upcast());
		}
		relm4::view! {
			add = adw::ButtonRow {
				set_title: "Add Rule",
				set_start_icon_name: Some("list-add-symbolic"),
				connect_activated[sender] => move |_| sender.input(Input::Edit(None)),
			}
		}
		expander.add_row(&add);
		self.rows.push(add.upcast());
	}

	/// For the screenshot scenes: how many pages have been opened.
	pub fn opened(&self) -> u64 {
		self.opened
	}

	/// For the screenshot scenes: the rule open on a page.
	pub fn page(&self) -> Option<Rc<Page>> {
		self.page.clone()
	}
}

/// The page editing one rule.
pub struct Editor {
	ctx: Ctx,
	dialog: adw::PreferencesDialog,
	/// What the core last said.
	shown: Option<RuleEditorView>,
	editing: bool,
	does: [gtk::ToggleButton; 3],
	/// The choices as listed, New Label not counted, to see which is which.
	folders: Vec<String>,
	labels: Vec<String>,
	/// New Label is chosen: the label named is the one being typed.
	naming: bool,
	/// Drawing what the core said, which is not the person doing anything. The signals it would set
	/// off run at once, so they look here.
	drawing: Rc<Cell<bool>>,
}

#[derive(Debug)]
pub enum EditorInput {
	View(Box<RuleEditorView>),
	FolderChosen(u32),
	LabelChosen(u32),
	NewLabel(String),
	/// Puts what a button under the line stands for in it, at the cursor.
	Put(RulePiece),
	Recolour,
}

/// The page was left, by whatever means.
#[derive(Debug)]
pub struct Left;

#[relm4::component(pub)]
impl Component for Editor {
	type Init = (Ctx, adw::PreferencesDialog, bool);
	type Input = EditorInput;
	type Output = Left;
	type CommandOutput = ();

	view! {
		adw::NavigationPage {
			#[watch]
			set_title: if model.editing { "Edit Rule" } else { "New Rule" },
			connect_hiding[sender] => move |_| {
				let _ = sender.output(Left);
			},
			#[wrap(Some)]
			set_child = &adw::ToolbarView {
				add_top_bar = &adw::HeaderBar {
					#[name = "save"]
					pack_end = &gtk::Button {
						set_label: "Save",
						add_css_class: "suggested-action",
						#[watch]
						set_sensitive: model.shown.as_ref().is_some_and(|v| v.can_save),
						connect_clicked[engine = model.ctx.engine.clone()] => move |_| engine.send(Action::RuleSave),
					},
				},
				#[wrap(Some)]
				set_content = &adw::PreferencesPage {
					add = &adw::PreferencesGroup {
						set_title: "Which Mail",
						#[wrap(Some)]
						set_header_suffix = &gtk::MenuButton {
							set_label: "How to Write a Rule",
							set_css_classes: &["flat", "caption"],
							#[wrap(Some)]
							set_popover = &gtk::Popover {
								#[wrap(Some)]
								set_child = &gtk::Label {
									set_label: HELP,
									set_wrap: true,
									set_max_width_chars: 44,
									set_xalign: 0.0,
									set_margin_top: 6,
									set_margin_bottom: 6,
									set_margin_start: 6,
									set_margin_end: 6,
								},
							},
						},
						#[name = "line"]
						add = &gtk::TextView {
							set_wrap_mode: gtk::WrapMode::WordChar,
							set_accepts_tab: false,
							set_top_margin: 12,
							set_bottom_margin: 12,
							set_left_margin: 14,
							set_right_margin: 14,
							set_overflow: gtk::Overflow::Hidden,
							set_css_classes: &["rule-line", "monospace"],
							update_property: &[gtk::accessible::Property::Label("Mail like")],
							// Its height is worked out from text set before it showed; work it out again once shown.
							connect_map => |line| {
								let line = line.clone();
								glib::idle_add_local_once(move || line.queue_resize());
							},
						},
						add: &chips,
						#[name = "preview"]
						add = &gtk::ListBox {
							set_selection_mode: gtk::SelectionMode::None,
							add_css_class: "boxed-list",
							set_margin_top: 12,
						},
					},
					add = &adw::PreferencesGroup {
						set_title: "What Happens",
						add: &switcher,
					},
					add = &adw::PreferencesGroup {
						#[name = "folder"]
						add = &adw::ComboRow {
							set_title: "Folder",
							set_model: Some(&gtk::StringList::new(&[])),
							#[watch]
							set_visible: model.does_now(RuleDoes::Folder),
							connect_selected_notify[sender, drawing = model.drawing.clone()] => move |row| {
								if !drawing.get() {
									sender.input(EditorInput::FolderChosen(row.selected()));
								}
							},
						},
						#[name = "label"]
						add = &adw::ComboRow {
							set_title: "Label",
							set_model: Some(&gtk::StringList::new(&[NEW_LABEL])),
							#[watch]
							set_visible: model.does_now(RuleDoes::Label),
							connect_selected_notify[sender, drawing = model.drawing.clone()] => move |row| {
								if !drawing.get() {
									sender.input(EditorInput::LabelChosen(row.selected()));
								}
							},
						},
						// The name of the label to make, when the label chosen is New Label.
						#[name = "new_label"]
						add = &adw::EntryRow {
							set_title: "New Label Name",
							#[watch]
							set_visible: model.does_now(RuleDoes::Label) && model.naming,
							connect_changed[sender, drawing = model.drawing.clone()] => move |row| {
								if !drawing.get() {
									sender.input(EditorInput::NewLabel(row.text().to_string()));
								}
							},
						},
					},
					add = &adw::PreferencesGroup {
						#[watch]
						set_visible: model.editing,
						#[name = "delete"]
						add = &gtk::Button {
							add_css_class: "delete-rule",
							connect_clicked[engine = model.ctx.engine.clone()] => move |_| engine.send(Action::RuleDelete),
							#[wrap(Some)]
							set_child = &adw::ButtonContent {
								set_icon_name: "user-trash-symbolic",
								set_label: "Delete Rule",
							},
						},
					},
				},
			},
		}
	}

	fn init(init: Self::Init, root: Self::Root, sender: ComponentSender<Self>) -> ComponentParts<Self> {
		let (ctx, dialog, editing) = init;
		let drawing: Rc<Cell<bool>> = Rc::default();
		let chips = gtk::FlowBox::builder().selection_mode(gtk::SelectionMode::None).column_spacing(6).row_spacing(6).max_children_per_line(8).margin_top(10).build();
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
			let sender = sender.clone();
			chip.connect_clicked(move |_| sender.input(EditorInput::Put(piece)));
			chips.append(&chip);
		}
		let does = DOES.map(|(label, does)| {
			let button = gtk::ToggleButton::builder().label(label).hexpand(true).build();
			let (engine, drawing) = (ctx.engine.clone(), drawing.clone());
			button.connect_toggled(move |b| {
				if b.is_active() && !drawing.get() {
					engine.send(Action::RuleSetDoes { does });
				}
			});
			button
		});
		let switcher = gtk::Box::builder().css_classes(["linked", "rule-does"]).homogeneous(true).build();
		for button in &does[1..] {
			button.set_group(Some(&does[0]));
		}
		for button in &does {
			switcher.append(button);
		}
		let model = Editor { ctx, dialog, shown: None, editing, does, folders: Vec::new(), labels: Vec::new(), naming: false, drawing };
		let widgets = view_output!();

		let buffer = widgets.line.buffer();
		for tag in ["rule-from", "rule-to", "rule-subject", "rule-operator", "rule-bracket"] {
			buffer.create_tag(Some(tag), &[]);
		}
		let (engine, drawing) = (model.ctx.engine.clone(), model.drawing.clone());
		buffer.connect_changed(move |buffer| {
			if drawing.get() {
				return;
			}
			let text = buffer.text(&buffer.start_iter(), &buffer.end_iter(), false);
			if text.contains(['\n', '\r']) {
				// Pasted lines join into one.
				buffer.set_text(&text.replace(['\n', '\r'], " "));
				return;
			}
			engine.send(Action::RuleLine { line: text.to_string() });
		});
		// One line: Enter saves instead of breaking it.
		let keys = gtk::EventControllerKey::new();
		let save = widgets.save.clone();
		keys.connect_key_pressed(move |_, key, _, _| {
			if !matches!(key, gtk::gdk::Key::Return | gtk::gdk::Key::KP_Enter) {
				return glib::Propagation::Proceed;
			}
			if save.is_sensitive() {
				save.emit_clicked();
			}
			glib::Propagation::Stop
		});
		widgets.line.add_controller(keys);
		// Outlives the page, so it must not assume the page is there to hear.
		let recolour = sender.input_sender().clone();
		adw::StyleManager::default().connect_dark_notify(move |_| recolour.emit(EditorInput::Recolour));
		ComponentParts { model, widgets }
	}

	fn update_with_view(&mut self, widgets: &mut Self::Widgets, input: EditorInput, sender: ComponentSender<Self>, _: &Self::Root) {
		match input {
			EditorInput::View(view) => self.draw(widgets, *view),
			EditorInput::FolderChosen(at) => {
				if let Some(name) = self.folders.get(at as usize) {
					self.ctx.engine.send(Action::RuleSetFolder { name: name.clone() });
				}
			},
			EditorInput::LabelChosen(at) => {
				let name = self.labels.get(at as usize).cloned();
				self.naming = name.is_none();
				self.ctx.engine.send(Action::RuleSetLabel { name: name.unwrap_or_else(|| widgets.new_label.text().to_string()) });
			},
			EditorInput::NewLabel(name) => {
				if self.naming {
					self.ctx.engine.send(Action::RuleSetLabel { name });
				}
			},
			EditorInput::Put(piece) => {
				let buffer = widgets.line.buffer();
				let cursor = buffer.iter_at_mark(&buffer.get_insert()).offset() as u32;
				let (from, to) = buffer.selection_bounds().map_or((cursor, cursor), |(a, b)| (a.offset() as u32, b.offset() as u32));
				self.ctx.engine.send(Action::RulePut { piece, cursor: from, selection_end: to });
				widgets.line.grab_focus();
			},
			EditorInput::Recolour => self.colour(&widgets.line),
		}
		self.update_view(widgets, sender);
	}
}

fn line_text(line: &gtk::TextView) -> String {
	let buffer = line.buffer();
	buffer.text(&buffer.start_iter(), &buffer.end_iter(), false).to_string()
}

impl Editor {
	fn does_now(&self, does: RuleDoes) -> bool {
		self.shown.as_ref().is_some_and(|v| v.does == does)
	}

	/// Draws what the core says about this rule.
	fn draw(&mut self, widgets: &EditorWidgets, view: RuleEditorView) {
		self.drawing.set(true);
		let before = self.shown.replace(view.clone());
		self.editing = view.editing;
		// The line is the person's while they type; the core only sets it when it changes it itself.
		if before.as_ref().map(|b| b.revision) != Some(view.revision) {
			let buffer = widgets.line.buffer();
			buffer.set_text(&view.line);
			buffer.place_cursor(&buffer.iter_at_offset(view.cursor as i32));
		}
		self.colour(&widgets.line);
		self.draw_preview(&widgets.preview, &view);
		for (button, (_, does)) in self.does.iter().zip(DOES) {
			if does == view.does {
				button.set_active(true);
			}
		}
		self.draw_choices(widgets, &view);
		self.drawing.set(false);
	}

	fn draw_choices(&mut self, widgets: &EditorWidgets, view: &RuleEditorView) {
		// A choice the rule names that has gone stays offered, so saving does not change it.
		let with = |names: &[String], named: &str| {
			let mut all = names.to_vec();
			if !named.is_empty() && !all.iter().any(|n| n.eq_ignore_ascii_case(named)) {
				all.push(named.to_string());
			}
			all
		};
		let folders = with(&view.folders, &view.folder);
		if self.folders != folders {
			let names: Vec<&str> = folders.iter().map(String::as_str).collect();
			widgets.folder.set_model(Some(&gtk::StringList::new(&names)));
			self.folders = folders;
		}
		if let Some(at) = self.folders.iter().position(|n| n.eq_ignore_ascii_case(&view.folder)) {
			widgets.folder.set_selected(at as u32);
		}
		let labels = if self.naming { view.labels.clone() } else { with(&view.labels, &view.label) };
		if self.labels != labels {
			let names: Vec<&str> = labels.iter().map(String::as_str).chain([NEW_LABEL]).collect();
			widgets.label.set_model(Some(&gtk::StringList::new(&names)));
			self.labels = labels;
		}
		let chosen = if self.naming { None } else { self.labels.iter().position(|n| n.eq_ignore_ascii_case(&view.label)) };
		widgets.label.set_selected(chosen.unwrap_or(self.labels.len()) as u32);
		if chosen.is_none() {
			self.naming = true;
		}
	}

	/// Colours the line's fields, operators and brackets, as the core has read them.
	fn colour(&self, line: &gtk::TextView) {
		let buffer = line.buffer();
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
		// What it read is of the line as it was: colours wait for the core to catch up with typing.
		let Some(view) = self.shown.as_ref().filter(|v| v.line == line_text(line)) else { return };
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
	fn draw_preview(&self, preview: &gtk::ListBox, view: &RuleEditorView) {
		preview.remove_all();
		let (icon, class) = match view.tone {
			PreviewTone::Neutral => ("dialog-information-symbolic", "dim-label"),
			PreviewTone::Good => ("object-select-symbolic", "success"),
			PreviewTone::Bad => ("dialog-warning-symbolic", "error"),
		};
		relm4::view! {
			head = gtk::ListBoxRow {
				set_activatable: false,
				#[wrap(Some)]
				set_child = &gtk::Box {
					set_spacing: 8,
					set_margin_top: 10,
					set_margin_bottom: 10,
					set_margin_start: 12,
					set_margin_end: 12,
					add_css_class: class,
					gtk::Image {
						set_icon_name: Some(icon),
					},
					gtk::Label {
						set_label: &view.message,
						set_wrap: true,
						set_xalign: 0.0,
						set_hexpand: true,
						add_css_class: "caption",
					},
				},
			}
		}
		preview.append(&head);
		for mail in &view.preview {
			relm4::view! {
				row = gtk::ListBoxRow {
					set_activatable: false,
					#[wrap(Some)]
					set_child = &gtk::Box {
						set_spacing: 12,
						set_margin_top: 8,
						set_margin_bottom: 8,
						set_margin_start: 12,
						set_margin_end: 12,
						gtk::Label {
							set_label: &mail.who,
							set_xalign: 0.0,
							set_width_chars: 16,
							set_max_width_chars: 16,
							set_ellipsize: gtk::pango::EllipsizeMode::End,
							add_css_class: "caption",
						},
						gtk::Label {
							set_label: &mail.subject,
							set_xalign: 0.0,
							set_hexpand: true,
							set_ellipsize: gtk::pango::EllipsizeMode::End,
							set_css_classes: &["caption", "dim-label"],
						},
					},
				}
			}
			preview.append(&row);
		}
		if let Some(more) = &view.more {
			let (engine, dialog) = (self.ctx.engine.clone(), self.dialog.clone());
			relm4::view! {
				row = gtk::ListBoxRow {
					set_activatable: false,
					#[wrap(Some)]
					set_child = &gtk::Box {
						set_spacing: 12,
						set_margin_top: 4,
						set_margin_bottom: 4,
						set_margin_start: 12,
						set_margin_end: 6,
						gtk::Label {
							set_label: more,
							set_xalign: 0.0,
							set_hexpand: true,
							set_css_classes: &["caption", "dim-label"],
						},
						gtk::Button {
							set_label: "Show All in Search",
							set_css_classes: &["flat", "caption", "accent"],
							connect_clicked => move |_| {
								engine.send(Action::RuleInSearch);
								dialog.close();
							},
						},
					},
				}
			}
			preview.append(&row);
		}
	}
}

/// A rule's page: the component, and what the scenes do with it.
pub struct Page {
	controller: Controller<Editor>,
	ctx: Ctx,
	dialog: adw::PreferencesDialog,
	pub folder: adw::ComboRow,
	pub label: adw::ComboRow,
	pub new_label: adw::EntryRow,
	pub save: gtk::Button,
}

impl Page {
	fn new(controller: Controller<Editor>, ctx: Ctx, dialog: adw::PreferencesDialog) -> Page {
		let (folder, label, new_label, save) = {
			let w = controller.widgets();
			(w.folder.clone(), w.label.clone(), w.new_label.clone(), w.save.clone())
		};
		Page { controller, ctx, dialog, folder, label, new_label, save }
	}

	/// The core has drawn this rule at least once, so the line and its choices are what it says.
	pub fn is_ready(&self) -> bool {
		self.controller.model().shown.is_some()
	}

	/// The rule's line as typed.
	pub fn line(&self) -> String {
		line_text(&self.controller.widgets().line)
	}

	/// For the screenshot scenes: how tall the line shows, in pixels.
	pub fn line_height(&self) -> i32 {
		self.controller.widgets().line.height()
	}

	pub fn set_line(&self, text: &str) {
		self.controller.widgets().line.buffer().set_text(text);
	}

	/// What the rule does: 0 Spam, 1 a folder, 2 a label.
	pub fn does(&self) -> usize {
		self.controller.model().does.iter().position(|b| b.is_active()).unwrap_or(0)
	}

	pub fn set_does(&self, i: usize) {
		if let Some(b) = self.controller.model().does.get(i) {
			b.set_active(true);
		}
	}

	pub fn put(&self, piece: RulePiece) {
		self.controller.emit(EditorInput::Put(piece));
	}

	/// Lists everything the line catches in the main window's search, leaving Preferences.
	pub fn show_all(&self) {
		self.ctx.engine.send(Action::RuleInSearch);
		self.dialog.close();
	}

	/// Deletes this rule.
	pub fn delete(&self) {
		self.controller.widgets().delete.emit_clicked();
	}

	/// For the screenshot scenes: the preview's message, the mails it lists, and the line under them.
	pub fn preview(&self) -> (String, usize, Option<String>) {
		let widgets = self.controller.widgets();
		let texts: Vec<Vec<String>> = std::iter::successors(widgets.preview.first_child(), |w| w.next_sibling())
			.filter_map(|row| row.downcast::<gtk::ListBoxRow>().ok()?.child())
			.map(|b| std::iter::successors(b.first_child(), |w| w.next_sibling()).filter_map(|w| w.downcast::<gtk::Label>().ok()).map(|l| l.label().to_string()).collect())
			.collect();
		let message = texts.first().and_then(|t| t.first().cloned()).unwrap_or_default();
		let more = texts.last().filter(|t| texts.len() > 1 && t.first().is_some_and(|l| l.starts_with("and "))).and_then(|t| t.first().cloned());
		let mails = texts.len().saturating_sub(1) - usize::from(more.is_some());
		(message, mails, more)
	}
}

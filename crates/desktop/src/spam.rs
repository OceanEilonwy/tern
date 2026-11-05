//! Move to Spam: the conversation on show, and other mail like it picked from a list, as the core
//! offers them.
//!
//! The list is what a rule's line catches, starting with the sender's address; Sender, Domain and
//! Subject fill the line in, and it can be edited. The conversation's own mails head the list, ticked.
//! The line can also be kept as a rule, so new mail like it goes to Spam as it arrives.

use std::sync::Arc;

use adw::prelude::*;
use gtk::glib;
use relm4::factory::FactoryVecDeque;
use relm4::prelude::*;
use tern_app::view::{Action, ChoiceView, SpamRow};
use tern_app::Engine;

/// What the core says about the sheet.
#[derive(Clone, Debug)]
pub struct Offer {
	pub revision: u64,
	pub line: String,
	pub problem: Option<String>,
	pub rows: Vec<SpamRow>,
	pub count: String,
	pub keep_rule: bool,
	pub confirm: String,
	pub choices: Vec<ChoiceView>,
}

/// One mail's tick.
pub struct Row {
	view: SpamRow,
	engine: Arc<Engine>,
}

#[relm4::factory(pub)]
impl FactoryComponent for Row {
	type Init = (SpamRow, Arc<Engine>);
	type Input = SpamRow;
	type Output = ();
	type CommandOutput = ();
	type ParentWidget = gtk::ListBox;

	view! {
		adw::ActionRow {
			set_title: &glib::markup_escape_text(&self.view.who),
			set_subtitle: &glib::markup_escape_text(&self.view.subject),
			set_subtitle_lines: 1,
			set_activatable_widget: Some(&check),
			#[name = "check"]
			add_prefix = &gtk::CheckButton {
				set_valign: gtk::Align::Center,
				#[watch]
				#[block_signal(toggled)]
				set_active: self.view.picked,
				connect_toggled[engine = self.engine.clone(), id = self.view.id.clone()] => move |_| engine.send(Action::SpamToggle { id: id.clone() }) @toggled,
			},
			add_suffix = &gtk::Label {
				set_label: &self.view.date,
				set_css_classes: &["dim-label", "caption", "numeric"],
			},
		}
	}

	fn init_model((view, engine): Self::Init, _: &DynamicIndex, _: FactorySender<Self>) -> Self {
		Row { view, engine }
	}

	fn update(&mut self, view: SpamRow, _: FactorySender<Self>) {
		self.view = view;
	}
}

pub struct Picker {
	engine: Arc<Engine>,
	offer: Offer,
	/// The core has set the line itself, rather than following what is typed.
	line_set: bool,
	rows: FactoryVecDeque<Row>,
}

#[relm4::component(pub)]
impl SimpleComponent for Picker {
	type Init = (Arc<Engine>, Offer);
	type Input = Offer;
	type Output = ();

	view! {
		adw::Dialog {
			set_title: "Move to Spam",
			set_content_width: 560,
			set_content_height: 640,
			set_default_widget: Some(&confirm),
			#[wrap(Some)]
			set_child = &adw::ToolbarView {
				add_top_bar = &adw::HeaderBar {
					set_show_end_title_buttons: false,
					set_show_start_title_buttons: false,
					pack_start = &gtk::Button {
						set_label: "Cancel",
						connect_clicked[root] => move |_| {
							root.close();
						},
					},
					#[name = "confirm"]
					pack_end = &gtk::Button {
						add_css_class: "destructive-action",
						#[watch]
						set_label: &model.offer.confirm,
						#[watch]
						set_sensitive: model.offer.keep_rule || model.offer.rows.iter().any(|r| r.picked),
						connect_clicked[engine = model.engine.clone()] => move |_| engine.send(Action::SpamConfirm),
					},
				},
				#[wrap(Some)]
				set_content = &gtk::Box {
					set_orientation: gtk::Orientation::Vertical,
					set_spacing: 12,
					set_margin_top: 6,
					set_margin_bottom: 18,
					set_margin_start: 18,
					set_margin_end: 18,

					gtk::Label {
						set_label: "Find more mail like this",
						set_xalign: 0.0,
						add_css_class: "heading",
					},
					#[local_ref]
					choices -> gtk::Box {
						add_css_class: "linked",
						set_halign: gtk::Align::Start,
					},
					#[name = "entry"]
					gtk::Entry {
						set_hexpand: true,
						update_property: &[gtk::accessible::Property::Label("Mail like this")],
						#[track = "model.line_set"]
						#[block_signal(typed)]
						set_text: &model.offer.line,
						#[watch]
						set_css_classes: if model.offer.problem.is_some() { &["monospace", "error"] } else { &["monospace"] },
						connect_changed[engine = model.engine.clone()] => move |entry| engine.send(Action::SpamLine { line: entry.text().to_string() }) @typed,
					},
					gtk::Label {
						set_xalign: 0.0,
						set_wrap: true,
						set_css_classes: &["caption", "error"],
						#[watch]
						set_visible: model.offer.problem.is_some(),
						#[watch]
						set_label: model.offer.problem.as_deref().unwrap_or_default(),
					},
					gtk::Box {
						set_spacing: 12,
						#[name = "all"]
						gtk::CheckButton {
							set_label: Some("Select All"),
							#[watch]
							#[block_signal(all_toggled)]
							set_active: !model.offer.rows.is_empty() && model.offer.rows.iter().all(|r| r.picked),
							#[watch]
							set_inconsistent: model.offer.rows.iter().any(|r| r.picked) && !model.offer.rows.iter().all(|r| r.picked),
							connect_toggled[engine = model.engine.clone()] => move |all| engine.send(Action::SpamSelectAll { on: all.is_active() }) @all_toggled,
						},
						gtk::Label {
							set_xalign: 1.0,
							set_hexpand: true,
							set_css_classes: &["dim-label", "caption"],
							#[watch]
							set_label: &model.offer.count,
						},
					},
					gtk::ScrolledWindow {
						set_hscrollbar_policy: gtk::PolicyType::Never,
						set_vexpand: true,
						set_min_content_height: 200,
						#[local_ref]
						list -> gtk::ListBox {
							set_selection_mode: gtk::SelectionMode::None,
							add_css_class: "boxed-list",
							set_valign: gtk::Align::Start,
							update_property: &[gtk::accessible::Property::Label("Mail like this")],
						},
					},
					gtk::ListBox {
						set_selection_mode: gtk::SelectionMode::None,
						add_css_class: "boxed-list",
						#[name = "keep"]
						append = &adw::SwitchRow {
							set_title: "Always Move Mail Like This to Spam",
							set_subtitle: "New mail this line catches goes straight to Spam; rules are in Preferences",
							#[watch]
							#[block_signal(keep_toggled)]
							set_active: model.offer.keep_rule,
							connect_active_notify[engine = model.engine.clone()] => move |keep| engine.send(Action::SpamKeepRule { on: keep.is_active() }) @keep_toggled,
						},
					},
				},
			},
			connect_closed[engine = model.engine.clone()] => move |_| engine.send(Action::CloseSheet),
		}
	}

	fn init(init: Self::Init, root: Self::Root, _sender: ComponentSender<Self>) -> ComponentParts<Self> {
		let (engine, offer) = init;
		let choices = gtk::Box::default();
		for choice in &offer.choices {
			let button = gtk::Button::with_label(&choice.label);
			let (engine, value) = (engine.clone(), choice.value.clone());
			button.connect_clicked(move |_| engine.send(Action::SpamUse { value: value.clone() }));
			choices.append(&button);
		}
		let rows = FactoryVecDeque::builder().launch(gtk::ListBox::default()).detach();
		let mut model = Picker { engine, offer: Offer { rows: Vec::new(), ..offer.clone() }, line_set: true, rows };
		model.show_rows(&offer.rows);
		model.offer = offer;
		let list = model.rows.widget();
		let widgets = view_output!();
		widgets.entry.grab_focus();
		ComponentParts { model, widgets }
	}

	fn update(&mut self, offer: Offer, _: ComponentSender<Self>) {
		self.line_set = offer.revision != self.offer.revision;
		self.show_rows(&offer.rows);
		self.offer = offer;
	}
}

impl Picker {
	fn show_rows(&mut self, rows: &[SpamRow]) {
		let same = self.offer.rows.len() == rows.len() && self.offer.rows.iter().zip(rows).all(|(a, b)| a.id == b.id);
		if same {
			for (i, row) in rows.iter().enumerate() {
				self.rows.send(i, row.clone());
			}
		} else {
			let mut shown = self.rows.guard();
			shown.clear();
			for row in rows {
				shown.push_back((row.clone(), self.engine.clone()));
			}
		}
	}

	/// For the screenshot scenes: the subjects listed, and whether each is ticked.
	pub fn listed(&self) -> Vec<(String, bool)> {
		self.offer.rows.iter().map(|r| (r.subject.clone(), r.picked)).collect()
	}
}

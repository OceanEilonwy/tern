//! The labels on a conversation, as the core offers them: a tick for each, and a line for a new one.

use std::sync::Arc;

use adw::prelude::*;
use gtk::glib;
use relm4::factory::FactoryVecDeque;
use relm4::prelude::*;
use tern_app::view::{Action, LabelChoice};
use tern_app::Engine;

/// One label's tick.
pub struct Row {
	view: LabelChoice,
	engine: Arc<Engine>,
}

#[relm4::factory(pub)]
impl FactoryComponent for Row {
	type Init = (LabelChoice, Arc<Engine>);
	type Input = LabelChoice;
	type Output = ();
	type CommandOutput = ();
	type ParentWidget = gtk::ListBox;

	view! {
		adw::ActionRow {
			set_title: &glib::markup_escape_text(&self.view.name),
			set_activatable_widget: Some(&check),
			#[name = "check"]
			add_prefix = &gtk::CheckButton {
				set_valign: gtk::Align::Center,
				#[watch]
				#[block_signal(toggled)]
				set_active: self.view.on,
				#[watch]
				set_inconsistent: self.view.mixed,
				connect_toggled[engine = self.engine.clone(), id = self.view.id.clone()] => move |_| engine.send(Action::ToggleLabel { id: id.clone() }) @toggled,
			},
		}
	}

	fn init_model((view, engine): Self::Init, _: &DynamicIndex, _: FactorySender<Self>) -> Self {
		Row { view, engine }
	}

	fn update(&mut self, view: LabelChoice, _: FactorySender<Self>) {
		self.view = view;
	}
}

pub struct Labels {
	engine: Arc<Engine>,
	subject: String,
	rows: FactoryVecDeque<Row>,
	shown: Vec<LabelChoice>,
}

#[derive(Debug)]
pub struct Offer {
	pub subject: String,
	pub choices: Vec<LabelChoice>,
}

/// Asks the core for a label named what is in `entry`, if anything is.
fn create(engine: &Engine, entry: &gtk::Entry) {
	let name = entry.text().trim().to_string();
	if !name.is_empty() {
		engine.send(Action::CreateLabel { name });
		entry.set_text("");
	}
}

#[relm4::component(pub)]
impl SimpleComponent for Labels {
	type Init = Arc<Engine>;
	type Input = Offer;
	type Output = ();

	view! {
		adw::Dialog {
			set_title: "Labels",
			set_content_width: 420,
			set_content_height: 460,
			#[wrap(Some)]
			set_child = &adw::ToolbarView {
				add_top_bar = &adw::HeaderBar {},
				#[wrap(Some)]
				set_content = &gtk::Box {
					set_orientation: gtk::Orientation::Vertical,
					set_spacing: 12,
					set_margin_top: 6,
					set_margin_bottom: 18,
					set_margin_start: 18,
					set_margin_end: 18,

					gtk::Label {
						set_xalign: 0.0,
						set_ellipsize: gtk::pango::EllipsizeMode::End,
						add_css_class: "dim-label",
						#[watch]
						set_label: &model.subject,
					},
					gtk::ScrolledWindow {
						set_hscrollbar_policy: gtk::PolicyType::Never,
						set_vexpand: true,
						set_min_content_height: 120,
						#[local_ref]
						list -> gtk::ListBox {
							set_selection_mode: gtk::SelectionMode::None,
							add_css_class: "boxed-list",
							set_valign: gtk::Align::Start,
							update_property: &[gtk::accessible::Property::Label("Labels")],
						},
					},
					gtk::Box {
						set_spacing: 6,
						#[name = "new_name"]
						gtk::Entry {
							set_placeholder_text: Some("New label"),
							set_hexpand: true,
							update_property: &[gtk::accessible::Property::Label("New label")],
							connect_changed[add] => move |entry| add.set_sensitive(!entry.text().trim().is_empty()),
							connect_activate[engine = model.engine.clone()] => move |entry| create(&engine, entry),
						},
						#[name = "add"]
						gtk::Button {
							set_label: "_Add",
							set_use_underline: true,
							set_sensitive: false,
							connect_clicked[engine = model.engine.clone(), new_name] => move |_| create(&engine, &new_name),
						},
					},
				},
			},
			connect_closed[engine = model.engine.clone()] => move |_| engine.send(Action::CloseSheet),
		}
	}

	fn init(engine: Self::Init, root: Self::Root, _sender: ComponentSender<Self>) -> ComponentParts<Self> {
		let rows = FactoryVecDeque::builder().launch(gtk::ListBox::default()).detach();
		let model = Labels { engine, subject: String::new(), rows, shown: Vec::new() };
		let list = model.rows.widget();
		let widgets = view_output!();
		ComponentParts { model, widgets }
	}

	fn update(&mut self, offer: Offer, _: ComponentSender<Self>) {
		self.subject = offer.subject;
		let same = self.shown.len() == offer.choices.len() && self.shown.iter().zip(&offer.choices).all(|(a, b)| a.id == b.id && a.name == b.name);
		if same {
			for (i, choice) in offer.choices.iter().enumerate() {
				self.rows.send(i, choice.clone());
			}
		} else {
			let mut rows = self.rows.guard();
			rows.clear();
			for choice in &offer.choices {
				rows.push_back((choice.clone(), self.engine.clone()));
			}
		}
		self.shown = offer.choices;
	}
}

impl Labels {
	/// For the screenshot scenes: the label ticks, by name.
	pub fn ticks(&self) -> Vec<(String, bool)> {
		self.shown.iter().map(|c| (c.name.clone(), c.on)).collect()
	}

	/// For the screenshot scenes: ticks or unticks a label, as a click does.
	pub fn toggle(&self, name: &str) {
		let rows = std::iter::successors(self.rows.widget().first_child(), |w| w.next_sibling());
		if let Some(row) = rows.filter_map(|w| w.downcast::<adw::ActionRow>().ok()).find(|r| r.title() == name) {
			ActionRowExt::activate(&row);
		}
	}
}

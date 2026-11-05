//! The Clean Up dialog, drawn from what the core offers. What is measured and cleared is the core's.

use std::rc::Rc;
use std::sync::Arc;

use adw::prelude::*;
use relm4::prelude::*;
use tern_app::format;
use tern_app::view::{Action, CleanupChoice, CleanupPart};
use tern_app::Engine;

pub struct Cleanup {
	engine: Arc<Engine>,
	/// The ticks: each part with its size, and whether it is picked.
	parts: Vec<(CleanupPart, u64, bool)>,
	/// All Mail is ticked, which takes the place of the rest.
	pub all: bool,
}

#[derive(Debug)]
pub enum Input {
	Tick(CleanupPart, bool),
	/// The dialog was answered: clean, or leave things be.
	Answer(bool),
}

impl Cleanup {
	fn ticked(&self, part: CleanupPart) -> bool {
		self.parts.iter().any(|(p, _, on)| *p == part && *on)
	}

	fn button(&self) -> String {
		if self.all {
			return "_Delete All Mail".to_string();
		}
		let bytes: u64 = self.parts.iter().filter(|(_, _, on)| *on).map(|(_, b, _)| b).sum();
		format!("_Free {}", format::size(bytes))
	}
}

/// A choice in the dialog: a tick, a name with what it means, and its size.
fn choice(view: &CleanupChoice, sender: &ComponentSender<Cleanup>, on: bool) -> adw::ActionRow {
	let part = view.part;
	let sender = sender.clone();
	relm4::view! {
		row = adw::ActionRow {
			set_title: &view.title,
			set_subtitle: &view.detail,
			set_activatable_widget: Some(&check),
			#[name = "check"]
			add_prefix = &gtk::CheckButton {
				set_valign: gtk::Align::Center,
				set_active: on,
				connect_toggled => move |check| sender.input(Input::Tick(part, check.is_active())),
			},
			add_suffix = &gtk::Label {
				set_label: &view.size,
				set_css_classes: &["dim-label", "numeric"],
			},
		}
	}
	row
}

#[relm4::component(pub)]
impl SimpleComponent for Cleanup {
	type Init = (Arc<Engine>, String, Vec<CleanupChoice>);
	type Input = Input;
	type Output = ();

	view! {
		adw::AlertDialog {
			set_heading: Some("Clean Up"),
			set_body: &format!("Tern takes {total} on this computer."),
			set_prefer_wide_layout: true,
			add_response: ("cancel", "_Cancel"),
			add_response: ("clean", ""),
			set_close_response: "cancel",
			set_default_response: Some("clean"),
			#[watch]
			set_response_label: ("clean", &model.button()),
			#[watch]
			set_response_appearance: ("clean", if model.all { adw::ResponseAppearance::Destructive } else { adw::ResponseAppearance::Suggested }),
			#[watch]
			set_response_enabled: ("clean", model.all || model.parts.iter().any(|(_, _, on)| *on)),
			#[wrap(Some)]
			set_extra_child = &gtk::Box {
				set_orientation: gtk::Orientation::Vertical,
				set_spacing: 12,
				#[local_ref]
				list -> gtk::ListBox {
					add_css_class: "boxed-list",
					set_selection_mode: gtk::SelectionMode::None,
					#[watch]
					set_sensitive: !model.all,
				},
				#[local_ref]
				everything -> gtk::ListBox {
					add_css_class: "boxed-list",
					set_selection_mode: gtk::SelectionMode::None,
				},
				gtk::Label {
					set_label: "Pinned conversations, drafts, rules and settings always stay.",
					set_css_classes: &["dim-label", "caption"],
					set_wrap: true,
					set_justify: gtk::Justification::Center,
				},
			},
		}
	}

	fn init((engine, total, choices): Self::Init, root: Self::Root, sender: ComponentSender<Self>) -> ComponentParts<Self> {
		let (list, everything) = (gtk::ListBox::default(), gtk::ListBox::default());
		// Only the older mail is ticked to start, as it costs nothing to clear.
		let picked = |c: &CleanupChoice| c.part == CleanupPart::Older && c.bytes > 0;
		for view in &choices {
			let row = choice(view, &sender, picked(view));
			if view.part == CleanupPart::All {
				row.add_css_class("cleanup-all");
				everything.append(&row);
			} else {
				list.append(&row);
			}
		}
		let parts = choices.iter().filter(|c| c.part != CleanupPart::All).map(|c| (c.part, c.bytes, picked(c))).collect();
		let model = Cleanup { engine, parts, all: false };
		let widgets = view_output!();
		let answers = sender.clone();
		root.connect_response(None, move |_, response| answers.input(Input::Answer(response == "clean")));
		ComponentParts { model, widgets }
	}

	fn update(&mut self, input: Input, _: ComponentSender<Self>) {
		match input {
			Input::Tick(CleanupPart::All, on) => self.all = on,
			Input::Tick(part, on) => {
				if let Some(tick) = self.parts.iter_mut().find(|(p, _, _)| *p == part) {
					tick.2 = on;
				}
			},
			Input::Answer(false) => self.engine.send(Action::CloseSheet),
			Input::Answer(true) => self.engine.send(Action::RunCleanup {
				older: self.ticked(CleanupPart::Older),
				vectors: self.ticked(CleanupPart::Vectors),
				model: self.ticked(CleanupPart::Model),
				all: self.all,
			}),
		}
	}
}

/// Clean Up over `parent`, answering the core.
pub fn open(engine: &Arc<Engine>, total: String, choices: Vec<CleanupChoice>, parent: &impl IsA<gtk::Widget>) -> Rc<Controller<Cleanup>> {
	let controller = Cleanup::builder().launch((engine.clone(), total, choices)).detach();
	controller.widget().present(Some(parent));
	Rc::new(controller)
}

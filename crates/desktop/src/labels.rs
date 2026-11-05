//! The labels on a conversation, as the core offers them: a tick for each, and a line for a new one.

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use adw::prelude::*;
use tern_app::view::{Action, LabelChoice};
use tern_app::Engine;

use crate::window::Ctx;

pub struct Picker {
	pub dialog: adw::Dialog,
	subject: gtk::Label,
	list: gtk::ListBox,
	pub new_name: gtk::Entry,
	shown: RefCell<Vec<(LabelChoice, gtk::CheckButton)>>,
	/// Drawing what the core said, which is not the person doing anything.
	filling: Rc<Cell<bool>>,
	ctx: Ctx,
}

/// Asks the core for a label named what is in `entry`, if anything is.
fn create(engine: &Engine, entry: &gtk::Entry) {
	let name = entry.text().trim().to_string();
	if !name.is_empty() {
		engine.send(Action::CreateLabel { name });
		entry.set_text("");
	}
}

/// Opens the dialog; [`Picker::show`] then fills it in.
pub fn open(ctx: &Ctx) -> Rc<Picker> {
	relm4_macros::view! {
		dialog = adw::Dialog {
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

					#[name = "subject"]
					append = &gtk::Label {
						set_xalign: 0.0,
						set_ellipsize: gtk::pango::EllipsizeMode::End,
						add_css_class: "dim-label",
					},
					append = &gtk::ScrolledWindow {
						set_hscrollbar_policy: gtk::PolicyType::Never,
						set_vexpand: true,
						set_min_content_height: 120,
						#[wrap(Some)]
						#[name = "list"]
						set_child = &gtk::ListBox {
							set_selection_mode: gtk::SelectionMode::None,
							add_css_class: "boxed-list",
							set_valign: gtk::Align::Start,
							update_property: &[gtk::accessible::Property::Label("Labels")],
						},
					},
					append = &gtk::Box {
						set_spacing: 6,
						#[name = "new_name"]
						append = &gtk::Entry {
							set_placeholder_text: Some("New label"),
							set_hexpand: true,
							update_property: &[gtk::accessible::Property::Label("New label")],
							connect_changed[add] => move |entry| add.set_sensitive(!entry.text().trim().is_empty()),
							connect_activate[engine = ctx.engine.clone()] => move |entry| create(&engine, entry),
						},
						#[name = "add"]
						append = &gtk::Button {
							set_label: "_Add",
							set_use_underline: true,
							set_sensitive: false,
							connect_clicked[engine = ctx.engine.clone(), new_name] => move |_| create(&engine, &new_name),
						},
					},
				},
			},
			connect_closed[engine = ctx.engine.clone()] => move |_| engine.send(Action::CloseSheet),
		}
	}
	dialog.present(Some(&ctx.window));
	Rc::new(Picker { dialog, subject, list, new_name, shown: RefCell::default(), filling: Rc::default(), ctx: ctx.clone() })
}

impl Picker {
	/// Draws what the core offers.
	pub fn show(&self, subject: &str, choices: &[LabelChoice]) {
		self.filling.set(true);
		self.subject.set_label(subject);
		let same = {
			let shown = self.shown.borrow();
			shown.len() == choices.len() && shown.iter().zip(choices).all(|((a, _), b)| a.id == b.id && a.name == b.name)
		};
		if !same {
			self.list.remove_all();
			let mut shown = Vec::new();
			for choice in choices {
				let check = gtk::CheckButton::builder().valign(gtk::Align::Center).build();
				let row = adw::ActionRow::builder().title(gtk::glib::markup_escape_text(&choice.name)).activatable_widget(&check).build();
				row.add_prefix(&check);
				self.list.append(&row);
				let (engine, id, filling) = (self.ctx.engine.clone(), choice.id.clone(), self.filling.clone());
				check.connect_toggled(move |_| {
					if !filling.get() {
						engine.send(Action::ToggleLabel { id: id.clone() });
					}
				});
				shown.push((choice.clone(), check));
			}
			*self.shown.borrow_mut() = shown;
		}
		for ((old, check), new) in self.shown.borrow_mut().iter_mut().zip(choices) {
			check.set_active(new.on);
			check.set_inconsistent(new.mixed);
			*old = new.clone();
		}
		self.filling.set(false);
	}

	/// For the screenshot scenes: the label ticks, by name.
	pub fn ticks(&self) -> Vec<(String, bool)> {
		self.shown.borrow().iter().map(|(c, b)| (c.name.clone(), b.is_active())).collect()
	}

	/// For the screenshot scenes: ticks or unticks a label, as a click does.
	pub fn toggle(&self, name: &str) {
		if let Some((_, check)) = self.shown.borrow().iter().find(|(c, _)| c.name == name) {
			check.set_active(!check.is_active());
		}
	}
}

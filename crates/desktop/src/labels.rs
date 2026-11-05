//! The labels on a conversation, as the core offers them: a tick for each, and a line for a new one.

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use adw::prelude::*;
use tern_app::view::{Action, LabelChoice};

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

/// Opens the dialog; [`Picker::show`] then fills it in.
pub fn open(ctx: &Ctx) -> Rc<Picker> {
	let subject = gtk::Label::builder().xalign(0.0).ellipsize(gtk::pango::EllipsizeMode::End).css_classes(["dim-label"]).build();
	let list = gtk::ListBox::builder().selection_mode(gtk::SelectionMode::None).css_classes(["boxed-list"]).valign(gtk::Align::Start).build();
	list.update_property(&[gtk::accessible::Property::Label("Labels")]);
	let scroll = gtk::ScrolledWindow::builder().hscrollbar_policy(gtk::PolicyType::Never).child(&list).vexpand(true).min_content_height(120).build();
	let new_name = gtk::Entry::builder().placeholder_text("New label").hexpand(true).build();
	new_name.update_property(&[gtk::accessible::Property::Label("New label")]);
	let add = gtk::Button::builder().label("_Add").use_underline(true).sensitive(false).build();
	let adding = gtk::Box::builder().spacing(6).build();
	adding.append(&new_name);
	adding.append(&add);
	let column = gtk::Box::builder().orientation(gtk::Orientation::Vertical).spacing(12).margin_top(6).margin_bottom(18).margin_start(18).margin_end(18).build();
	column.append(&subject);
	column.append(&scroll);
	column.append(&adding);
	let view = adw::ToolbarView::new();
	view.add_top_bar(&adw::HeaderBar::new());
	view.set_content(Some(&column));
	let dialog = adw::Dialog::builder().title("Labels").content_width(420).content_height(460).child(&view).build();

	let b = add.clone();
	new_name.connect_changed(move |e| b.set_sensitive(!e.text().trim().is_empty()));
	let (engine, entry) = (ctx.engine.clone(), new_name.clone());
	let create = move || {
		let name = entry.text().trim().to_string();
		if !name.is_empty() {
			engine.send(Action::CreateLabel { name });
			entry.set_text("");
		}
	};
	let c = create.clone();
	add.connect_clicked(move |_| c());
	new_name.connect_activate(move |_| create());
	let engine = ctx.engine.clone();
	dialog.connect_closed(move |_| engine.send(Action::CloseSheet));
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

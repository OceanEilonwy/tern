//! The Clean Up dialog, drawn from what the core offers. What is measured and cleared is the core's.

use adw::prelude::*;
use tern_app::format;
use tern_app::view::{Action, CleanupChoice, CleanupPart};

use crate::window::Ctx;

/// A choice in the dialog: a tick, a name with what it means, and its size.
fn choice(view: &CleanupChoice) -> (adw::ActionRow, gtk::CheckButton) {
	let check = gtk::CheckButton::builder().valign(gtk::Align::Center).build();
	let row = adw::ActionRow::builder().title(&view.title).subtitle(&view.detail).activatable_widget(&check).build();
	row.add_prefix(&check);
	row.add_suffix(&gtk::Label::builder().label(&view.size).css_classes(["dim-label", "numeric"]).build());
	(row, check)
}

/// The dialog for what the core offers, with its "All Mail" tick. Only the older mail is ticked to
/// start, as it costs nothing to clear.
pub fn dialog(ctx: &Ctx, total: &str, choices: &[CleanupChoice]) -> (adw::AlertDialog, gtk::CheckButton) {
	let dialog = adw::AlertDialog::builder().heading("Clean Up").body(format!("Tern takes {total} on this computer.")).prefer_wide_layout(true).build();
	let list = gtk::ListBox::builder().css_classes(["boxed-list"]).selection_mode(gtk::SelectionMode::None).build();
	let everything = gtk::ListBox::builder().css_classes(["boxed-list"]).selection_mode(gtk::SelectionMode::None).build();
	let mut parts = Vec::new();
	let mut all = gtk::CheckButton::new();
	for view in choices {
		let (row, check) = choice(view);
		if view.part == CleanupPart::All {
			row.add_css_class("cleanup-all");
			everything.append(&row);
			all = check;
		} else {
			check.set_active(view.part == CleanupPart::Older && view.bytes > 0);
			list.append(&row);
			parts.push((view.part, check, view.bytes, row));
		}
	}
	let note = gtk::Label::builder().label("Pinned conversations, drafts, rules and settings always stay.").css_classes(["dim-label", "caption"]).wrap(true).justify(gtk::Justification::Center).build();
	let content = gtk::Box::builder().orientation(gtk::Orientation::Vertical).spacing(12).build();
	content.append(&list);
	content.append(&everything);
	content.append(&note);
	dialog.set_extra_child(Some(&content));
	dialog.add_response("cancel", "_Cancel");
	dialog.add_response("clean", "");
	dialog.set_close_response("cancel");

	let parts = std::rc::Rc::new(parts);
	let update = {
		let (dialog, parts, all) = (dialog.clone(), parts.clone(), all.clone());
		move || {
			let everything = all.is_active();
			for (_, _, _, row) in parts.iter() {
				row.set_sensitive(!everything);
			}
			let bytes: u64 = parts.iter().filter(|(_, c, _, _)| c.is_active()).map(|(_, _, b, _)| b).sum();
			if everything {
				dialog.set_response_label("clean", "_Delete All Mail");
				dialog.set_response_appearance("clean", adw::ResponseAppearance::Destructive);
			} else {
				dialog.set_response_label("clean", &format!("_Free {}", format::size(bytes)));
				dialog.set_response_appearance("clean", adw::ResponseAppearance::Suggested);
			}
			dialog.set_response_enabled("clean", everything || parts.iter().any(|(_, c, _, _)| c.is_active()));
			dialog.set_default_response(Some("clean"));
		}
	};
	update();
	let update = std::rc::Rc::new(update);
	for check in parts.iter().map(|(_, c, _, _)| c).chain([&all]) {
		let update = update.clone();
		check.connect_toggled(move |_| update());
	}
	let (engine, ticks) = (ctx.engine.clone(), all.clone());
	dialog.connect_response(None, move |_, response| {
		if response != "clean" {
			return engine.send(Action::CloseSheet);
		}
		let ticked = |part: CleanupPart| parts.iter().any(|(p, c, _, _)| *p == part && c.is_active());
		engine.send(Action::RunCleanup { older: ticked(CleanupPart::Older), vectors: ticked(CleanupPart::Vectors), model: ticked(CleanupPart::Model), all: ticks.is_active() });
	});
	(dialog, all)
}

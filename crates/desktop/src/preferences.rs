//! Preferences, drawn from what the core publishes, with its Rules section.

use std::rc::Rc;

use adw::prelude::*;
use gtk::glib;
use relm4::prelude::*;
use tern_app::view::{Action, ChoiceView, SettingsView};

use crate::rule_editor::Rules;
use crate::{default_app, window::Ctx};

pub struct Preferences {
	view: SettingsView,
	pub rules: Rc<Controller<Rules>>,
}

/// One of the choices the core offers.
fn choice_row(ctx: &Ctx, title: &str, subtitle: Option<&str>, choices: &[ChoiceView], action: fn(String) -> Action) -> adw::ComboRow {
	let names: Vec<&str> = choices.iter().map(|c| c.label.as_str()).collect();
	let row = adw::ComboRow::builder().title(title).model(&gtk::StringList::new(&names)).build();
	if let Some(subtitle) = subtitle {
		row.set_subtitle(subtitle);
	}
	row.set_selected(choices.iter().position(|c| c.selected).unwrap_or(0) as u32);
	let (ctx, values): (_, Vec<String>) = (ctx.clone(), choices.iter().map(|c| c.value.clone()).collect());
	row.connect_selected_notify(move |row| {
		if let Some(value) = values.get(row.selected() as usize) {
			ctx.engine.send(action(value.clone()));
		}
	});
	row
}

#[relm4::component(pub)]
impl SimpleComponent for Preferences {
	type Init = Ctx;
	type Input = SettingsView;
	type Output = ();

	view! {
		adw::PreferencesDialog {
			// Tall enough for a rule's page without scrolling.
			set_content_height: 680,
			connect_closed[ctx] => move |dialog| {
				ctx.engine.send(Action::CloseSettings);
				// Not from inside the dialog's own signal.
				let (ctx, dialog) = (ctx.clone(), dialog.clone());
				glib::idle_add_local_once(move || ctx.forget_preferences(&dialog));
			},
			add = &adw::PreferencesPage {
				add = &adw::PreferencesGroup {
					set_title: "Offline Mail",
					set_description: Some("Tern keeps recent mail on this computer, encrypted, so it shows at once and can be read offline. It checks for new mail every few minutes."),
					add: &choice_row(&ctx, "Keep Ready", None, &model.view.keep_ready, |value| Action::SetKeepReady { value }),
					add = &adw::SwitchRow {
						set_title: "Forget Mail on Sign Out",
						set_subtitle: "Deletes the mail kept on this computer when you sign out. Signing in again downloads it afresh.",
						set_active: model.view.forget_on_sign_out,
						connect_active_notify[engine = ctx.engine.clone()] => move |row| engine.send(Action::SetForget { on: row.is_active() }),
					},
					add: &choice_row(&ctx, "Offer Cleanup Past", Some("The sidebar offers to clean up once mail here takes more than this."), &model.view.cleanup_limits, |value| Action::SetCleanupLimit { value }),
					// How much Tern takes on disk, with Clean Up always to hand whatever the size.
					add = &adw::ActionRow {
						set_title: "Storage",
						set_visible: !model.view.address.is_empty(),
						#[watch]
						set_subtitle: &model.view.storage_line,
						add_suffix = &gtk::Button {
							set_label: "Clean Up…",
							set_valign: gtk::Align::Center,
							connect_clicked[engine = ctx.engine.clone()] => move |_| engine.send(Action::OpenCleanup),
						},
					},
				},
				// Search by meaning is off until asked for, as it downloads a model once.
				add = &adw::PreferencesGroup {
					set_title: "Search",
					set_description: Some("Search looks through the mail kept on this computer."),
					add = &adw::SwitchRow {
						set_title: "Search by Meaning",
						// The download's progress, and the switch going back off if it fails.
						#[watch]
						set_subtitle: &model.view.semantic_line,
						#[watch]
						#[block_signal(meaning)]
						set_active: model.view.semantic,
						connect_active_notify[engine = ctx.engine.clone()] => move |row| engine.send(Action::SetSemantic { on: row.is_active() }) @meaning,
					},
				},
				add: &default_app::group(),
				add: model.rules.widget(),
			},
		}
	}

	fn init(ctx: Ctx, root: Self::Root, _sender: ComponentSender<Self>) -> ComponentParts<Self> {
		let view = ctx.settings_view().unwrap_or_else(|| panic!("the core publishes settings at once"));
		let rules = Rc::new(Rules::builder().launch((ctx.clone(), root.clone())).detach());
		let model = Preferences { view, rules };
		let widgets = view_output!();
		ComponentParts { model, widgets }
	}

	fn update(&mut self, view: SettingsView, _: ComponentSender<Self>) {
		self.view = view;
	}
}

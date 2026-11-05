//! A window for writing one mail. What happens to it is the core's: this draws one composer of those
//! it publishes, and sends back what is written.

use std::sync::Arc;

use adw::prelude::*;
use gtk::{gdk, gio, glib};
use relm4::factory::FactoryVecDeque;
use relm4::prelude::*;
use relm4::RelmWidgetExt;
use tern_app::view::{Action, AddressField, ComposeText, ComposerAttachment, ComposerView};
use tern_app::Engine;
use tern_compose::mime_for;

use crate::activity::Word;
use crate::suggest::{self, Suggest};
use crate::{attachments::icon_for, runtime, window::Ctx};

/// One of the address lines, with the suggestions under it.
struct Address {
	field: AddressField,
	entry: gtk::Entry,
	/// Blocked while drawing what the core said, which is not the person typing.
	typed: glib::SignalHandlerId,
	suggest: Controller<Suggest>,
}

/// A label and a frameless entry, as one line of the header.
fn field(label: &str) -> (gtk::Box, gtk::Entry) {
	relm4::view! {
		row = gtk::Box {
			set_spacing: 12,
			set_margin_start: 12,
			set_margin_end: 12,
			set_margin_top: 4,
			set_margin_bottom: 4,
			gtk::Label {
				set_label: label,
				set_xalign: 1.0,
				set_width_chars: 7,
				add_css_class: "dim-label",
			},
			#[name = "entry"]
			gtk::Entry {
				set_has_frame: false,
				set_hexpand: true,
				update_property: &[gtk::accessible::Property::Label(label)],
			},
		}
	}
	(row, entry)
}

/// An attached file, with a button to take it off.
struct File {
	attachment: ComposerAttachment,
	/// Its place among all the composer's attachments, which is how the core knows it.
	index: u32,
}

#[relm4::factory]
impl FactoryComponent for File {
	type Init = (ComposerAttachment, u32);
	type Input = ();
	type Output = u32;
	type CommandOutput = ();
	type ParentWidget = gtk::ListBox;

	view! {
		adw::ActionRow {
			set_title: &glib::markup_escape_text(&self.attachment.name),
			set_subtitle: &self.attachment.size,
			add_prefix = &gtk::Image {
				set_from_gicon: &icon_for(&self.attachment.mime),
			},
			add_suffix = &gtk::Button {
				set_icon_name: "edit-delete-symbolic",
				set_valign: gtk::Align::Center,
				add_css_class: "flat",
				set_tooltip_text: Some("Remove"),
				connect_clicked[sender, index = self.index] => move |_| {
					let _ = sender.output(index);
				},
			},
		}
	}

	fn init_model((attachment, index): Self::Init, _: &DynamicIndex, _: FactorySender<Self>) -> Self {
		File { attachment, index }
	}
}

pub struct Writing {
	ctx: Ctx,
	id: u64,
	view: ComposerView,
	/// The subject as written, which titles the window.
	subject: String,
	subject_entry: gtk::Entry,
	cc_shown: bool,
	addresses: [Address; 3],
	files: FactoryVecDeque<File>,
	/// Files being read from disk, on their way to the core.
	reading: usize,
	/// How many problems have been shown, for the screenshot scenes.
	toasts_shown: u32,
}

#[derive(Debug)]
pub enum Input {
	View(Box<ComposerView>),
	Typed(AddressField, String),
	Subject(String),
	Take(AddressField, u32),
	ShowCc(bool),
	SubjectDone,
	DropPicked,
	IncludeWhole(bool),
	RemoveFile(u32),
	Add(Vec<gio::File>),
	Read(Result<(String, Vec<u8>), String>),
	PickFiles,
	Send,
	Discard,
	Close,
	Escape,
}

impl Writing {
	fn act(&self, action: Action) {
		act(&self.ctx.engine, self.id, action);
	}

	fn address(&self, field: AddressField) -> &Address {
		match field {
			AddressField::To => &self.addresses[0],
			AddressField::Cc => &self.addresses[1],
			AddressField::Bcc => &self.addresses[2],
		}
	}

	fn title(&self) -> &str {
		match self.subject.trim() {
			"" => "New Message",
			subject => subject,
		}
	}

	fn can_send(&self) -> bool {
		!self.view.sending && self.reading == 0 && !self.view.attachments.iter().any(|a| a.pending)
	}

	/// What is written now.
	fn text(&self, widgets: &WritingWidgets) -> ComposeText {
		ComposeText {
			to: self.addresses[0].entry.text().into(),
			cc: self.addresses[1].entry.text().into(),
			bcc: self.addresses[2].entry.text().into(),
			subject: self.subject_entry.text().into(),
			body: body_text(&widgets.body),
			from_index: widgets.from.selected(),
		}
	}

	/// The text is the person's while they write; the core only sets it when it changes it itself.
	fn draw_text(&mut self, widgets: &WritingWidgets, view: &ComposerView) {
		for (address, text) in self.addresses.iter().zip([&view.text.to, &view.text.cc, &view.text.bcc]) {
			if address.entry.text() != text.as_str() {
				address.entry.block_signal(&address.typed);
				address.entry.set_text(text);
				address.entry.set_position(-1);
				address.entry.unblock_signal(&address.typed);
			}
		}
		if self.subject_entry.text() != view.text.subject.as_str() {
			self.subject_entry.set_text(&view.text.subject);
			self.subject_entry.set_position(-1);
		}
		self.subject.clone_from(&view.text.subject);
		let buffer = widgets.body.buffer();
		if body_text(&widgets.body) != view.text.body {
			buffer.set_text(&view.text.body);
			buffer.place_cursor(&buffer.start_iter());
		}
		widgets.from.set_selected(view.text.from_index);
	}

	fn draw_files(&mut self, attachments: &[ComposerAttachment]) {
		let mut files = self.files.guard();
		files.clear();
		for (i, attachment) in attachments.iter().enumerate().filter(|(_, a)| !a.pending) {
			files.push_back((attachment.clone(), i as u32));
		}
	}

	/// Whatever a new word from the core changes beyond what the view macro watches.
	fn draw(&mut self, widgets: &WritingWidgets, view: ComposerView, root: &adw::Window) {
		let before = std::mem::replace(&mut self.view, view.clone());
		if before.revision != view.revision {
			self.draw_text(widgets, &view);
		}
		self.cc_shown |= view.show_cc;
		if before.attachments != view.attachments {
			self.draw_files(&view.attachments);
		}
		let fields = [(AddressField::To, view.bad_to, before.bad_to), (AddressField::Cc, view.bad_cc, before.bad_cc), (AddressField::Bcc, view.bad_bcc, before.bad_bcc)];
		for (field, bad, was) in fields {
			if bad && !was {
				self.address(field).entry.grab_focus();
			}
		}
		// Trying again and getting the same answer is worth saying again.
		let told = before.error == view.error && before.tries == view.tries;
		if let Some(error) = view.error.as_ref().filter(|_| !told) {
			let toast = adw::Toast::new(error);
			toast.set_timeout(5);
			widgets.toasts.add_toast(toast);
			self.toasts_shown += 1;
		}
		self.suggest(&view);
		// Saving goes on behind the screen; it comes back if that fails.
		if view.saving && !before.saving {
			root.set_visible(false);
		} else if before.saving && !view.saving {
			root.present();
		}
	}

	fn suggest(&self, view: &ComposerView) {
		for address in &self.addresses {
			let found = if view.suggest_field == Some(address.field) { view.suggestions.clone() } else { Vec::new() };
			address.suggest.emit(suggest::Input::Found(found));
		}
	}
}

/// Tells the core something about this composer.
fn act(engine: &Arc<Engine>, id: u64, action: Action) {
	engine.send(Action::ComposeFocus { id });
	engine.send(action);
}

fn body_text(body: &gtk::TextView) -> String {
	let buffer = body.buffer();
	buffer.text(&buffer.start_iter(), &buffer.end_iter(), false).into()
}

#[relm4::component(pub)]
impl Component for Writing {
	type Init = (Ctx, ComposerView);
	type Input = Input;
	type Output = ();
	type CommandOutput = ();

	view! {
		adw::Window {
			set_default_width: 720,
			set_default_height: 640,
			set_size_request: (360, 360),
			set_transient_for: Some(&ctx.window),
			set_application: ctx.window.application().as_ref(),
			#[watch]
			set_title: Some(model.title()),
			connect_close_request[sender] => move |_| {
				// Closing keeps the message, which is the core's to do; it says when this may go.
				sender.input(Input::Close);
				glib::Propagation::Stop
			},

			#[wrap(Some)]
			set_content = &adw::ToolbarView {
				add_top_bar = &adw::HeaderBar {
					#[wrap(Some)]
					set_title_widget = &adw::WindowTitle {
						#[watch]
						set_title: model.title(),
					},
					pack_start = &gtk::Button {
						set_icon_name: "mail-attachment-symbolic",
						set_tooltip_text: Some("Attach Files… (Ctrl+Shift+A)"),
						connect_clicked => Input::PickFiles,
					},
					pack_end = &gtk::Button {
						add_css_class: "suggested-action",
						set_tooltip_text: Some("Send (Ctrl+Enter)"),
						#[watch]
						set_sensitive: model.can_send(),
						connect_clicked => Input::Send,
						#[wrap(Some)]
						set_child = &gtk::Stack {
							#[watch]
							set_visible_child_name: if model.view.sending { "sending" } else { "send" },
							add_named[Some("send")] = &gtk::Label {
								set_label: "Send",
							},
							add_named[Some("sending")] = &adw::Spinner {},
						},
					},
					pack_end = &gtk::Button {
						set_icon_name: "user-trash-symbolic",
						set_tooltip_text: Some("Discard"),
						connect_clicked => Input::Discard,
					},
					pack_end: &indicator,
				},

				#[wrap(Some)]
				#[name = "toasts"]
				set_content = &adw::ToastOverlay {
					#[wrap(Some)]
					#[name = "content"]
					set_child = &gtk::Box {
						set_orientation: gtk::Orientation::Vertical,
						add_css_class: "view",

						// The address to send from, among the account's; shown only when it has more than one.
						gtk::Box {
							set_spacing: 12,
							set_margin_start: 12,
							set_margin_end: 12,
							set_margin_top: 4,
							set_margin_bottom: 4,
							set_visible: view.addresses.len() > 1,
							gtk::Label {
								set_label: "From",
								set_xalign: 1.0,
								set_width_chars: 7,
								add_css_class: "dim-label",
							},
							#[name = "from"]
							gtk::DropDown::from_strings(&view.addresses.iter().map(String::as_str).collect::<Vec<_>>()) {
								add_css_class: "flat",
								update_property: &[gtk::accessible::Property::Label("From")],
							},
						},
						gtk::Separator {
							set_visible: view.addresses.len() > 1,
						},
						#[local_ref]
						to_row -> gtk::Box {
							#[name = "cc_toggle"]
							append = &gtk::ToggleButton {
								set_label: "Cc/Bcc",
								set_css_classes: &["flat", "caption"],
								set_valign: gtk::Align::Center,
								set_tooltip_text: Some("Show Cc and Bcc"),
								#[watch]
								#[block_signal(cc_toggled)]
								set_active: model.cc_shown,
								connect_toggled[sender] => move |toggle| sender.input(Input::ShowCc(toggle.is_active())) @cc_toggled,
							},
						},
						gtk::Separator {
							#[watch]
							set_visible: model.cc_shown,
						},
						#[local_ref]
						cc_row -> gtk::Box {
							#[watch]
							set_visible: model.cc_shown,
						},
						gtk::Separator {
							#[watch]
							set_visible: model.cc_shown,
						},
						#[local_ref]
						bcc_row -> gtk::Box {
							#[watch]
							set_visible: model.cc_shown,
						},
						gtk::Separator {},
						#[local_ref]
						subject_row -> gtk::Box {},
						gtk::Separator {},

						gtk::ScrolledWindow {
							set_hscrollbar_policy: gtk::PolicyType::Never,
							set_vexpand: true,
							#[wrap(Some)]
							set_child = &gtk::Box {
								set_orientation: gtk::Orientation::Vertical,

								// The lines picked in the reader, above the text, with an × to leave them out.
								gtk::Box {
									set_orientation: gtk::Orientation::Vertical,
									add_css_class: "quote-picked",
									set_margin_top: 18,
									set_margin_start: 24,
									set_margin_end: 24,
									#[watch]
									set_visible: model.view.picked_text.is_some(),
									gtk::Box {
										set_spacing: 6,
										gtk::Label {
											set_xalign: 0.0,
											set_hexpand: true,
											set_css_classes: &["caption", "quote-who"],
											#[watch]
											set_label: model.view.picked_who.as_deref().unwrap_or_default(),
										},
										gtk::Button {
											set_icon_name: "window-close-symbolic",
											set_tooltip_text: Some("Leave This Quote Out"),
											set_css_classes: &["circular", "quote-button"],
											set_valign: gtk::Align::Start,
											connect_clicked => Input::DropPicked,
										},
									},
									gtk::Label {
										set_xalign: 0.0,
										set_wrap: true,
										set_wrap_mode: gtk::pango::WrapMode::WordChar,
										set_selectable: true,
										#[watch]
										set_label: model.view.picked_text.as_deref().unwrap_or_default(),
									},
								},

								#[name = "body"]
								gtk::TextView {
									set_wrap_mode: gtk::WrapMode::WordChar,
									set_top_margin: 18,
									set_bottom_margin: 18,
									set_left_margin: 24,
									set_right_margin: 24,
									set_vexpand: true,
									set_accepts_tab: false,
									update_property: &[gtk::accessible::Property::Label("Message")],
								},

								// The whole earlier mail, under the text: ••• to look at it, and a button to
								// add it or an × to leave it out.
								gtk::Box {
									add_css_class: "quote-pill",
									set_halign: gtk::Align::Start,
									set_margin_start: 24,
									set_margin_bottom: 12,
									#[watch]
									set_visible: model.has_whole(),
									#[watch]
									set_class_active: ("included", model.view.whole_included),
									gtk::ToggleButton {
										set_label: "•••",
										set_css_classes: &["quote-button", "quote-dots"],
										#[watch]
										set_tooltip_text: Some(if model.view.whole_forward { "Show the Forwarded Mail" } else { "Show the Earlier Mail" }),
										connect_toggled[whole] => move |dots| whole.set_reveal_child(dots.is_active()),
									},
									gtk::Separator {
										set_orientation: gtk::Orientation::Vertical,
									},
									gtk::Label {
										set_css_classes: &["caption", "quote-label"],
										set_ellipsize: gtk::pango::EllipsizeMode::End,
										#[watch]
										set_visible: model.view.whole_included,
										#[watch]
										set_label: model.view.whole_label.as_deref().unwrap_or_default(),
									},
									gtk::Separator {
										set_orientation: gtk::Orientation::Vertical,
										#[watch]
										set_visible: model.view.whole_included,
									},
									gtk::Button {
										set_icon_name: "window-close-symbolic",
										add_css_class: "quote-button",
										#[watch]
										set_visible: model.view.whole_included,
										#[watch]
										set_tooltip_text: Some(if model.view.whole_forward { "Leave the Forwarded Mail Out" } else { "Leave the Quote Out" }),
										connect_clicked => Input::IncludeWhole(false),
									},
									gtk::Button {
										set_css_classes: &["flat", "caption", "quote-add"],
										#[watch]
										set_visible: !model.view.whole_included,
										#[watch]
										set_label: &format!("+ Quote {}", model.view.whole_label.as_deref().unwrap_or_default()),
										connect_clicked => Input::IncludeWhole(true),
									},
								},
								#[name = "whole"]
								gtk::Revealer {
									set_transition_type: gtk::RevealerTransitionType::SlideDown,
									#[watch]
									set_visible: model.has_whole(),
									#[wrap(Some)]
									set_child = &gtk::Label {
										set_xalign: 0.0,
										set_wrap: true,
										set_wrap_mode: gtk::pango::WrapMode::WordChar,
										set_selectable: true,
										add_css_class: "quote-whole",
										set_margin_start: 24,
										set_margin_end: 24,
										set_margin_bottom: 18,
										#[watch]
										set_class_active: ("left-out", !model.view.whole_included),
										#[watch]
										set_label: model.view.whole_text.as_deref().unwrap_or_default().trim_end(),
									},
								},
							},
						},

						gtk::ScrolledWindow {
							set_hscrollbar_policy: gtk::PolicyType::Never,
							set_propagate_natural_height: true,
							set_max_content_height: 220,
							#[wrap(Some)]
							set_child = &gtk::Box {
								set_orientation: gtk::Orientation::Vertical,
								set_spacing: 6,
								set_margin_start: 12,
								set_margin_end: 12,
								set_margin_top: 6,
								set_margin_bottom: 12,
								#[watch]
								set_visible: !model.view.attachments.is_empty(),
								gtk::Label {
									set_xalign: 0.0,
									set_css_classes: &["caption", "dim-label"],
									#[watch]
									set_label: &files_summary(&model.view.attachments, model.view.total.as_deref()),
								},
								#[local_ref]
								files -> gtk::ListBox {
									set_selection_mode: gtk::SelectionMode::None,
									add_css_class: "boxed-list",
								},
							},
						},
					},
				},
			},
		}
	}

	fn init(init: Self::Init, root: Self::Root, sender: ComponentSender<Self>) -> ComponentParts<Self> {
		let (ctx, view) = init;
		let indicator = ctx.activity.indicator();
		let (to_row, to) = field("To");
		let (cc_row, cc) = field("Cc");
		let (bcc_row, bcc) = field("Bcc");
		let (subject_row, subject_entry) = field("Subject");
		let addresses = [(AddressField::To, to), (AddressField::Cc, cc), (AddressField::Bcc, bcc)].map(|(field, entry)| {
			let typing = sender.clone();
			let typed = entry.connect_changed(move |entry| typing.input(Input::Typed(field, entry.text().to_string())));
			let suggest = Suggest::builder().launch(entry.clone()).forward(sender.input_sender(), move |suggest::Take(index)| Input::Take(field, index));
			Address { field, entry, typed, suggest }
		});
		let files = FactoryVecDeque::builder().launch(gtk::ListBox::default()).forward(sender.input_sender(), Input::RemoveFile);
		let mut model = Writing {
			id: view.id,
			view: view.clone(),
			subject: view.text.subject.clone(),
			subject_entry: subject_entry.clone(),
			cc_shown: view.show_cc,
			addresses,
			files,
			reading: 0,
			toasts_shown: 0,
			ctx: ctx.clone(),
		};
		let files = model.files.widget().clone();
		let widgets = view_output!();
		subject_entry.connect_changed({
			let sender = sender.clone();
			move |entry| sender.input(Input::Subject(entry.text().to_string()))
		});
		subject_entry.connect_activate({
			let sender = sender.clone();
			move |_| sender.input(Input::SubjectDone)
		});
		model.draw_text(&widgets, &view);
		model.draw_files(&view.attachments);
		model.suggest(&view);
		connect_drop(&widgets.content, &sender);
		connect_keys(&root, &sender);

		root.present();
		for address in &model.addresses {
			address.entry.select_region(0, 0);
		}
		model.subject_entry.select_region(0, 0);
		if view.text.to.is_empty() {
			model.addresses[0].entry.grab_focus();
		} else {
			widgets.body.grab_focus();
		}
		ComponentParts { model, widgets }
	}

	fn update_with_view(&mut self, widgets: &mut Self::Widgets, input: Input, sender: ComponentSender<Self>, root: &Self::Root) {
		let mut focus_cc = false;
		match input {
			Input::View(view) => self.draw(widgets, *view, root),
			Input::Typed(field, text) => {
				if self.view.error.is_some() || self.view.bad_to || self.view.bad_cc || self.view.bad_bcc {
					self.act(Action::ComposeEdited);
				}
				self.act(Action::ComposeSuggest { field, text });
			},
			Input::Subject(subject) => self.subject = subject,
			Input::Take(field, index) => self.act(Action::ComposeTakeSuggestion { text: self.text(widgets), field, index }),
			Input::ShowCc(show) => {
				self.cc_shown = show;
				focus_cc = show;
			},
			Input::SubjectDone => {
				widgets.body.grab_focus();
			},
			Input::DropPicked => self.act(Action::ComposeDropPicked),
			Input::IncludeWhole(included) => self.act(Action::ComposeIncludeWhole { included }),
			Input::RemoveFile(index) => self.act(Action::ComposeRemoveAttachment { index }),
			Input::Add(files) => self.read(files, &sender),
			Input::Read(result) => {
				self.reading -= 1;
				match result {
					Ok((name, data)) => self.act(Action::ComposeAttach { mime: mime_for(&name).into(), name, data }),
					Err(name) => self.act(Action::ComposeAttachFailed { name }),
				}
			},
			Input::PickFiles => pick_files(root, &sender),
			Input::Send => self.act(Action::ComposeSend { text: self.text(widgets) }),
			Input::Discard => {
				// Throws the mail away, once the core has asked if it is worth asking.
				self.ctx.ask_discard_from(self.id, root);
				self.act(Action::ComposeDiscard { text: self.text(widgets) });
			},
			Input::Close => self.act(Action::ComposeClose { text: self.text(widgets) }),
			// Escape first closes open address suggestions.
			Input::Escape => {
				if !self.addresses.iter().any(|a| suggest::dismiss(&a.suggest)) {
					root.close();
				}
			},
		}
		self.update_view(widgets, sender);
		if focus_cc {
			self.addresses[1].entry.grab_focus();
		}
	}
}

impl Writing {
	fn has_whole(&self) -> bool {
		self.view.whole_label.is_some() && self.view.whole_text.is_some()
	}

	fn read(&mut self, files: Vec<gio::File>, sender: &ComponentSender<Self>) {
		for file in files {
			self.reading += 1;
			let busy = self.ctx.activity.start(Word::Attaching);
			let sender = sender.clone();
			runtime::spawn_local(async move {
				let name = file.basename().map(|p| p.to_string_lossy().into_owned()).unwrap_or_else(|| "attachment".into());
				let result = file.load_contents_future().await;
				drop(busy);
				sender.input(Input::Read(match result {
					Ok((bytes, _)) => Ok((name, bytes.to_vec())),
					Err(_) => Err(name),
				}));
			});
		}
	}
}

/// What the attachments come to, and how many are still on their way.
fn files_summary(attachments: &[ComposerAttachment], total: Option<&str>) -> String {
	let waiting = attachments.iter().filter(|a| a.pending).count();
	let size = total.unwrap_or_default();
	let mut summary = match attachments.len() - waiting {
		0 => String::new(),
		1 => format!("1 attachment · {size}"),
		n => format!("{n} attachments · {size}"),
	};
	if waiting > 0 {
		if !summary.is_empty() {
			summary.push_str(" · ");
		}
		summary.push_str(&format!("downloading {waiting}…"));
	}
	summary
}

fn pick_files(root: &adw::Window, sender: &ComponentSender<Writing>) {
	let (root, sender) = (root.clone(), sender.clone());
	runtime::spawn_local(async move {
		let dialog = gtk::FileDialog::builder().title("Attach Files").modal(true).build();
		let Ok(list) = dialog.open_multiple_future(Some(&root)).await else { return };
		sender.input(Input::Add((0..list.n_items()).filter_map(|i| list.item(i).and_downcast::<gio::File>()).collect()));
	});
}

fn connect_drop(widget: &gtk::Box, sender: &ComponentSender<Writing>) {
	let target = gtk::DropTarget::new(gdk::FileList::static_type(), gdk::DragAction::COPY);
	let sender = sender.clone();
	target.connect_drop(move |_, value, _, _| {
		let Ok(list) = value.get::<gdk::FileList>() else { return false };
		sender.input(Input::Add(list.files()));
		true
	});
	widget.add_controller(target);
}

fn connect_keys(root: &adw::Window, sender: &ComponentSender<Writing>) {
	let keys = gtk::ShortcutController::new();
	keys.set_propagation_phase(gtk::PropagationPhase::Capture);
	for (trigger, input) in [("<Control>Return|<Control>KP_Enter", 0), ("<Control>w|Escape", 1), ("<Control><Shift>a", 2)] {
		let sender = sender.clone();
		let action = gtk::CallbackAction::new(move |_, _| {
			sender.input(match input {
				0 => Input::Send,
				1 => Input::Escape,
				_ => Input::PickFiles,
			});
			glib::Propagation::Stop
		});
		keys.add_shortcut(gtk::Shortcut::new(gtk::ShortcutTrigger::parse_string(trigger), Some(action)));
	}
	root.add_controller(keys);
}

/// A composer on screen: the component, and what the rest of the app and the scenes do with it.
pub struct Composer {
	engine: Arc<Engine>,
	pub id: u64,
	pub window: adw::Window,
	pub from: gtk::DropDown,
	controller: Controller<Writing>,
}

impl Composer {
	/// Opens the window for a composer the core has just opened.
	pub fn open(ctx: &Ctx, view: &ComposerView) -> Composer {
		let controller = Writing::builder().launch((ctx.clone(), view.clone())).detach();
		let (window, from) = (controller.widget().clone(), controller.widgets().from.clone());
		Composer { engine: ctx.engine.clone(), id: view.id, window, from, controller }
	}

	/// Draws what the core says about this composer.
	pub fn show(&self, view: &ComposerView) {
		self.controller.emit(Input::View(Box::new(view.clone())));
	}

	/// The core has finished with this composer.
	pub fn finish(&self) {
		// Closing would only close a dialog open over it, such as the one asking to discard.
		self.window.destroy();
	}

	fn act(&self, action: Action) {
		act(&self.engine, self.id, action);
	}

	/// Leaves out the lines picked in the reader.
	pub fn drop_picked(&self) {
		self.act(Action::ComposeDropPicked);
	}

	/// Puts the whole earlier mail in the mail, or leaves it out.
	pub fn include_whole(&self, included: bool) {
		self.act(Action::ComposeIncludeWhole { included });
	}

	pub fn send(&self) {
		self.controller.emit(Input::Send);
	}

	/// For the screenshot scenes: what the core says about this composer.
	pub fn view(&self) -> Option<ComposerView> {
		Some(self.controller.model().view.clone())
	}

	/// For the screenshot scenes: throws the mail away, as the button does.
	pub fn discard(&self) {
		self.controller.emit(Input::Discard);
	}

	/// For the screenshot scenes: how many problems have been shown.
	pub fn toasts_shown(&self) -> u32 {
		self.controller.model().toasts_shown
	}

	/// For the screenshot scenes: types into To.
	pub fn type_to(&self, text: &str) {
		let model = self.controller.model();
		let to = &model.addresses[0].entry;
		to.grab_focus();
		to.set_text(text);
		to.set_position(-1);
	}

	/// For the screenshot scenes: how many suggestions are open under To.
	pub fn suggested(&self) -> usize {
		suggest::open_count(&self.controller.model().addresses[0].suggest)
	}

	/// For the screenshot scenes: types `text` as the message.
	pub fn type_body(&self, text: &str) {
		self.controller.widgets().body.buffer().set_text(text);
	}

	/// For the screenshot scenes: the message as written, quotes apart.
	pub fn body_text(&self) -> String {
		body_text(&self.controller.widgets().body)
	}

	/// For the screenshot scenes: the subject and To as written.
	pub fn subject_and_to(&self) -> (String, String) {
		(self.controller.model().subject_entry.text().into(), self.controller.model().addresses[0].entry.text().into())
	}

	/// For the screenshot scenes: adds an attachment, as picking a file does.
	pub fn attach(&self, name: &str, mime: &str, data: &[u8]) {
		self.act(Action::ComposeAttach { name: name.into(), mime: mime.into(), data: data.to_vec() });
	}

	/// For the screenshot scenes: adds files, as dragging them in does.
	pub fn drop_files(&self, files: Vec<gio::File>) {
		self.controller.emit(Input::Add(files));
	}
}

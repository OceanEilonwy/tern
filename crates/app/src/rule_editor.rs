//! The Rules section of Preferences: the rules in a row that opens up, each edited on a page of its
//! own with the mail it would catch counted when typing pauses.

use std::cell::RefCell;
use std::rc::Rc;
use std::time::Duration;

use adw::prelude::*;
use gtk::glib;
use tern_core::rules::{parse, Action, Rule};
use tern_core::{Folder, FolderKind};

use crate::window::Ctx;
use crate::{rules, runtime};

const HELP: &str = "from:, to: and subject: look at the sender, the first recipient and the subject; a word on its own looks at the subject and the sender. @example.com is a whole domain. Join terms with and, or and not, and group them in brackets: (from:@example.com or subject:\"offer\") and not to:me@tuta.com";

/// What a rule can do, in the order offered.
const DOES: [&str; 3] = ["Move to Spam", "Move to Folder", "Add Label"];

pub struct Rules {
	ctx: Ctx,
	dialog: adw::PreferencesDialog,
	pub group: adw::PreferencesGroup,
	pub expander: adw::ExpanderRow,
	rows: RefCell<Vec<gtk::Widget>>,
	rules: RefCell<Vec<Rule>>,
}

impl Rules {
	pub fn new(ctx: &Ctx, dialog: &adw::PreferencesDialog) -> Rc<Rules> {
		let group = adw::PreferencesGroup::builder().title("Rules").description("Rules sort new mail in the Inbox as it arrives.").build();
		let expander = adw::ExpanderRow::builder().title("Mail Rules").build();
		group.add(&expander);
		let this = Rc::new(Rules {
			ctx: ctx.clone(),
			dialog: dialog.clone(),
			group,
			expander,
			rows: RefCell::default(),
			rules: RefCell::new(rules::load()),
		});
		this.fill();
		this
	}

	/// One row per rule, then Add Rule.
	fn fill(self: &Rc<Self>) {
		for row in self.rows.borrow_mut().drain(..) {
			self.expander.remove(&row);
		}
		let rules = self.rules.borrow().clone();
		self.expander.set_subtitle(&match rules.len() {
			0 => "None yet".to_string(),
			1 => "1 rule".to_string(),
			n => format!("{n} rules"),
		});
		let mut rows = Vec::new();
		for (i, rule) in rules.iter().enumerate() {
			let broken = parse(&rule.when).is_err();
			let row = adw::ActionRow::builder()
				.title(glib::markup_escape_text(&rule.when))
				.title_lines(2)
				.subtitle(if broken { "Does nothing until fixed".to_string() } else { rules::describe(&rule.action) })
				.activatable(true)
				.css_classes(["rule"])
				.build();
			row.add_suffix(&gtk::Image::from_icon_name("go-next-symbolic"));
			if broken {
				row.add_prefix(&gtk::Image::builder().icon_name("dialog-warning-symbolic").css_classes(["warning"]).build());
			}
			let weak = Rc::downgrade(self);
			row.connect_activated(move |_| {
				if let Some(this) = weak.upgrade() {
					this.edit(Some(i));
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

	fn store(self: &Rc<Self>, rules: Vec<Rule>) {
		rules::save(&rules);
		*self.rules.borrow_mut() = rules;
		self.fill();
	}

	/// Deletes rule `i`, from its page.
	pub fn delete(self: &Rc<Self>, i: usize) {
		let mut all = self.rules.borrow().clone();
		if i < all.len() {
			all.remove(i);
			self.store(all);
		}
		self.dialog.pop_subpage();
	}

	/// Opens rule `i`, or a new one, on a page of its own.
	pub fn edit(self: &Rc<Self>, i: Option<usize>) -> Rc<Page> {
		let rule = i.and_then(|i| self.rules.borrow().get(i).cloned());
		let page = Page::new(self, i, rule);
		self.dialog.push_subpage(&page.page);
		page.line.grab_focus();
		page
	}
}

/// The page editing one rule.
pub struct Page {
	ctx: Ctx,
	page: adw::NavigationPage,
	pub line: adw::EntryRow,
	catches: gtk::Label,
	pub does: adw::ComboRow,
	pub folder: adw::ComboRow,
	pub label: adw::ComboRow,
	pub save: gtk::Button,
	folders: Vec<Folder>,
	labels: RefCell<Vec<String>>,
	/// The count of mail caught, waiting for typing to pause.
	pending: RefCell<Option<glib::SourceId>>,
}

impl Page {
	fn new(rules: &Rc<Rules>, i: Option<usize>, rule: Option<Rule>) -> Rc<Page> {
		let ctx = rules.ctx.clone();
		// Mail can move to any folder but the ones it only gets into by being written.
		let folders: Vec<Folder> = ctx
			.cache()
			.and_then(|c| c.folders().ok())
			.unwrap_or_default()
			.into_iter()
			.filter(|f| !matches!(f.kind, FolderKind::Drafts | FolderKind::Sent | FolderKind::Spam))
			.collect();
		let line = adw::EntryRow::builder().title("Mail Like").text(rule.as_ref().map_or("", |r| r.when.as_str())).build();
		line.add_css_class("monospace");
		let catches = gtk::Label::builder().xalign(0.0).wrap(true).css_classes(["caption"]).margin_top(6).build();
		let which = adw::PreferencesGroup::builder().title("Which Mail").description(HELP).build();
		which.add(&line);
		which.add(&catches);

		let does = adw::ComboRow::builder().title("Then").model(&gtk::StringList::new(&DOES)).build();
		let names: Vec<&str> = folders.iter().map(|f| f.name.as_str()).collect();
		let folder = adw::ComboRow::builder().title("Folder").model(&gtk::StringList::new(&names)).build();
		let label = adw::ComboRow::builder().title("Label").model(&gtk::StringList::new(&[])).build();
		let then = adw::PreferencesGroup::builder().title("What Happens").build();
		then.add(&does);
		then.add(&folder);
		then.add(&label);

		let save = gtk::Button::builder().label("Save").css_classes(["suggested-action"]).build();
		let header = adw::HeaderBar::new();
		header.pack_end(&save);
		let content = adw::PreferencesPage::new();
		content.add(&which);
		content.add(&then);
		if let Some(i) = i {
			let delete = adw::ButtonRow::builder().title("Delete Rule").css_classes(["destructive-action"]).build();
			let group = adw::PreferencesGroup::new();
			group.add(&delete);
			content.add(&group);
			let weak = Rc::downgrade(rules);
			delete.connect_activated(move |_| {
				if let Some(rules) = weak.upgrade() {
					rules.delete(i);
				}
			});
		}
		let view = adw::ToolbarView::new();
		view.add_top_bar(&header);
		view.set_content(Some(&content));
		let page = adw::NavigationPage::builder().title(if i.is_some() { "Edit Rule" } else { "New Rule" }).child(&view).build();

		let this = Rc::new(Page { ctx, page, line, catches, does, folder, label, save, folders, labels: RefCell::default(), pending: RefCell::default() });
		match rule.as_ref().map(|r| &r.action) {
			None | Some(Action::Spam) => this.does.set_selected(0),
			Some(Action::Move(name)) => {
				this.does.set_selected(1);
				if let Some(at) = this.folders.iter().position(|f| f.name.eq_ignore_ascii_case(name)) {
					this.folder.set_selected(at as u32);
				}
			},
			Some(Action::Label(_)) => this.does.set_selected(2),
		}
		this.load_labels(rule.and_then(|r| match r.action {
			Action::Label(name) => Some(name),
			_ => None,
		}));
		for row in [&this.does, &this.folder, &this.label] {
			let weak = Rc::downgrade(&this);
			row.connect_selected_notify(move |_| {
				if let Some(this) = weak.upgrade() {
					this.update();
				}
			});
		}
		let weak = Rc::downgrade(&this);
		this.line.connect_changed(move |_| {
			if let Some(this) = weak.upgrade() {
				this.update();
				this.count_soon();
			}
		});
		let (weak, rules, dialog) = (Rc::downgrade(&this), Rc::downgrade(rules), rules.dialog.clone());
		this.save.connect_clicked(move |_| {
			let (Some(this), Some(rules)) = (weak.upgrade(), rules.upgrade()) else { return };
			let Some(rule) = this.rule() else { return };
			let mut all = rules.rules.borrow().clone();
			match i {
				Some(i) if i < all.len() => all[i] = rule,
				_ => all.push(rule),
			}
			rules.store(all);
			dialog.pop_subpage();
		});
		this.update();
		this.count();
		this
	}

	fn count_soon(self: &Rc<Self>) {
		if let Some(id) = self.pending.borrow_mut().take() {
			id.remove();
		}
		let weak = Rc::downgrade(self);
		let id = glib::timeout_add_local_once(Duration::from_millis(150), move || {
			if let Some(this) = weak.upgrade() {
				this.pending.borrow_mut().take();
				this.count();
			}
		});
		*self.pending.borrow_mut() = Some(id);
	}

	/// Shows what the line catches among the mail here, or why it does not read.
	fn count(&self) {
		let line = self.line.text();
		let message = match parse(&line) {
			_ if line.trim().is_empty() => String::new(),
			Ok(expr) => {
				let skip = [FolderKind::Spam, FolderKind::Trash, FolderKind::Sent, FolderKind::Drafts];
				let n = self.ctx.cache().and_then(|c| c.matching(&expr, &skip, 0).ok()).map_or(0, |(_, n)| n);
				self.catches.remove_css_class("error");
				match n {
					0 => "Catches no mail here yet".to_string(),
					1 => "Catches 1 mail here already; rules only sort new mail".to_string(),
					n => format!("Catches {n} mails here already; rules only sort new mail"),
				}
			},
			Err(e) => {
				self.catches.add_css_class("error");
				e.message
			},
		};
		self.catches.set_label(&message);
		self.catches.set_visible(!message.is_empty());
	}

	/// The account's labels, fetched as the page opens; `keep` is selected once they are in.
	fn load_labels(self: &Rc<Self>, keep: Option<String>) {
		let Some(backend) = self.ctx.backend() else { return };
		let weak = Rc::downgrade(self);
		runtime::spawn_local(async move {
			let labels = runtime::run(async move { backend.labels().await }).await;
			let Some(this) = weak.upgrade() else { return };
			let mut names: Vec<String> = labels.unwrap_or_default().into_iter().map(|l| l.name).collect();
			// A label the rule names that is gone stays offered, so saving does not change it.
			if let Some(keep) = keep.as_ref().filter(|k| !names.iter().any(|n| n.eq_ignore_ascii_case(k))) {
				names.push(keep.clone());
			}
			this.label.set_model(Some(&gtk::StringList::new(&names.iter().map(String::as_str).collect::<Vec<_>>())));
			if let Some(at) = keep.and_then(|k| names.iter().position(|n| n.eq_ignore_ascii_case(&k))) {
				this.label.set_selected(at as u32);
			}
			*this.labels.borrow_mut() = names;
			this.update();
		});
	}

	/// The rule as the page has it, if it is whole.
	fn rule(&self) -> Option<Rule> {
		let when = self.line.text().trim().to_string();
		parse(&when).ok()?;
		let action = match self.does.selected() {
			0 => Action::Spam,
			1 => Action::Move(self.folders.get(self.folder.selected() as usize)?.name.clone()),
			_ => Action::Label(self.labels.borrow().get(self.label.selected() as usize)?.clone()),
		};
		Some(Rule { when, action })
	}

	/// Shows which choices apply, and whether the rule can be saved.
	fn update(&self) {
		let does = self.does.selected();
		self.folder.set_visible(does == 1);
		self.label.set_visible(does == 2);
		self.label.set_subtitle(if does == 2 && self.labels.borrow().is_empty() { "No labels yet; make them in Tuta" } else { "" });
		self.save.set_sensitive(self.rule().is_some());
	}
}

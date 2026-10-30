//! Draws a [`Doc`] into a read-only `gtk::TextView`.

use std::collections::HashMap;

use adw::prelude::*;
use gtk::{gdk, glib, pango};

use crate::html_doc::{Block, Cell, Doc, ImageSrc, Inline, Style};

const INDENT: i32 = 28;
const MAX_IMAGE_WIDTH: i32 = 640;
const MAX_IMAGE_HEIGHT: i32 = 560;

/// What a rendered body still needs: images to load, and where its links point.
#[derive(Default)]
pub struct Rendered {
	pub images: Vec<(gtk::Picture, ImageSrc)>,
	pub links: Vec<(gtk::TextTag, String)>,
}

struct Painter<'a> {
	buffer: gtk::TextBuffer,
	tags: HashMap<String, gtk::TextTag>,
	view: &'a gtk::TextView,
	out: Rendered,
	accent: gdk::RGBA,
	dim: gdk::RGBA,
	faint: gdk::RGBA,
}

/// Replaces what `view` shows with `doc`.
pub fn render(view: &gtk::TextView, doc: &Doc) -> Rendered {
	// Children can only be anchored in the view's own buffer, so attach it first.
	let buffer = gtk::TextBuffer::new(None);
	view.set_buffer(Some(&buffer));
	let dark = adw::StyleManager::default().is_dark();
	let accent = adw::StyleManager::default().accent_color().to_standalone_rgba(dark);
	let fg = view.color();
	let mut p = Painter {
		buffer: buffer.clone(),
		tags: HashMap::new(),
		view,
		out: Rendered::default(),
		accent,
		dim: gdk::RGBA::new(fg.red(), fg.green(), fg.blue(), 0.62),
		faint: gdk::RGBA::new(fg.red(), fg.green(), fg.blue(), 0.06),
	};
	p.blocks(&doc.blocks, 0, 0);
	// No trailing blank lines.
	let mut end = buffer.end_iter();
	let mut start = end;
	while start.backward_char() && start.char() == '\n' {}
	if start.char() != '\n' {
		start.forward_char();
	}
	buffer.delete(&mut start, &mut end);
	p.out
}

impl Painter<'_> {
	fn tag(&mut self, name: &str) -> gtk::TextTag {
		if let Some(t) = self.tags.get(name) {
			return t.clone();
		}
		let tag = gtk::TextTag::new(Some(name));
		match name {
			"bold" => tag.set_weight(700),
			"italic" => tag.set_style(pango::Style::Italic),
			"underline" => tag.set_underline(pango::Underline::Single),
			"strike" => tag.set_strikethrough(true),
			"code" => {
				tag.set_family(Some("monospace"));
				tag.set_background_rgba(Some(&self.faint));
			}
			"pre" => {
				tag.set_family(Some("monospace"));
				tag.set_paragraph_background_rgba(Some(&self.faint));
				tag.set_wrap_mode(gtk::WrapMode::None);
				tag.set_left_margin(12);
			}
			"dim" => tag.set_foreground_rgba(Some(&self.dim)),
			"missing" => {
				tag.set_foreground_rgba(Some(&self.dim));
				tag.set_style(pango::Style::Italic);
			}
			"link" => {
				tag.set_foreground_rgba(Some(&self.accent));
				tag.set_underline(pango::Underline::Single);
			}
			"h1" | "h2" | "h3" | "h4" | "h5" | "h6" => {
				let scale = [1.6, 1.4, 1.2, 1.1, 1.0, 1.0][(name.as_bytes()[1] - b'1') as usize];
				tag.set_scale(scale);
				tag.set_weight(800);
				tag.set_pixels_below_lines(2);
			}
			"rule" => {
				tag.set_foreground_rgba(Some(&self.dim));
				tag.set_justification(gtk::Justification::Center);
			}
			_ => {
				if let Some(n) = name.strip_prefix("indent-").and_then(|n| n.parse::<i32>().ok()) {
					tag.set_left_margin(n * INDENT);
				}
			}
		}
		self.buffer.tag_table().add(&tag);
		self.tags.insert(name.to_string(), tag.clone());
		tag
	}

	fn insert(&mut self, text: &str, names: &[String], extra: Option<&gtk::TextTag>) {
		let start = self.buffer.end_iter().offset();
		self.buffer.insert(&mut self.buffer.end_iter(), text);
		let (s, e) = (self.buffer.iter_at_offset(start), self.buffer.end_iter());
		for n in names {
			let tag = self.tag(n);
			self.buffer.apply_tag(&tag, &s, &e);
		}
		if let Some(t) = extra {
			self.buffer.apply_tag(t, &s, &e);
		}
	}

	/// Tags every line of a block gets: its indent, and dimming inside quotes.
	fn base(indent: i32, quote: i32) -> Vec<String> {
		let mut v = Vec::new();
		if indent > 0 {
			v.push(format!("indent-{indent}"));
		}
		if quote > 0 {
			v.push("dim".into());
		}
		v
	}

	fn blocks(&mut self, blocks: &[Block], indent: i32, quote: i32) {
		for (i, block) in blocks.iter().enumerate() {
			let base = Self::base(indent, quote);
			if i > 0 {
				// A blank line between blocks, like a mail's paragraph margins.
				self.insert("\n\n", &base, None);
			}
			match block {
				Block::Para(inl) => self.inlines(inl, &base),
				Block::Heading(level, inl) => {
					let mut names = base.clone();
					names.push(format!("h{level}"));
					self.inlines(inl, &names);
				}
				Block::Pre(text) => {
					let mut names = base.clone();
					names.push("pre".into());
					self.insert(text, &names, None);
				}
				Block::Rule => {
					let mut names = base.clone();
					names.push("rule".into());
					self.insert("⸻", &names, None);
				}
				Block::Quote(inner) => self.blocks(inner, indent + 1, quote + 1),
				Block::List { ordered, start, items } => self.list(*ordered, *start, items, indent, quote),
				Block::Table(rows) => self.table(rows, &base),
			}
		}
	}

	/// Items one per line, the marker hanging left of the text.
	fn list(&mut self, ordered: bool, start: u32, items: &[Vec<Block>], indent: i32, quote: i32) {
		let level = indent + 1;
		let hang = format!("hang-{level}");
		if !self.tags.contains_key(&hang) {
			let tag = gtk::TextTag::new(Some(&hang));
			tag.set_left_margin(level * INDENT);
			tag.set_indent(-INDENT);
			let mut tabs = pango::TabArray::new(1, true);
			tabs.set_tab(0, pango::TabAlign::Left, INDENT);
			tag.set_tabs(Some(&tabs));
			self.buffer.tag_table().add(&tag);
			self.tags.insert(hang.clone(), tag);
		}
		// The hanging tag sets the margin, so no indent tag here.
		let mut names = Self::base(0, quote);
		names.push(hang);
		for (k, item) in items.iter().enumerate() {
			if k > 0 {
				self.insert("\n", &names, None);
			}
			let marker = if ordered { format!("{}.\t", start as usize + k) } else { "•\t".to_string() };
			self.insert(&marker, &names, None);
			let (first, rest) = match item.split_first() {
				Some((Block::Para(inl) | Block::Heading(_, inl), rest)) => (Some(inl), rest),
				_ => (None, &item[..]),
			};
			if let Some(inl) = first {
				self.inlines(inl, &names);
			}
			for block in rest {
				self.insert("\n", &names, None);
				self.blocks(std::slice::from_ref(block), level, quote);
			}
		}
	}

	fn inlines(&mut self, inlines: &[Inline], base: &[String]) {
		for inl in inlines {
			match inl {
				Inline::Text { text, style, link } => {
					let mut names = base.to_vec();
					names.extend(style_names(*style));
					let link_tag = link.as_ref().map(|url| {
						names.push("link".into());
						let tag = gtk::TextTag::new(None);
						self.buffer.tag_table().add(&tag);
						self.out.links.push((tag.clone(), url.clone()));
						tag
					});
					self.insert(text, &names, link_tag.as_ref());
				}
				Inline::Break => self.insert("\n", base, None),
				Inline::Missing(alt) => {
					let mut names = base.to_vec();
					names.push("missing".into());
					self.insert(&format!("[{alt}]"), &names, None);
				}
				Inline::Image { src, alt } => {
					let anchor = self.buffer.create_child_anchor(&mut self.buffer.end_iter());
					let picture = gtk::Picture::builder()
						.content_fit(gtk::ContentFit::Contain)
						.can_shrink(true)
						.alternative_text(alt.as_str())
						.css_classes(["inline-image"])
						.build();
					if !alt.is_empty() {
						picture.set_tooltip_text(Some(alt));
					}
					self.view.add_child_at_anchor(&picture, &anchor);
					self.out.images.push((picture, src.clone()));
				}
			}
		}
	}

	fn table(&mut self, rows: &[Vec<Cell>], base: &[String]) {
		let grid = gtk::Grid::builder().column_spacing(24).row_spacing(8).margin_top(12).margin_bottom(12).margin_start(16).margin_end(16).build();
		let columns = rows.iter().map(Vec::len).max().unwrap_or(1) as i32;
		let header = rows.len() > 1 && rows[0].iter().all(|c| c.header);
		let mut y = 0;
		for (r, row) in rows.iter().enumerate() {
			for (c, cell) in row.iter().enumerate() {
				let label = gtk::Label::builder().xalign(0.0).wrap(true).max_width_chars(40).selectable(true).build();
				label.set_markup(&cell_markup(&cell.content));
				if cell.header {
					label.add_css_class("heading");
				}
				label.connect_activate_link(|label, uri| {
					open_uri(label, uri);
					glib::Propagation::Stop
				});
				grid.attach(&label, c as i32, y, 1, 1);
			}
			y += 1;
			if r == 0 && header {
				grid.attach(&gtk::Separator::new(gtk::Orientation::Horizontal), 0, y, columns, 1);
				y += 1;
			}
		}
		let frame = gtk::Frame::builder().child(&grid).halign(gtk::Align::Start).css_classes(["view"]).build();
		let start = self.buffer.end_iter().offset();
		let anchor = self.buffer.create_child_anchor(&mut self.buffer.end_iter());
		self.view.add_child_at_anchor(&frame, &anchor);
		let (s, e) = (self.buffer.iter_at_offset(start), self.buffer.end_iter());
		for n in base {
			let tag = self.tag(n);
			self.buffer.apply_tag(&tag, &s, &e);
		}
	}
}

fn style_names(s: Style) -> Vec<String> {
	let mut v = Vec::new();
	for (on, name) in [(s.bold, "bold"), (s.italic, "italic"), (s.underline, "underline"), (s.strike, "strike"), (s.code, "code")] {
		if on {
			v.push(name.to_string());
		}
	}
	v
}

fn cell_markup(inlines: &[Inline]) -> String {
	let mut m = String::new();
	for inl in inlines {
		match inl {
			Inline::Text { text, style, link } => {
				let mut t = glib::markup_escape_text(text).to_string();
				if style.code {
					t = format!("<tt>{t}</tt>");
				}
				if style.bold {
					t = format!("<b>{t}</b>");
				}
				if style.italic {
					t = format!("<i>{t}</i>");
				}
				if style.underline {
					t = format!("<u>{t}</u>");
				}
				if style.strike {
					t = format!("<s>{t}</s>");
				}
				if let Some(url) = link {
					t = format!("<a href=\"{}\">{t}</a>", glib::markup_escape_text(url));
				}
				m.push_str(&t);
			}
			Inline::Break => m.push('\n'),
			Inline::Missing(alt) => m.push_str(&format!("<i>[{}]</i>", glib::markup_escape_text(alt))),
			Inline::Image { alt, .. } => m.push_str(&glib::markup_escape_text(alt)),
		}
	}
	m
}

/// Opens a link from the mail in the browser (or mail app, for mailto).
pub fn open_uri(widget: &impl IsA<gtk::Widget>, uri: &str) {
	let window = widget.root().and_downcast::<gtk::Window>();
	gtk::UriLauncher::new(uri).launch(window.as_ref(), gtk::gio::Cancellable::NONE, |_| {});
}

/// Decoded pixels of an image, small enough to show. Runs off the GTK thread.
pub struct Pixels {
	pub width: u32,
	pub height: u32,
	pub rgba: Vec<u8>,
}

pub fn decode(bytes: &[u8]) -> Option<Pixels> {
	let mut reader = image::ImageReader::new(std::io::Cursor::new(bytes)).with_guessed_format().ok()?;
	let mut limits = image::Limits::default();
	limits.max_image_width = Some(8000);
	limits.max_image_height = Some(8000);
	limits.max_alloc = Some(256 * 1024 * 1024);
	reader.limits(limits);
	let img = reader.decode().ok()?;
	// Shrink big photos here, so the GTK thread never handles more pixels than it shows.
	let img = if img.width() > 2 * MAX_IMAGE_WIDTH as u32 || img.height() > 2 * MAX_IMAGE_HEIGHT as u32 {
		img.thumbnail(2 * MAX_IMAGE_WIDTH as u32, 2 * MAX_IMAGE_HEIGHT as u32)
	} else {
		img
	};
	let rgba = img.to_rgba8();
	Some(Pixels { width: rgba.width(), height: rgba.height(), rgba: rgba.into_raw() })
}

/// Shows decoded pixels in a picture placed by [`render`]. Returns its natural size in logical
/// pixels, which [`fit`] shrinks to the reading width.
pub fn show_pixels(picture: &gtk::Picture, px: Pixels, scale: i32) -> Option<(f64, f64)> {
	if px.width <= 1 && px.height <= 1 {
		picture.set_visible(false);
		return None;
	}
	let texture = gdk::MemoryTexture::new(
		px.width as i32,
		px.height as i32,
		gdk::MemoryFormat::R8g8b8a8,
		&glib::Bytes::from_owned(px.rgba),
		px.width as usize * 4,
	);
	picture.set_paintable(Some(&texture));
	let scale = scale.max(1) as f64;
	Some((px.width as f64 / scale, px.height as f64 / scale))
}

/// Sizes a picture to its natural size, shrunk to `width` and the height cap.
pub fn fit(picture: &gtk::Picture, natural: (f64, f64), width: i32) {
	let (w, h) = natural;
	let max_w = (width.min(MAX_IMAGE_WIDTH) as f64).max(48.0);
	let k = (max_w / w).min(MAX_IMAGE_HEIGHT as f64 / h).min(1.0);
	picture.set_size_request((w * k).round().max(1.0) as i32, (h * k).round().max(1.0) as i32);
}

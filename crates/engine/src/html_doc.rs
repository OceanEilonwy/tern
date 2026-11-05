//! Turns a mail's HTML into a small document Tern can draw without a browser.
//!
//! Only structure survives: paragraphs, headings, emphasis, links, lists, quotes, data tables and
//! images the mail carries itself (`cid:` and `data:`). CSS, scripts, colours and fonts are dropped,
//! and remote images are never fetched.

use base64::Engine;
use ego_tree::NodeRef;
use scraper::{Html, Node};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Style {
	pub bold: bool,
	pub italic: bool,
	pub underline: bool,
	pub strike: bool,
	pub code: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ImageSrc {
	/// An attachment of the mail, by Content-ID (without the `cid:` and angle brackets).
	Cid(String),
	/// Bytes carried in a `data:` URI.
	Data(Vec<u8>),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Inline {
	Text { text: String, style: Style, link: Option<String> },
	Break,
	Image { src: ImageSrc, alt: String },
	/// A remote image Tern will not load, by its alt text.
	Missing(String),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Cell {
	pub header: bool,
	pub content: Vec<Inline>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Block {
	Para(Vec<Inline>),
	Heading(u8, Vec<Inline>),
	Pre(String),
	List { ordered: bool, start: u32, items: Vec<Vec<Block>> },
	Quote(Vec<Block>),
	Rule,
	Table(Vec<Vec<Cell>>),
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Doc {
	pub blocks: Vec<Block>,
	/// Remote images left out, not counting tracking pixels.
	pub remote_images: usize,
	/// Tables used for layout, drawn as plain blocks.
	pub layout_tables: usize,
	/// Things with no plain equivalent: frames, video, forms, SVG.
	pub dropped: usize,
}

impl Doc {
	pub fn parse(html: &str) -> Doc {
		if !html.contains('<') {
			return Doc::plain(html);
		}
		let parsed = Html::parse_document(html);
		let mut b = Builder::default();
		b.walk_children(parsed.tree.root());
		b.flush();
		let root = b.containers.pop().map(|c| c.blocks).unwrap_or_default();
		Doc { blocks: root, remote_images: b.remote_images, layout_tables: b.layout_tables, dropped: b.dropped }
	}

	/// A body that is plain text: blank lines split paragraphs, `>` lines become quotes.
	pub fn plain(text: &str) -> Doc {
		Doc { blocks: plain_blocks(text), ..Default::default() }
	}

	/// True when the mail leans on things Tern does not draw, so Tuta's web app would show it better.
	pub fn is_complex(&self) -> bool {
		self.remote_images > 0 || self.layout_tables > 0 || self.dropped > 0
	}

	/// Content-IDs of the images drawn inline.
	pub fn cids(&self) -> Vec<String> {
		let mut out = Vec::new();
		walk_inlines(&self.blocks, &mut |i| {
			if let Inline::Image { src: ImageSrc::Cid(cid), .. } = i {
				out.push(cid.clone());
			}
		});
		out
	}

	/// The text a reader sees, for quoting in replies.
	pub fn to_plain_text(&self) -> String {
		let mut out = String::new();
		blocks_text(&self.blocks, &mut out);
		out.trim_end().to_string()
	}
}

fn walk_inlines(blocks: &[Block], f: &mut impl FnMut(&Inline)) {
	for b in blocks {
		match b {
			Block::Para(i) | Block::Heading(_, i) => i.iter().for_each(&mut *f),
			Block::List { items, .. } => items.iter().for_each(|item| walk_inlines(item, f)),
			Block::Quote(inner) => walk_inlines(inner, f),
			Block::Table(rows) => rows.iter().flatten().for_each(|c| c.content.iter().for_each(&mut *f)),
			Block::Pre(_) | Block::Rule => {}
		}
	}
}

fn inlines_text(inlines: &[Inline]) -> String {
	let mut s = String::new();
	for i in inlines {
		match i {
			Inline::Text { text, link, .. } => {
				s.push_str(text);
				if let Some(url) = link {
					let bare = url.strip_prefix("mailto:").unwrap_or(url);
					if bare != text.trim() {
						s.push_str(&format!(" <{bare}>"));
					}
				}
			}
			Inline::Break => s.push('\n'),
			Inline::Image { alt, .. } | Inline::Missing(alt) => {
				if !alt.is_empty() {
					s.push_str(&format!("[{alt}]"));
				}
			}
		}
	}
	s
}

fn blocks_text(blocks: &[Block], out: &mut String) {
	for (n, b) in blocks.iter().enumerate() {
		if n > 0 {
			out.push('\n');
		}
		match b {
			Block::Para(i) | Block::Heading(_, i) => out.push_str(&inlines_text(i)),
			Block::Pre(t) => out.push_str(t.trim_end()),
			Block::Rule => out.push_str("----"),
			Block::Quote(inner) => {
				let mut q = String::new();
				blocks_text(inner, &mut q);
				for line in q.trim_end().lines() {
					out.push_str(if line.is_empty() { ">" } else { "> " });
					out.push_str(line);
					out.push('\n');
				}
				out.pop();
			}
			Block::List { ordered, start, items } => {
				for (k, item) in items.iter().enumerate() {
					let marker = if *ordered { format!("{}. ", *start as usize + k) } else { "• ".into() };
					let mut t = String::new();
					blocks_text(item, &mut t);
					for (l, line) in t.trim_end().lines().filter(|l| !l.is_empty()).enumerate() {
						if l == 0 {
							out.push_str(&marker);
						} else {
							out.push_str(&" ".repeat(marker.chars().count()));
						}
						out.push_str(line);
						out.push('\n');
					}
				}
				out.pop();
			}
			Block::Table(rows) => {
				for row in rows {
					let cells: Vec<String> = row.iter().map(|c| inlines_text(&c.content).replace('\n', " ")).collect();
					out.push_str(cells.join(" | ").trim_end());
					out.push('\n');
				}
				out.pop();
			}
		}
		out.push('\n');
	}
}

fn plain_blocks(text: &str) -> Vec<Block> {
	let text = text.replace("\r\n", "\n");
	let mut blocks = Vec::new();
	let mut lines = text.lines().peekable();
	while lines.peek().is_some() {
		if lines.peek().is_some_and(|l| l.trim().is_empty()) {
			lines.next();
			continue;
		}
		if lines.peek().is_some_and(|l| l.starts_with('>')) {
			let mut quoted = String::new();
			while let Some(l) = lines.next_if(|l| l.starts_with('>')) {
				let l = l.strip_prefix('>').unwrap();
				quoted.push_str(l.strip_prefix(' ').unwrap_or(l));
				quoted.push('\n');
			}
			blocks.push(Block::Quote(plain_blocks(&quoted)));
			continue;
		}
		let mut para = Vec::new();
		while let Some(l) = lines.next_if(|l| !l.trim().is_empty() && !l.starts_with('>')) {
			if !para.is_empty() {
				para.push(Inline::Break);
			}
			linkify(l.trim_end(), Style::default(), &mut para);
		}
		blocks.push(Block::Para(para));
	}
	blocks
}

/// Appends `text`, turning bare http(s) URLs into links.
fn linkify(text: &str, style: Style, out: &mut Vec<Inline>) {
	let mut rest = text;
	while let Some(start) = find_url(rest) {
		let end = rest[start..].find(char::is_whitespace).map_or(rest.len(), |e| start + e);
		let mut url = &rest[start..end];
		url = url.trim_end_matches(['.', ',', ';', ':', '!', '?', ')', '\'', '"', '>']);
		let end = start + url.len();
		push_text(out, &rest[..start], style, None);
		push_text(out, url, style, Some(url.to_string()));
		rest = &rest[end..];
	}
	push_text(out, rest, style, None);
}

fn find_url(s: &str) -> Option<usize> {
	let i = [s.find("https://"), s.find("http://")].into_iter().flatten().min()?;
	// Only at a word start, so "xhttp://" is left alone.
	if i == 0 || !s[..i].ends_with(|c: char| c.is_alphanumeric()) {
		Some(i)
	} else {
		None
	}
}

fn push_text(out: &mut Vec<Inline>, text: &str, style: Style, link: Option<String>) {
	if text.is_empty() {
		return;
	}
	if let Some(Inline::Text { text: last, style: s, link: l }) = out.last_mut() {
		if *s == style && *l == link {
			last.push_str(text);
			return;
		}
	}
	out.push(Inline::Text { text: text.to_string(), style, link });
}

enum Kind {
	Root,
	Quote,
	List { ordered: bool, start: u32, items: Vec<Vec<Block>> },
	Item,
}

struct Container {
	kind: Kind,
	blocks: Vec<Block>,
}

struct Builder {
	containers: Vec<Container>,
	para: Vec<Inline>,
	style: Style,
	link: Option<String>,
	remote_images: usize,
	layout_tables: usize,
	dropped: usize,
}

impl Default for Builder {
	fn default() -> Self {
		Self {
			containers: vec![Container { kind: Kind::Root, blocks: Vec::new() }],
			para: Vec::new(),
			style: Style::default(),
			link: None,
			remote_images: 0,
			layout_tables: 0,
			dropped: 0,
		}
	}
}

type NodeR<'a> = NodeRef<'a, Node>;

const SKIP: &[&str] = &["head", "script", "style", "title", "template", "noscript", "meta", "link", "base"];
const DROP: &[&str] =
	&["iframe", "frame", "object", "embed", "video", "audio", "canvas", "svg", "math", "input", "button", "select", "textarea"];
const BLOCKS: &[&str] = &[
	"p", "div", "section", "article", "header", "footer", "main", "nav", "aside", "center", "address", "figure", "figcaption", "dl",
	"dt", "dd", "form", "fieldset", "details", "summary", "body", "html", "tr", "td", "th", "thead", "tbody", "tfoot", "caption",
	"table",
];

impl Builder {
	fn blocks(&mut self) -> &mut Vec<Block> {
		&mut self.containers.last_mut().unwrap().blocks
	}

	/// Ends the paragraph being collected.
	fn flush(&mut self) {
		let mut para = std::mem::take(&mut self.para);
		trim_inlines(&mut para);
		if !para.is_empty() {
			self.blocks().push(Block::Para(para));
		}
	}

	fn walk_children(&mut self, node: NodeR) {
		for child in node.children() {
			self.walk(child);
		}
	}

	fn text(&mut self, raw: &str) {
		let mut collapsed = String::with_capacity(raw.len());
		let mut space = self.para_ends_in_space();
		for c in raw.chars() {
			if c.is_whitespace() {
				if !space {
					collapsed.push(' ');
					space = true;
				}
			} else {
				collapsed.push(c);
				space = false;
			}
		}
		if self.link.is_some() {
			push_text(&mut self.para, &collapsed, self.style, self.link.clone());
		} else {
			linkify(&collapsed, self.style, &mut self.para);
		}
	}

	fn para_ends_in_space(&self) -> bool {
		match self.para.last() {
			None | Some(Inline::Break) => true,
			Some(Inline::Text { text, .. }) => text.ends_with(' '),
			Some(_) => false,
		}
	}

	fn walk(&mut self, node: NodeR) {
		let el = match node.value() {
			Node::Text(t) => return self.text(t),
			Node::Element(el) => el,
			Node::Document | Node::Fragment => return self.walk_children(node),
			_ => return,
		};
		let name = el.name();
		if SKIP.contains(&name) {
			return;
		}
		if DROP.contains(&name) {
			self.dropped += 1;
			return;
		}
		match name {
			"br" => self.para.push(Inline::Break),
			"hr" => {
				self.flush();
				self.blocks().push(Block::Rule);
			}
			"h1" | "h2" | "h3" | "h4" | "h5" | "h6" => {
				self.flush();
				let saved = self.style;
				self.walk_children(node);
				self.style = saved;
				let mut inlines = std::mem::take(&mut self.para);
				trim_inlines(&mut inlines);
				if !inlines.is_empty() {
					self.blocks().push(Block::Heading(name.as_bytes()[1] - b'0', inlines));
				}
			}
			"pre" | "xmp" | "plaintext" | "listing" => {
				self.flush();
				let text = text_content(node);
				let text = text.strip_prefix('\n').unwrap_or(&text).trim_end().to_string();
				if !text.is_empty() {
					self.blocks().push(Block::Pre(text));
				}
			}
			"blockquote" => {
				self.flush();
				self.containers.push(Container { kind: Kind::Quote, blocks: Vec::new() });
				self.walk_children(node);
				self.flush();
				let c = self.containers.pop().unwrap();
				if !c.blocks.is_empty() {
					self.blocks().push(Block::Quote(c.blocks));
				}
			}
			"ul" | "ol" | "menu" => {
				self.flush();
				let start = el.attr("start").and_then(|s| s.trim().parse().ok()).unwrap_or(1);
				self.containers.push(Container { kind: Kind::List { ordered: name == "ol", start, items: Vec::new() }, blocks: Vec::new() });
				self.walk_children(node);
				self.flush();
				let c = self.containers.pop().unwrap();
				if let Kind::List { ordered, start, mut items } = c.kind {
					// Stray content right inside the list becomes an item of its own.
					if !c.blocks.is_empty() {
						items.push(c.blocks);
					}
					if !items.is_empty() {
						self.blocks().push(Block::List { ordered, start, items });
					}
				}
			}
			"li" => {
				self.flush();
				self.containers.push(Container { kind: Kind::Item, blocks: Vec::new() });
				self.walk_children(node);
				self.flush();
				let item = self.containers.pop().unwrap().blocks;
				match &mut self.containers.last_mut().unwrap().kind {
					Kind::List { items, .. } => {
						if !item.is_empty() {
							items.push(item);
						}
					}
					// An item outside a list is just a paragraph.
					_ => self.blocks().extend(item),
				}
			}
			"table" => {
				self.flush();
				if is_data_table(node) {
					let rows = table_rows(node);
					if !rows.is_empty() {
						self.blocks().push(Block::Table(rows));
					}
				} else {
					self.layout_tables += 1;
					self.walk_children(node);
					self.flush();
				}
			}
			"img" => self.image(el),
			"a" => {
				let saved = self.link.clone();
				if let Some(href) = el.attr("href").map(str::trim).filter(|h| is_safe_link(h)) {
					self.link = Some(href.to_string());
				}
				self.walk_children(node);
				self.link = saved;
			}
			_ if BLOCKS.contains(&name) => {
				self.flush();
				self.walk_children(node);
				self.flush();
			}
			_ => {
				let saved = self.style;
				match name {
					"b" | "strong" => self.style.bold = true,
					"i" | "em" | "cite" | "var" | "dfn" => self.style.italic = true,
					"u" | "ins" => self.style.underline = true,
					"s" | "strike" | "del" => self.style.strike = true,
					"code" | "kbd" | "samp" | "tt" => self.style.code = true,
					_ => {}
				}
				self.walk_children(node);
				self.style = saved;
			}
		}
	}

	fn image(&mut self, el: &scraper::node::Element) {
		let src = el.attr("src").unwrap_or("").trim();
		let alt = el.attr("alt").unwrap_or("").trim().to_string();
		if let Some(cid) = src.get(..4).filter(|p| p.eq_ignore_ascii_case("cid:")).map(|_| &src[4..]) {
			let cid = cid.trim_start_matches('<').trim_end_matches('>').to_string();
			self.para.push(Inline::Image { src: ImageSrc::Cid(cid), alt });
		} else if let Some(data) = decode_data_uri(src) {
			self.para.push(Inline::Image { src: ImageSrc::Data(data), alt });
		} else {
			let tiny = |a: &str| el.attr(a).and_then(|v| v.trim().trim_end_matches("px").parse::<u32>().ok()).is_some_and(|v| v <= 2);
			// Tracking pixels are neither shown nor counted.
			if !(tiny("width") || tiny("height")) && !src.is_empty() {
				self.remote_images += 1;
				if !alt.is_empty() {
					self.para.push(Inline::Missing(alt));
				}
			}
		}
	}
}

fn is_safe_link(href: &str) -> bool {
	let lower = href.to_ascii_lowercase();
	lower.starts_with("https://") || lower.starts_with("http://") || lower.starts_with("mailto:")
}

/// Bytes of a base64 `data:image/...` URI.
fn decode_data_uri(src: &str) -> Option<Vec<u8>> {
	let rest = src.get(..5).filter(|p| p.eq_ignore_ascii_case("data:")).map(|_| &src[5..])?;
	let (meta, payload) = rest.split_once(',')?;
	if !meta.to_ascii_lowercase().starts_with("image/") || !meta.to_ascii_lowercase().ends_with(";base64") {
		return None;
	}
	let clean: String = payload.chars().filter(|c| !c.is_whitespace()).collect();
	base64::engine::general_purpose::STANDARD.decode(clean).ok()
}

/// Drops whitespace at the ends of a paragraph, and paragraphs of only breaks.
fn trim_inlines(para: &mut Vec<Inline>) {
	while matches!(para.last(), Some(Inline::Break)) {
		para.pop();
	}
	while matches!(para.first(), Some(Inline::Break)) {
		para.remove(0);
	}
	if let Some(Inline::Text { text, .. }) = para.last_mut() {
		let len = text.trim_end().len();
		text.truncate(len);
	}
	if let Some(Inline::Text { text, .. }) = para.first_mut() {
		*text = text.trim_start().to_string();
	}
	para.retain(|i| !matches!(i, Inline::Text { text, .. } if text.is_empty()));
	let has_content = para.iter().any(|i| match i {
		Inline::Text { text, .. } => !text.trim().is_empty(),
		Inline::Image { .. } | Inline::Missing(_) => true,
		Inline::Break => false,
	});
	if !has_content {
		para.clear();
	}
	// Trailing breaks may surface once empty text is gone.
	while matches!(para.last(), Some(Inline::Break)) {
		para.pop();
	}
}

fn text_content(node: NodeR) -> String {
	let mut s = String::new();
	for d in node.descendants() {
		match d.value() {
			Node::Text(t) => s.push_str(t),
			Node::Element(e) if e.name() == "br" => s.push('\n'),
			_ => {}
		}
	}
	s
}

fn element_name<'a>(node: &NodeR<'a>) -> Option<&'a str> {
	node.value().as_element().map(|e| e.name())
}

/// Rows of a table, looking through thead/tbody/tfoot but not into nested tables.
fn rows_of(table: NodeR) -> Vec<NodeR> {
	let mut rows = Vec::new();
	for child in table.children() {
		match element_name(&child) {
			Some("tr") => rows.push(child),
			Some("thead" | "tbody" | "tfoot") => rows.extend(child.children().filter(|c| element_name(c) == Some("tr"))),
			_ => {}
		}
	}
	rows
}

fn cells_of(row: NodeR) -> impl Iterator<Item = NodeR> {
	row.children().filter(|c| matches!(element_name(c), Some("td" | "th")))
}

/// A table of data (a grid worth drawing as one) rather than one used to lay out a newsletter.
fn is_data_table(table: NodeR) -> bool {
	let el = table.value().as_element().unwrap();
	if matches!(el.attr("role"), Some("presentation" | "none")) {
		return false;
	}
	let nested = table.descendants().skip(1).any(|d| matches!(element_name(&d), Some("table" | "img" | "div" | "p" | "ul" | "ol")));
	if nested {
		return false;
	}
	let rows = rows_of(table);
	let max_cols = rows.iter().map(|r| cells_of(*r).count()).max().unwrap_or(0);
	if rows.len() < 2 || max_cols < 2 {
		return false;
	}
	let bordered = el.attr("border").and_then(|b| b.trim().parse::<u32>().ok()).is_some_and(|b| b > 0);
	if bordered {
		return true;
	}
	if el.attr("width").is_some() || el.attr("align").is_some() || el.attr("bgcolor").is_some() {
		return false;
	}
	rows.iter().flat_map(|r| cells_of(*r)).all(|c| text_content(c).trim().chars().count() <= 60)
}

fn table_rows(table: NodeR) -> Vec<Vec<Cell>> {
	let mut out = Vec::new();
	for row in rows_of(table) {
		let mut cells = Vec::new();
		for cell in cells_of(row) {
			let mut b = Builder::default();
			b.walk_children(cell);
			b.flush();
			// A cell is one run of text; its paragraphs become line breaks.
			let mut content = Vec::new();
			for block in b.containers.pop().unwrap().blocks {
				if let Block::Para(i) | Block::Heading(_, i) = block {
					if !content.is_empty() {
						content.push(Inline::Break);
					}
					content.extend(i);
				}
			}
			cells.push(Cell { header: element_name(&cell) == Some("th"), content });
		}
		if cells.iter().any(|c| !c.content.is_empty()) {
			out.push(cells);
		}
	}
	out
}

#[cfg(test)]
mod tests {
	use super::*;

	fn text(t: &str) -> Inline {
		Inline::Text { text: t.into(), style: Style::default(), link: None }
	}

	fn styled(t: &str, style: Style) -> Inline {
		Inline::Text { text: t.into(), style, link: None }
	}

	#[test]
	fn paragraphs_breaks_and_emphasis() {
		let doc = Doc::parse("<p>Hi <b>there</b>,</p><p>line one<br>line\n  two</p>");
		let bold = Style { bold: true, ..Default::default() };
		assert_eq!(
			doc.blocks,
			vec![
				Block::Para(vec![text("Hi "), styled("there", bold), text(",")]),
				Block::Para(vec![text("line one"), Inline::Break, text("line two")]),
			]
		);
		assert!(!doc.is_complex());
	}

	#[test]
	fn scripts_styles_and_head_are_stripped() {
		let doc = Doc::parse(
			"<html><head><title>T</title><style>p{color:red}</style></head><body><script>alert(1)</script><p>Body</p><noscript>x</noscript></body></html>",
		);
		assert_eq!(doc.blocks, vec![Block::Para(vec![text("Body")])]);
		assert_eq!(doc.to_plain_text(), "Body");
	}

	#[test]
	fn links_keep_their_target_and_unsafe_ones_are_dropped() {
		let doc = Doc::parse(r#"<p><a href="https://x.org/a">site</a> <a href="javascript:alert(1)">bad</a> see https://y.org/b.</p>"#);
		let Block::Para(inl) = &doc.blocks[0] else { panic!() };
		assert_eq!(inl[0], Inline::Text { text: "site".into(), style: Style::default(), link: Some("https://x.org/a".into()) });
		assert_eq!(inl[1], text(" bad see "));
		assert_eq!(inl[2], Inline::Text { text: "https://y.org/b".into(), style: Style::default(), link: Some("https://y.org/b".into()) });
		assert_eq!(inl[3], text("."));
	}

	#[test]
	fn lists_nest_and_number() {
		let doc = Doc::parse("<ol start=3><li>three</li><li>four<ul><li>inner</li></ul></li></ol>");
		assert_eq!(
			doc.blocks,
			vec![Block::List {
				ordered: true,
				start: 3,
				items: vec![
					vec![Block::Para(vec![text("three")])],
					vec![
						Block::Para(vec![text("four")]),
						Block::List { ordered: false, start: 1, items: vec![vec![Block::Para(vec![text("inner")])]] }
					],
				],
			}]
		);
		assert_eq!(doc.to_plain_text(), "3. three\n4. four\n   • inner");
	}

	#[test]
	fn quotes_hold_blocks() {
		let doc = Doc::parse("<p>Sure.</p><blockquote><p>Lunch?</p><blockquote>Older</blockquote></blockquote>");
		assert_eq!(
			doc.blocks[1],
			Block::Quote(vec![Block::Para(vec![text("Lunch?")]), Block::Quote(vec![Block::Para(vec![text("Older")])])])
		);
		assert_eq!(doc.to_plain_text(), "Sure.\n\n> Lunch?\n>\n> > Older");
	}

	#[test]
	fn bordered_table_is_data() {
		let doc = Doc::parse(
			"<table border=1><tr><th>Item</th><th>Price</th></tr><tr><td>Hosting</td><td>€12</td></tr><tr><td><b>Total</b></td><td></td></tr></table>",
		);
		let Block::Table(rows) = &doc.blocks[0] else { panic!("{:?}", doc.blocks) };
		assert_eq!(rows.len(), 3);
		assert!(rows[0][0].header);
		assert_eq!(rows[1][1].content, vec![text("€12")]);
		assert!(rows[2][1].content.is_empty());
		assert!(!doc.is_complex());
		assert_eq!(doc.to_plain_text(), "Item | Price\nHosting | €12\nTotal |");
	}

	#[test]
	fn short_grid_without_border_is_data() {
		let doc = Doc::parse("<table><tr><td>a</td><td>1</td></tr><tr><td>b</td><td>2</td></tr></table>");
		assert!(matches!(doc.blocks[0], Block::Table(_)));
	}

	#[test]
	fn layout_tables_are_flattened() {
		let single = Doc::parse("<table><tr><td><p>Hello</p></td></tr><tr><td>World</td></tr></table>");
		assert_eq!(single.blocks, vec![Block::Para(vec![text("Hello")]), Block::Para(vec![text("World")])]);
		assert_eq!(single.layout_tables, 1);
		assert!(single.is_complex());

		let nested = Doc::parse("<table><tr><td><table><tr><td>a</td><td>b</td></tr><tr><td>c</td><td>d</td></tr></table></td><td>x</td></tr></table>");
		assert_eq!(nested.layout_tables, 1, "outer is layout, inner is data: {:?}", nested.blocks);
		assert!(matches!(nested.blocks[0], Block::Table(_)));

		let presentation = Doc::parse("<table role=presentation border=1><tr><td>a</td><td>b</td></tr><tr><td>c</td><td>d</td></tr></table>");
		assert_eq!(presentation.layout_tables, 1);
		let wide = Doc::parse("<table width=600><tr><td>a</td><td>b</td></tr><tr><td>c</td><td>d</td></tr></table>");
		assert_eq!(wide.layout_tables, 1);
	}

	#[test]
	fn cid_and_data_images_are_kept() {
		let doc = Doc::parse(r#"<p><img src="cid:&lt;sunset&gt;" alt="Sunset"><img src="data:image/png;base64,iVBO Rw=="></p>"#);
		let Block::Para(inl) = &doc.blocks[0] else { panic!() };
		assert_eq!(inl[0], Inline::Image { src: ImageSrc::Cid("sunset".into()), alt: "Sunset".into() });
		assert_eq!(inl[1], Inline::Image { src: ImageSrc::Data(vec![0x89, 0x50, 0x4e, 0x47]), alt: String::new() });
		assert_eq!(doc.cids(), vec!["sunset".to_string()]);
		assert!(!doc.is_complex());
	}

	#[test]
	fn remote_images_are_never_kept() {
		let doc = Doc::parse(
			r#"<p>Hi<img src="https://t.example/pixel.gif" width="1" height="1"></p><p><img src="https://cdn.example/logo.png" alt="Logo"><img src="http://x/y.png"></p>"#,
		);
		assert_eq!(doc.blocks, vec![Block::Para(vec![text("Hi")]), Block::Para(vec![Inline::Missing("Logo".into())])]);
		assert_eq!(doc.remote_images, 2, "tracking pixel is not counted");
		assert!(doc.is_complex());
		assert!(!format!("{doc:?}").contains("https://"));
	}

	#[test]
	fn pre_keeps_whitespace_and_headings_have_levels() {
		let doc = Doc::parse("<h2>Invoice</h2><pre>\n  a  b\n c</pre><hr>");
		assert_eq!(doc.blocks, vec![Block::Heading(2, vec![text("Invoice")]), Block::Pre("  a  b\n c".into()), Block::Rule]);
	}

	#[test]
	fn frames_and_forms_are_dropped_and_counted() {
		let doc = Doc::parse("<p>a</p><iframe src=x></iframe><form><input></form>");
		assert_eq!(doc.dropped, 2);
		assert!(doc.is_complex());
	}

	#[test]
	fn plain_text_bodies_keep_lines_and_quotes() {
		let doc = Doc::parse("Hello,\nsee https://x.org.\n\n> quoted\n> more\nBye");
		assert_eq!(doc.blocks.len(), 3);
		assert!(matches!(&doc.blocks[1], Block::Quote(q) if q.len() == 1));
		assert_eq!(doc.to_plain_text(), "Hello,\nsee https://x.org.\n\n> quoted\n> more\n\nBye");
	}

	#[test]
	fn inline_blocks_inside_links_do_not_lose_text() {
		let doc = Doc::parse(r#"<a href="https://x.org"><div>Read more</div></a>"#);
		assert_eq!(doc.blocks, vec![Block::Para(vec![Inline::Text { text: "Read more".into(), style: Style::default(), link: Some("https://x.org".into()) }])]);
	}

	#[test]
	fn demo_mails_parse() {
		let doc = Doc::parse("<h2>Invoice 2026-10</h2><table border=\"1\"><tr><th>Item</th><th>Qty</th><th>Price</th></tr><tr><td>Hosting</td><td>1</td><td>€12.00</td></tr></table><p>The PDF is attached.</p>");
		assert!(matches!(doc.blocks[..], [Block::Heading(2, _), Block::Table(_), Block::Para(_)]));
	}
}

//! A mail's document as the screen draws it: flat blocks, each saying how deep it sits.

use tern_engine::html_doc::{Block, Cell, Doc, ImageSrc, Inline, Style};

use crate::view::{BlockKind, BlockView, CellView, SpanView, TableRowView};

/// A picture of a mail, waiting to be put on disk for drawing.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Picture {
	Cid(String),
	Data(Vec<u8>),
}

/// The blocks of `doc`, and for each picture among them its block's index and where it comes from.
pub fn flatten(doc: &Doc) -> (Vec<BlockView>, Vec<(u32, Picture)>) {
	let mut out = Flat::default();
	out.blocks(&doc.blocks, 0, 0);
	(out.views, out.pictures)
}

#[derive(Default)]
struct Flat {
	views: Vec<BlockView>,
	pictures: Vec<(u32, Picture)>,
	marker: Option<String>,
}

fn span(text: &str, style: &Style, link: Option<&String>) -> SpanView {
	SpanView { text: text.to_string(), bold: style.bold, italic: style.italic, underline: style.underline, strike: style.strike, code: style.code, link: link.cloned() }
}

fn plain(text: &str) -> SpanView {
	span(text, &Style::default(), None)
}

impl Flat {
	fn push(&mut self, kind: BlockKind, quote_depth: u32, list_depth: u32) -> u32 {
		let index = self.views.len() as u32;
		self.views.push(BlockView { index, kind, quote_depth, list_depth, marker: self.marker.take(), picked: false });
		index
	}

	/// Inlines as text blocks, with each picture a block of its own between them.
	fn inlines(&mut self, inlines: &[Inline], heading: Option<u8>, quote: u32, list: u32) {
		let mut spans: Vec<SpanView> = Vec::new();
		let flush = |this: &mut Flat, spans: &mut Vec<SpanView>| {
			if spans.iter().any(|s| !s.text.trim().is_empty()) {
				let kind = match heading {
					Some(level) => BlockKind::Heading { level, spans: std::mem::take(spans) },
					None => BlockKind::Text { spans: std::mem::take(spans) },
				};
				this.push(kind, quote, list);
			}
			spans.clear();
		};
		for inline in inlines {
			match inline {
				Inline::Text { text, style, link } => spans.push(span(text, style, link.as_ref())),
				Inline::Break => spans.push(plain("\n")),
				Inline::Missing(alt) if !alt.is_empty() => spans.push(SpanView { italic: true, ..plain(&format!("[{alt}]")) }),
				Inline::Missing(_) => {},
				Inline::Image { src, alt } => {
					flush(self, &mut spans);
					let index = self.push(BlockKind::Image { path: None, alt: alt.clone() }, quote, list);
					self.pictures.push((
						index,
						match src {
							ImageSrc::Cid(cid) => Picture::Cid(cid.clone()),
							ImageSrc::Data(data) => Picture::Data(data.clone()),
						},
					));
				},
			}
		}
		flush(self, &mut spans);
	}

	fn cell(cell: &Cell) -> CellView {
		let mut spans = Vec::new();
		for inline in &cell.content {
			match inline {
				Inline::Text { text, style, link } => spans.push(span(text, style, link.as_ref())),
				Inline::Break => spans.push(plain("\n")),
				Inline::Image { alt, .. } | Inline::Missing(alt) if !alt.is_empty() => spans.push(plain(&format!("[{alt}]"))),
				_ => {},
			}
		}
		CellView { header: cell.header, spans }
	}

	fn blocks(&mut self, blocks: &[Block], quote: u32, list: u32) {
		for block in blocks {
			match block {
				Block::Para(inlines) => self.inlines(inlines, None, quote, list),
				Block::Heading(level, inlines) => self.inlines(inlines, Some(*level), quote, list),
				Block::Pre(text) => {
					self.push(BlockKind::Pre { text: text.clone() }, quote, list);
				},
				Block::Rule => {
					self.push(BlockKind::Rule, quote, list);
				},
				Block::Quote(inner) => self.blocks(inner, quote + 1, list),
				Block::List { ordered, start, items } => {
					for (i, item) in items.iter().enumerate() {
						self.marker = Some(if *ordered { format!("{}.", *start as usize + i) } else { "•".to_string() });
						self.blocks(item, quote, list + 1);
						self.marker = None;
					}
				},
				Block::Table(rows) => {
					let rows = rows.iter().map(|row| TableRowView { cells: row.iter().map(Flat::cell).collect() }).collect();
					self.push(BlockKind::Table { rows }, quote, list);
				},
			}
		}
	}
}

/// The words of a block, for quoting it.
pub fn block_text(block: &BlockView) -> String {
	let spans = |spans: &[SpanView]| spans.iter().map(|s| s.text.as_str()).collect::<String>();
	match &block.kind {
		BlockKind::Text { spans: s } | BlockKind::Heading { spans: s, .. } => spans(s),
		BlockKind::Pre { text } => text.clone(),
		BlockKind::Image { alt, .. } => alt.clone(),
		BlockKind::Rule => String::new(),
		BlockKind::Table { rows } => rows.iter().map(|r| r.cells.iter().map(|c| spans(&c.spans)).collect::<Vec<_>>().join("\t")).collect::<Vec<_>>().join("\n"),
	}
}

#[cfg(test)]
mod tests {
	use super::*;

	#[test]
	fn quotes_and_lists_flatten_with_their_depth() {
		let doc = Doc::parse("<p>Hi <b>there</b></p><blockquote><p>Said</p><blockquote><p>Before</p></blockquote></blockquote><ul><li>One</li><li>Two</li></ul><ol start=3><li>Three</li></ol>");
		let (blocks, pictures) = flatten(&doc);
		assert!(pictures.is_empty());
		let texts: Vec<(String, u32, u32, Option<String>)> = blocks.iter().map(|b| (block_text(b).trim().to_string(), b.quote_depth, b.list_depth, b.marker.clone())).collect();
		assert_eq!(texts[0], ("Hi there".to_string(), 0, 0, None));
		assert_eq!(texts[1], ("Said".to_string(), 1, 0, None));
		assert_eq!(texts[2], ("Before".to_string(), 2, 0, None));
		assert_eq!(texts[3], ("One".to_string(), 0, 1, Some("•".into())));
		assert_eq!(texts[5], ("Three".to_string(), 0, 1, Some("3.".into())));
		let BlockKind::Text { spans } = &blocks[0].kind else { panic!("not text") };
		assert!(spans.iter().any(|s| s.bold && s.text == "there"));
		assert_eq!(blocks.iter().map(|b| b.index).collect::<Vec<_>>(), (0..blocks.len() as u32).collect::<Vec<_>>());
	}

	#[test]
	fn pictures_stand_apart_and_remote_ones_never_load() {
		let doc = Doc::parse("<p>Before<img src=\"cid:sun\" alt=\"The sun\">After</p><p><img src=\"https://x.org/a.png\" alt=\"Far\"></p>");
		let (blocks, pictures) = flatten(&doc);
		assert_eq!(pictures, vec![(1, Picture::Cid("sun".into()))]);
		assert!(matches!(&blocks[1].kind, BlockKind::Image { path: None, alt } if alt == "The sun"));
		assert_eq!(block_text(&blocks[0]), "Before");
		assert_eq!(block_text(&blocks[2]), "After");
		assert!(block_text(blocks.last().unwrap()).contains("[Far]"), "a remote image is only its alt text");
	}
}

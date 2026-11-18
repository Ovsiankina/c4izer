//! Markdown to drawable blocks with `pulldown-cmark` (P3-514, P3-517):
//! headings, paragraphs, emphasis, lists, code blocks, tables, links,
//! images and `embed:` view references.
//!
//! Depends on: `pulldown-cmark`.

// DECISION: CommonMark plus tables and strikethrough (pulldown-cmark's
// `ENABLE_TABLES`, `ENABLE_STRIKETHROUGH`), the GitHub flavour Structurizr's
// markdown-it renders; strikethrough text is kept as plain text. Raw HTML
// is shown as its source text, since the viewer draws no HTML.
// DECISION: an image ends the paragraph it is in and becomes its own
// block, so images and embedded views always get the full column width;
// an image inside a table cell shows as its alt text.

use pulldown_cmark::{Event, Options, Parser, Tag, TagEnd};

/// A run of inline text with its look (P3-514).
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Span {
    /// The text.
    pub text: String,
    /// Bold.
    pub strong: bool,
    /// Italic.
    pub emphasis: bool,
    /// Inline code.
    pub code: bool,
    /// The link target, if inside a link.
    pub link: Option<String>,
}

/// Where an image comes from (P3-514).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ImageSource {
    /// A path relative to the docs folder.
    Relative(String),
    /// An `http(s)://` URL: not fetched without network (P3-600).
    Remote(String),
    /// `embed:<view key>` (P3-517).
    Embed(String),
}

impl ImageSource {
    /// Classifies an image destination.
    #[must_use]
    pub fn of(dest: &str) -> ImageSource {
        let lower = dest.to_ascii_lowercase();
        if let Some(key) = dest.strip_prefix("embed:") {
            ImageSource::Embed(key.trim().to_owned())
        } else if lower.starts_with("http://") || lower.starts_with("https://") {
            ImageSource::Remote(dest.to_owned())
        } else {
            ImageSource::Relative(dest.to_owned())
        }
    }
}

/// One block of a rendered section (P3-514).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Block {
    /// `#`..`######`.
    Heading {
        /// 1..6.
        level: u8,
        /// The text.
        spans: Vec<Span>,
    },
    /// A paragraph.
    Paragraph(Vec<Span>),
    /// A list.
    List {
        /// The first number of an ordered list, `None` for bullets.
        start: Option<u64>,
        /// Each item's blocks.
        items: Vec<Vec<Block>>,
    },
    /// A fenced or indented code block.
    Code(String),
    /// A table.
    Table {
        /// The header cells.
        header: Vec<Vec<Span>>,
        /// The body rows.
        rows: Vec<Vec<Vec<Span>>>,
    },
    /// An image on its own.
    Image {
        /// The alt text.
        alt: String,
        /// The source.
        source: ImageSource,
    },
    /// A block quote.
    Quote(Vec<Block>),
    /// A thematic break.
    Rule,
}

/// A container being filled.
enum Frame {
    /// The document, a block quote or a list item.
    Blocks { quote: bool, blocks: Vec<Block> },
    /// A list.
    List { start: Option<u64>, items: Vec<Vec<Block>> },
}

/// Where inline text is going.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Inline {
    /// A paragraph (written or implied by a tight list item).
    Paragraph,
    /// A heading of this level.
    Heading(u8),
    /// A table cell.
    Cell,
}

/// A table being filled: header cells, body rows.
type TableParts = (Vec<Vec<Span>>, Vec<Vec<Vec<Span>>>);

/// The block builder.
struct Builder {
    frames: Vec<Frame>,
    inline: Option<Inline>,
    spans: Vec<Span>,
    strong: u32,
    emphasis: u32,
    links: Vec<String>,
    image: Option<(String, String)>,
    code: Option<String>,
    table: Option<TableParts>,
    row: Vec<Vec<Span>>,
}

impl Builder {
    fn push_block(&mut self, block: Block) {
        if let Some(Frame::Blocks { blocks, .. }) = self.frames.last_mut() {
            blocks.push(block);
        }
    }

    fn text(&mut self, text: &str, code: bool) {
        if let Some((_, alt)) = &mut self.image {
            alt.push_str(text);
            return;
        }
        if let Some(c) = &mut self.code {
            c.push_str(text);
            return;
        }
        if self.inline.is_none() {
            self.inline = Some(Inline::Paragraph);
        }
        let span = Span {
            text: text.to_owned(),
            strong: self.strong > 0,
            emphasis: self.emphasis > 0,
            code,
            link: self.links.last().cloned(),
        };
        match self.spans.last_mut() {
            Some(last)
                if !code
                    && !last.code
                    && (last.strong, last.emphasis, &last.link) == (span.strong, span.emphasis, &span.link) =>
            {
                last.text.push_str(text);
            }
            _ => self.spans.push(span),
        }
    }

    /// Ends the inline run as its block.
    fn flush(&mut self) {
        let Some(kind) = self.inline.take() else { return };
        let spans = std::mem::take(&mut self.spans);
        match kind {
            Inline::Paragraph => {
                if spans.iter().any(|s| !s.text.trim().is_empty()) {
                    self.push_block(Block::Paragraph(spans));
                }
            }
            Inline::Heading(level) => self.push_block(Block::Heading { level, spans }),
            Inline::Cell => self.row.push(spans),
        }
    }

    fn start(&mut self, tag: Tag<'_>) {
        match tag {
            Tag::Paragraph => {
                self.flush();
                self.inline = Some(Inline::Paragraph);
            }
            Tag::Heading { level, .. } => {
                self.flush();
                self.inline = Some(Inline::Heading(level as u8));
            }
            Tag::BlockQuote(_) => {
                self.flush();
                self.frames.push(Frame::Blocks { quote: true, blocks: Vec::new() });
            }
            Tag::CodeBlock(_) => {
                self.flush();
                self.code = Some(String::new());
            }
            Tag::List(start) => {
                self.flush();
                self.frames.push(Frame::List { start, items: Vec::new() });
            }
            Tag::Item => {
                self.flush();
                self.frames.push(Frame::Blocks { quote: false, blocks: Vec::new() });
            }
            Tag::Table(_) => {
                self.flush();
                self.table = Some((Vec::new(), Vec::new()));
            }
            Tag::TableHead | Tag::TableRow => self.row.clear(),
            Tag::TableCell => {
                self.inline = Some(Inline::Cell);
                self.spans.clear();
            }
            Tag::Emphasis => self.emphasis += 1,
            Tag::Strong => self.strong += 1,
            Tag::Link { dest_url, .. } => self.links.push(dest_url.into_string()),
            Tag::Image { dest_url, .. } => self.image = Some((dest_url.into_string(), String::new())),
            _ => {}
        }
    }

    fn end(&mut self, tag: TagEnd) {
        match tag {
            TagEnd::Paragraph | TagEnd::Heading(_) | TagEnd::TableCell => self.flush(),
            TagEnd::BlockQuote(_) => {
                self.flush();
                if let Some(Frame::Blocks { quote: true, blocks }) = self.frames.pop() {
                    self.push_block(Block::Quote(blocks));
                }
            }
            TagEnd::CodeBlock => {
                let mut code = self.code.take().unwrap_or_default();
                if code.ends_with('\n') {
                    code.pop();
                }
                self.push_block(Block::Code(code));
            }
            TagEnd::List(_) => {
                self.flush();
                if let Some(Frame::List { start, items }) = self.frames.pop() {
                    self.push_block(Block::List { start, items });
                }
            }
            TagEnd::Item => {
                self.flush();
                if let Some(Frame::Blocks { quote: false, blocks }) = self.frames.pop()
                    && let Some(Frame::List { items, .. }) = self.frames.last_mut()
                {
                    items.push(blocks);
                }
            }
            TagEnd::TableHead => {
                if let Some((header, _)) = &mut self.table {
                    *header = std::mem::take(&mut self.row);
                }
            }
            TagEnd::TableRow => {
                if let Some((_, rows)) = &mut self.table {
                    rows.push(std::mem::take(&mut self.row));
                }
            }
            TagEnd::Table => {
                if let Some((header, rows)) = self.table.take() {
                    self.push_block(Block::Table { header, rows });
                }
            }
            TagEnd::Emphasis => self.emphasis = self.emphasis.saturating_sub(1),
            TagEnd::Strong => self.strong = self.strong.saturating_sub(1),
            TagEnd::Link => {
                self.links.pop();
            }
            TagEnd::Image => self.end_image(),
            _ => {}
        }
    }

    fn end_image(&mut self) {
        let Some((dest, alt)) = self.image.take() else { return };
        if self.inline == Some(Inline::Cell) {
            self.text(&alt, false);
            return;
        }
        let kind = self.inline;
        self.flush();
        self.push_block(Block::Image {
            alt,
            source: ImageSource::of(&dest),
        });
        if kind == Some(Inline::Paragraph) {
            self.inline = Some(Inline::Paragraph);
        }
    }
}

/// The blocks of `text` (P3-514).
#[must_use]
pub fn blocks(text: &str) -> Vec<Block> {
    let mut b = Builder {
        frames: vec![Frame::Blocks { quote: false, blocks: Vec::new() }],
        inline: None,
        spans: Vec::new(),
        strong: 0,
        emphasis: 0,
        links: Vec::new(),
        image: None,
        code: None,
        table: None,
        row: Vec::new(),
    };
    let options = Options::ENABLE_TABLES | Options::ENABLE_STRIKETHROUGH;
    for event in Parser::new_ext(text, options) {
        match event {
            Event::Start(tag) => b.start(tag),
            Event::End(tag) => b.end(tag),
            Event::Text(t) | Event::Html(t) | Event::InlineHtml(t) => b.text(&t, false),
            Event::Code(t) => b.text(&t, true),
            Event::SoftBreak => b.text(" ", false),
            Event::HardBreak => b.text("\n", false),
            Event::Rule => {
                b.flush();
                b.push_block(Block::Rule);
            }
            _ => {}
        }
    }
    b.flush();
    match b.frames.into_iter().next() {
        Some(Frame::Blocks { blocks, .. }) => blocks,
        _ => Vec::new(),
    }
}

/// The plain text of `spans`.
#[must_use]
pub fn plain(spans: &[Span]) -> String {
    spans.iter().map(|s| s.text.as_str()).collect()
}

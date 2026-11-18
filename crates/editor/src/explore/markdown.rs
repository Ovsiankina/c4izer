//! Markdown to drawable blocks with `pulldown-cmark` (P3-514, P3-517):
//! headings, paragraphs, emphasis, lists, code blocks, tables, links,
//! images and `embed:` view references.
//!
//! Depends on: `pulldown-cmark`.

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

/// The blocks of `text` (P3-514).
#[must_use]
pub fn blocks(text: &str) -> Vec<Block> {
    let _ = text;
    todo!()
}

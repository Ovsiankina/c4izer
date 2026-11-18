//! Reads the files `!docs` and `!adrs` attach (P3-513, P3-516): the
//! editor does this I/O, core does none.
//!
//! Depends on: `c4rizer-core` (`model`, `explore`).

use std::path::PathBuf;

use c4rizer_core::explore::Decision;
use c4rizer_core::model::{ElementId, Parsed};

/// How a section is shown (P3-514, P3-515).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Format {
    /// `.md`, `.markdown`, `.text`: rendered.
    Markdown,
    /// `.adoc`, `.asciidoc`, `.asc`: shown as source.
    AsciiDoc,
}

/// One documentation section (P3-513).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Section {
    /// The element it is attached to, `None` for the workspace.
    pub owner: Option<ElementId>,
    /// The file.
    pub path: PathBuf,
    /// The title: the first heading, else the file name.
    pub title: String,
    /// Markdown or AsciiDoc.
    pub format: Format,
    /// The content.
    pub text: String,
}

/// One decision file (P3-516).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DecisionFile {
    /// The element it is attached to, `None` for the workspace.
    pub owner: Option<ElementId>,
    /// The file.
    pub path: PathBuf,
    /// The parsed record.
    pub decision: Decision,
    /// The content.
    pub text: String,
}

/// The format of `path` by its extension, `None` for any other file
/// (Structurizr's `FormatFinder`).
#[must_use]
pub fn format_of(path: &std::path::Path) -> Option<Format> {
    let _ = path;
    todo!()
}

/// Every documentation section of `parsed`: the workspace's first, then
/// each element's in model order; within one `!docs`, file-name order
/// (P3-513).
#[must_use]
pub fn sections(parsed: &Parsed) -> Vec<Section> {
    let _ = parsed;
    todo!()
}

/// Every decision of `parsed`, sorted by id (P3-516).
#[must_use]
pub fn decisions(parsed: &Parsed) -> Vec<DecisionFile> {
    let _ = parsed;
    todo!()
}

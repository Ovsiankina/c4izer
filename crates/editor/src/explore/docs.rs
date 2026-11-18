//! Reads the files `!docs` and `!adrs` attach (P3-513, P3-516): the
//! editor does this I/O, core does none.
//!
//! Depends on: `c4rizer-core` (`model`, `explore`).

// DECISION: a `!docs` folder is read one level deep, files in file-name
// order, dot files skipped, Markdown (`.md`, `.markdown`, `.text`) and
// AsciiDoc (`.adoc`, `.asciidoc`, `.asc`) only: Structurizr 6.2.4's
// `DefaultDocumentationImporter` and `FormatFinder` (the recursive
// importer is a separate FQCN). A path naming one file is that one
// section. Sections of every element with `!docs` are listed (Structurizr
// renders container and component documentation too), the workspace's
// first, then by model order.
// DECISION: an `!adrs` folder: the `.md` files whose name starts with a
// digit (adr-tools, MADR), or every `.md` file but `README.md`,
// `index.md` and `template.md` for `log4brains`.
// TODO(srs): P3-513/P3-516 with `exclude` patterns. The model checks and
// drops the `{ exclude ... }` block (P2-159), so they are not applied
// here; deferred row "explore exclude patterns".
// DECISION: unreadable files and folders are skipped silently: the parse
// already checked the path exists (P2-159), and a file removed since then
// is gone from the list on the next reload.

use std::path::{Path, PathBuf};

use c4rizer_core::explore::{Decision, parse_decision};
use c4rizer_core::model::{Attachment, AttachmentKind, ElementId, Parsed};

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
    /// Markdown or `AsciiDoc`.
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
pub fn format_of(path: &Path) -> Option<Format> {
    let ext = path.extension()?.to_str()?.to_ascii_lowercase();
    match ext.as_str() {
        "md" | "markdown" | "text" => Some(Format::Markdown),
        "adoc" | "asciidoc" | "asc" => Some(Format::AsciiDoc),
        _ => None,
    }
}

/// Every attachment of `kind` with its owner: the workspace's, then each
/// element's in model order.
fn attachments(parsed: &Parsed, kind: AttachmentKind) -> Vec<(Option<ElementId>, &Attachment)> {
    let model = &parsed.model;
    let mut out: Vec<(Option<ElementId>, &Attachment)> =
        model.docs.iter().filter(|a| a.kind == kind).map(|a| (None, a)).collect();
    for e in &model.elements {
        out.extend(e.docs.iter().filter(|a| a.kind == kind).map(|a| (Some(e.id), a)));
    }
    out
}

/// Where `attachment`'s path points: resolved against the folder of the
/// file it is written in.
fn resolve(parsed: &Parsed, attachment: &Attachment) -> Option<PathBuf> {
    let file = parsed.paths.get(attachment.path.span.file.0 as usize)?;
    let folder = file.parent().unwrap_or_else(|| Path::new("."));
    Some(folder.join(&attachment.path.value))
}

/// The files of `path`, one level deep, by name; `path` itself when it is
/// a file.
fn files(path: &Path) -> Vec<PathBuf> {
    if path.is_file() {
        return vec![path.to_owned()];
    }
    let Ok(read) = std::fs::read_dir(path) else { return Vec::new() };
    let mut out: Vec<PathBuf> = read
        .filter_map(Result::ok)
        .map(|e| e.path())
        .filter(|p| p.is_file())
        .filter(|p| !p.file_name().and_then(|n| n.to_str()).is_some_and(|n| n.starts_with('.')))
        .collect();
    out.sort();
    out
}

/// The text of `path`, invalid UTF-8 replaced.
fn read(path: &Path) -> Option<String> {
    std::fs::read(path).ok().map(|b| String::from_utf8_lossy(&b).into_owned())
}

/// The section title: the first Markdown (`#`) or `AsciiDoc` (`=`)
/// heading, else the file name.
fn title_of(path: &Path, text: &str, format: Format) -> String {
    let mark = match format {
        Format::Markdown => '#',
        Format::AsciiDoc => '=',
    };
    text.lines()
        .find_map(|l| {
            let rest = l.trim_start_matches(mark);
            (rest.len() < l.len() && rest.starts_with(' ')).then(|| rest.trim().to_owned())
        })
        .filter(|t| !t.is_empty())
        .unwrap_or_else(|| path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default())
}

/// Every documentation section of `parsed`: the workspace's first, then
/// each element's in model order; within one `!docs`, file-name order
/// (P3-513).
#[must_use]
pub fn sections(parsed: &Parsed) -> Vec<Section> {
    let mut out = Vec::new();
    for (owner, attachment) in attachments(parsed, AttachmentKind::Docs) {
        let Some(path) = resolve(parsed, attachment) else { continue };
        for file in files(&path) {
            let Some(format) = format_of(&file) else { continue };
            let Some(text) = read(&file) else { continue };
            out.push(Section {
                owner,
                title: title_of(&file, &text, format),
                path: file,
                format,
                text,
            });
        }
    }
    out
}

/// The numeric part of an id for sorting: ids that are numbers sort as
/// numbers, before any other id.
fn id_key(id: &str) -> (u8, u64, String) {
    id.parse::<u64>().map_or_else(|_| (1, 0, id.to_owned()), |n| (0, n, String::new()))
}

/// Every decision of `parsed`, by owner (workspace first, then model
/// order), then by id (P3-516).
#[must_use]
pub fn decisions(parsed: &Parsed) -> Vec<DecisionFile> {
    let mut out = Vec::new();
    for (rank, (owner, attachment)) in attachments(parsed, AttachmentKind::Adrs).into_iter().enumerate() {
        let Some(path) = resolve(parsed, attachment) else { continue };
        let importer = attachment.importer.as_ref().map(|i| i.value.as_str());
        let log4brains = importer.is_some_and(|i| i.eq_ignore_ascii_case("log4brains"));
        let mut group = Vec::new();
        for file in files(&path) {
            let Some(name) = file.file_name().and_then(|n| n.to_str()).map(str::to_owned) else { continue };
            let lower = name.to_ascii_lowercase();
            if !Path::new(&name).extension().is_some_and(|x| x.eq_ignore_ascii_case("md")) {
                continue;
            }
            let wanted = if log4brains {
                !matches!(lower.as_str(), "readme.md" | "index.md" | "template.md")
            } else {
                name.starts_with(|c: char| c.is_ascii_digit())
            };
            if !wanted {
                continue;
            }
            let Some(text) = read(&file) else { continue };
            group.push((rank, DecisionFile {
                owner,
                decision: parse_decision(&name, &text, importer),
                path: file,
                text,
            }));
        }
        out.extend(group);
    }
    out.sort_by(|(ra, a), (rb, b)| {
        ra.cmp(rb)
            .then_with(|| id_key(&a.decision.id).cmp(&id_key(&b.decision.id)))
            .then_with(|| a.path.cmp(&b.path))
    });
    out.into_iter().map(|(_, d)| d).collect()
}

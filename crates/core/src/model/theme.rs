//! Themes (P2-13, P2-427..P2-429): reading `Model::themes` into theme
//! styles after the file is built. A theme file is Structurizr theme JSON
//! read through the `Loader`; `default` is bundled; any other name or
//! URL gets a P2-3 notice and draws nothing. Core stays free of I/O: every
//! read goes through the `Loader` (HLD).

use std::path::{Component, Path, PathBuf};
use std::sync::Arc;

use super::{
    Asset, Border, ElementStyle, Loader, Mime, Model, ModelError, ModelErrorKind, Notice,
    NoticeKind, RelationshipStyle, Routing, Shape, Spanned, Theme, ThemeSource, parse_colour,
};
use crate::text::{self, Span, SyntaxTree};

/// Structurizr's default theme, bundled. Source: github.com/structurizr/
/// themes `default/theme.json`, copied on 2026-11-17.
///
/// Copyright 2022 Structurizr Limited. Licensed under the Apache License,
/// Version 2.0 (the "License"); you may not use this file except in
/// compliance with the License. You may obtain a copy of the License at
/// <http://www.apache.org/licenses/LICENSE-2.0>. Unless required by
/// applicable law or agreed to in writing, software distributed under
/// the License is distributed on an "AS IS" BASIS, WITHOUT WARRANTIES OR
/// CONDITIONS OF ANY KIND, either express or implied.
// DECISION: `theme default` draws in phase 2 (approver C): the
// structurizr/themes README licenses every theme.json under Apache-2.0,
// which allows bundling with this notice; the PNG icons, owned by others,
// are not bundled (the default theme has none).
const DEFAULT_THEME: &str = r##"{
  "name" : "Default theme",
  "description" : "Description",
  "elements" : [ {
    "tag" : "Boundary:Container",
    "color" : "#438dd5"
  }, {
    "tag" : "Boundary:SoftwareSystem",
    "color" : "#1168bd"
  }, {
    "tag" : "Component",
    "background" : "#85bbf0",
    "color" : "#ffffff"
  }, {
    "tag" : "Container",
    "background" : "#438dd5",
    "color" : "#ffffff"
  }, {
    "tag" : "Element",
    "shape" : "RoundedBox"
  }, {
    "tag" : "Person",
    "background" : "#08427b",
    "color" : "#ffffff",
    "shape" : "Person"
  }, {
    "tag" : "Software System",
    "background" : "#1168bd",
    "color" : "#ffffff"
  } ]
}"##;

/// The URL Structurizr resolves `theme default` to; written out, it reads
/// the bundled copy too (same content, no network).
const DEFAULT_THEME_URL: &str = "https://static.structurizr.com/themes/default/theme.json";

/// What a theme source names: `default` (any case) is the only name, an
/// `http://`/`https://` value a URL, anything else a file path.
// DECISION: Structurizr for Java's DSL knows one theme name, `default`;
// every other bare value is a file relative to the DSL file.
pub(super) fn classify(source: &str) -> ThemeSource {
    let lower = source.to_ascii_lowercase();
    if lower == "default" {
        ThemeSource::Name("default".into())
    } else if lower.starts_with("https://") || lower.starts_with("http://") {
        ThemeSource::Url(source.into())
    } else {
        ThemeSource::File(source.into())
    }
}

/// File access for theme files: the diagram file's folder and the loader.
pub(super) struct Files<'a> {
    /// The diagram file; `None` for stdin or in-memory text.
    pub(super) path: Option<&'a Path>,
    /// Every read goes through it.
    pub(super) loader: &'a mut dyn Loader,
}

/// Reads every theme of `model.themes` into theme styles, put before the
/// file's own (`Model::element_styles`, `Model::relationship_styles`),
/// loads their icons into `Model::assets`, and adds a notice for each URL
/// theme. `files` is `None` when the text was parsed with no loader
/// (`parse`, `Model::from_tree`).
///
/// # Errors
///
/// `NeedsFile`, `PathOutsideRoot`, `FileRead`, `FileFormat`, at the
/// theme's source token.
pub(super) fn apply(
    model: &mut Model,
    trees: &[SyntaxTree],
    mut files: Option<Files<'_>>,
) -> Result<(), ModelError> {
    apply_themes(model, trees, &mut files)?;
    file_icons(model, trees, &mut files)?;
    view_images(model, trees, &mut files)
}

/// Reads the file of every `image <file>` view source (P2-320) through
/// the loader into `ViewImage::asset` and `Model::assets`, after the
/// themes and icons. A file is read once, keyed by its path relative to
/// the diagram's folder.
///
/// # Errors
///
/// `PathOutsideRoot`, `FileRead`, `FileFormat` (not a PNG, JPEG or SVG
/// image), at the source line.
// TODO(srs): P2-322 (a), (b). DECISION: readings taken (B
// landscape-filtered 10a, 10b) and checked against Structurizr 6.2.4: the
// path is relative to the diagram file's folder and must stay under it;
// a missing file is `FileRead`; a file that is no image is `FileFormat`
// naming the path. Structurizr also takes JPEG files ("not a supported
// image file" otherwise), so JPEG is accepted too (B 10b said PNG or SVG).
// DECISION: text parsed with no path (`parse`, stdin) keeps the source as
// written and reads nothing; the view then draws a placeholder naming the
// file, as an icon is then not drawn (B styles-themes 7e).
fn view_images(
    model: &mut Model,
    trees: &[SyntaxTree],
    files: &mut Option<Files<'_>>,
) -> Result<(), ModelError> {
    let no_path = files.as_ref().is_none_or(|f| f.path.is_none());
    if no_path {
        return Ok(());
    }
    for index in 0..model.views.len() {
        let Some(image) = &model.views[index].image else {
            continue;
        };
        let super::ImageSource::File(written) = &image.source else {
            continue;
        };
        let (written, span) = (written.value.clone(), image.span);
        let path = normalise(&written);
        let error = |kind| ModelError {
            kind,
            span,
            position: position(trees, span),
        };
        let asset = if let Some(a) = model.assets.iter().find(|a| a.path == path) {
            a.clone()
        } else {
            let Some(files) = files.as_mut() else {
                return Ok(());
            };
            let bytes = read(files, &path).map_err(error)?;
            let Some(mime) = sniff(&bytes) else {
                return Err(error(format_error(&path, "the image is not a PNG, JPEG or SVG file")));
            };
            let asset = Asset {
                path,
                mime,
                bytes: Arc::from(bytes),
            };
            model.assets.push(asset.clone());
            asset
        };
        if let Some(image) = model.views[index].image.as_mut() {
            image.asset = Some(asset);
        }
    }
    Ok(())
}

/// Loads the `icon` of every file element style (P2-409): a local file
/// relative to the diagram's folder through the loader, a `data:` URI
/// decoded, a URL left undrawn with a P2-3 notice. Theme icons were
/// loaded with their theme.
///
/// # Errors
///
/// `NeedsFile`, `PathOutsideRoot`, `FileRead`, `FileFormat`, at the icon
/// value.
// TODO(srs): P2-409. Readings taken (B styles-themes 7c, 7d, 7e): PNG,
// JPEG and SVG files and `data:` URIs; a URL icon is not drawn and gets
// a P2-3 notice; a missing or unreadable file is a `FileRead` error.
fn file_icons(
    model: &mut Model,
    trees: &[SyntaxTree],
    files: &mut Option<Files<'_>>,
) -> Result<(), ModelError> {
    let icons: Vec<(Spanned<String>, Span)> = model
        .element_styles
        .iter()
        .filter(|s| s.theme.is_none())
        .filter_map(|s| s.icon.clone().map(|i| (i, s.span)))
        .collect();
    for (mut icon, statement) in icons {
        let written = icon.value.clone();
        let lower = written.to_ascii_lowercase();
        // DECISION: text parsed without a path (`parse`, stdin) keeps a
        // file icon as written and draws none, rather than refusing the
        // file; with a path a missing file is `FileRead` (B ruling 7e).
        let no_path = files.as_ref().is_none_or(|f| f.path.is_none());
        if no_path && !lower.starts_with("data:") && !lower.starts_with("http") {
            continue;
        }
        let loaded = load_icon(model, files, None, &mut icon).map_err(|kind| ModelError {
            kind,
            span: icon.span,
            position: position(trees, icon.span),
        })?;
        // A file icon keeps its path as written (DSL-11); `load_icon` keys
        // the asset by the folded path, so key it by the written one too.
        if icon.value != written
            && let Some(asset) = model.assets.iter().find(|a| a.path == icon.value).cloned()
            && !model.assets.iter().any(|a| a.path == written)
        {
            model.assets.push(Asset {
                path: written.clone(),
                ..asset
            });
        }
        if let Some(subject) = loaded {
            model.notices.push(Notice {
                kind: NoticeKind::NoEffect,
                subject,
                view: None,
                span: Some(statement),
                position: Some(position(trees, statement)),
            });
        }
    }
    Ok(())
}

/// Reads every theme of `model.themes` (see `apply`).
fn apply_themes(
    model: &mut Model,
    trees: &[SyntaxTree],
    files: &mut Option<Files<'_>>,
) -> Result<(), ModelError> {
    if model.themes.is_empty() {
        return Ok(());
    }
    let error = |kind: ModelErrorKind, span: Span| ModelError {
        kind,
        span,
        position: position(trees, span),
    };
    let mut element_styles = Vec::new();
    let mut relationship_styles = Vec::new();
    let themes = model.themes.clone();
    for (index, theme) in themes.iter().enumerate() {
        let source = theme.source.span;
        let (json, folder) = match &theme.kind {
            ThemeSource::Name(_) => (DEFAULT_THEME.to_owned(), None),
            ThemeSource::Url(url) if url == DEFAULT_THEME_URL => (DEFAULT_THEME.to_owned(), None),
            ThemeSource::Url(url) => {
                model.notices.push(notice(trees, theme, &format!("theme {url}")));
                continue;
            }
            ThemeSource::File(path) => {
                let Some(files) = files.as_mut() else {
                    return Err(error(needs_file(), source));
                };
                let bytes = read(files, path).map_err(|kind| error(kind, source))?;
                let json = String::from_utf8(bytes).map_err(|_| {
                    error(format_error(path, "the file is not UTF-8 text"), source)
                })?;
                let folder = parent(path);
                (json, Some(folder))
            }
        };
        let label = match &theme.kind {
            ThemeSource::File(path) => path.clone(),
            _ => "default".into(),
        };
        let value = json::parse(&json)
            .map_err(|reason| error(format_error(&label, &reason), source))?;
        let styles = ThemeStyles::read(&value, index, theme.span)
            .map_err(|reason| error(format_error(&label, &reason), source))?;
        for mut style in styles.elements {
            if let Some(icon) = style.icon.as_mut() {
                let loaded = load_icon(model, files, folder.as_deref(), icon);
                if let Some(subject) = loaded.map_err(|kind| error(kind, source))? {
                    model.notices.push(notice(trees, theme, &subject));
                }
            }
            element_styles.push(style);
        }
        relationship_styles.extend(styles.relationships);
    }
    element_styles.append(&mut model.element_styles);
    model.element_styles = element_styles;
    relationship_styles.append(&mut model.relationship_styles);
    model.relationship_styles = relationship_styles;
    Ok(())
}

/// A P2-3 notice for `subject`, at `theme`'s statement.
fn notice(trees: &[SyntaxTree], theme: &Theme, subject: &str) -> Notice {
    Notice {
        kind: NoticeKind::NoEffect,
        subject: subject.into(),
        view: None,
        span: Some(theme.span),
        position: Some(position(trees, theme.span)),
    }
}

/// Line and column of `span` in the tree of its file.
fn position(trees: &[SyntaxTree], span: Span) -> text::Position {
    let source = trees.get(span.file.0 as usize).map_or("", SyntaxTree::source);
    text::position(source, span.start)
}

fn needs_file() -> ModelErrorKind {
    ModelErrorKind::NeedsFile {
        construct: "theme".into(),
    }
}

fn format_error(path: &str, reason: &str) -> ModelErrorKind {
    ModelErrorKind::FileFormat {
        path: path.into(),
        reason: reason.into(),
    }
}

/// Resolves `icon` (relative to the theme's `folder`, or to the diagram's
/// folder for `None`) to a path relative to the diagram's folder, reads
/// it once into `Model::assets` and rewrites the style's icon to that
/// path (P2-428). A `data:` URI is decoded into an asset keyed by the URI
/// itself. A URL loads nothing; the result is then the subject of the
/// P2-3 notice to add.
// TODO(srs): P2-409 (B styles-themes 7c, 7d): a URL icon gets a P2-3
// notice and is not drawn; a `data:` URI is read as Structurizr reads it
// (`data_uri`).
fn load_icon(
    model: &mut Model,
    files: &mut Option<Files<'_>>,
    folder: Option<&str>,
    icon: &mut Spanned<String>,
) -> Result<Option<String>, ModelErrorKind> {
    let lower = icon.value.to_ascii_lowercase();
    if lower.starts_with("https://") || lower.starts_with("http://") {
        return Ok(Some(format!("icon {}", icon.value)));
    }
    if icon.value.starts_with("data:") {
        if !model.assets.iter().any(|a| a.path == icon.value) {
            let Some((mime, bytes)) = data_uri(&icon.value)? else {
                return Ok(Some(undrawn(&icon.value)));
            };
            model.assets.push(Asset {
                path: icon.value.clone(),
                mime,
                bytes: Arc::from(bytes),
            });
        }
        return Ok(None);
    }
    let Some(files) = files.as_mut() else {
        // The bundled default theme has no icons; a file theme always
        // comes with files.
        return Err(needs_file());
    };
    let path = match folder {
        Some(folder) if !folder.is_empty() => format!("{folder}/{}", icon.value),
        _ => icon.value.clone(),
    };
    let path = normalise(&path);
    icon.value.clone_from(&path);
    if model.assets.iter().any(|a| a.path == path) {
        return Ok(None);
    }
    let bytes = read(files, &path)?;
    let Some(mime) = sniff(&bytes) else {
        if image_name(&path) {
            return Ok(Some(undrawn(&path)));
        }
        return Err(format_error(&path, "the icon is not a PNG, JPEG or SVG image"));
    };
    model.assets.push(Asset {
        path,
        mime,
        bytes: Arc::from(bytes),
    });
    Ok(None)
}

/// The type and bytes of a `data:` URI icon, read as Structurizr reads
/// it (`ImageUtils.isSupportedDataUri`): `data:image/png;base64,`,
/// `data:image/jpeg;base64,` or `data:image/svg+xml;...,` (an SVG may be
/// text, percent-encoded or not). The payload is not checked against the
/// declared type; recognised bytes of another type are drawn as what
/// they are.
// DECISION: B ruling 7c [I] checked against Structurizr 6.2.4 (the
// installed reference; Java wins): only these three prefixes, matched
// case-sensitively as Java does; `image/jpg` is refused. A malformed
// base64 payload is `Ok(None)`: Java accepts it (it never decodes at
// parse time), and there are no bytes to draw (approver ruling
// 2026-11-17: not drawn, with a notice).
fn data_uri(uri: &str) -> Result<Option<(Mime, Vec<u8>)>, ModelErrorKind> {
    let bad = |why: &str| format_error("data: URI icon", why);
    let (declared, bytes) = if let Some(payload) = uri.strip_prefix("data:image/png;base64,") {
        (Mime::Png, base64_decode(payload))
    } else if let Some(payload) = uri.strip_prefix("data:image/jpeg;base64,") {
        (Mime::Jpeg, base64_decode(payload))
    } else if let Some(rest) = uri.strip_prefix("data:image/svg+xml;") {
        let Some((params, payload)) = rest.split_once(',') else {
            return Err(bad("it has no `,` before the data"));
        };
        let bytes = if params.split(';').any(|p| p == "base64") {
            base64_decode(payload)
        } else {
            Some(percent_decode(payload))
        };
        (Mime::Svg, bytes)
    } else {
        return Err(bad(
            "it does not start with data:image/png;base64, data:image/jpeg;base64, or data:image/svg+xml;",
        ));
    };
    Ok(bytes.map(|bytes| (sniff(&bytes).unwrap_or(declared), bytes)))
}

/// The P2-3 notice subject of an icon Structurizr accepts and c4rizer
/// cannot draw (approver ruling 2026-11-17).
fn undrawn(icon: &str) -> String {
    format!("icon {icon} (not a PNG, JPEG or SVG image c4rizer can draw)")
}

/// True when `path`'s name gives an `image/*` type, as Structurizr's
/// `URLConnection.guessContentTypeFromName` does; such a file is accepted
/// there whatever its content.
// DECISION: the image extensions of the JDK's content-types table
// (plus `webp`, `avif`, `ico`), any case.
fn image_name(path: &str) -> bool {
    const IMAGE: &[&str] = &[
        "avif", "bmp", "fpx", "gif", "ico", "ief", "jfif", "jpe", "jpeg", "jpg", "pbm", "pgm",
        "png", "pnm", "ppm", "ras", "rgb", "svg", "tif", "tiff", "webp", "xbm", "xpm", "xwd",
    ];
    Path::new(path)
        .extension()
        .and_then(|e| e.to_str())
        .is_some_and(|e| IMAGE.iter().any(|i| i.eq_ignore_ascii_case(e)))
}

/// `%XX` escapes decoded; anything else kept as written.
fn percent_decode(text: &str) -> Vec<u8> {
    let raw = text.as_bytes();
    let mut out = Vec::with_capacity(raw.len());
    let mut i = 0;
    while i < raw.len() {
        let hex = raw.get(i + 1..i + 3).and_then(|h| std::str::from_utf8(h).ok());
        match (raw[i], hex.and_then(|h| u8::from_str_radix(h, 16).ok())) {
            (b'%', Some(byte)) => {
                out.push(byte);
                i += 3;
            }
            (c, _) => {
                out.push(c);
                i += 1;
            }
        }
    }
    out
}

/// Standard base64 (RFC 4648, `=` padding optional, whitespace skipped).
fn base64_decode(text: &str) -> Option<Vec<u8>> {
    let mut out = Vec::with_capacity(text.len() * 3 / 4);
    let (mut acc, mut bits) = (0u32, 0u32);
    for c in text.bytes() {
        let v = match c {
            b'A'..=b'Z' => c - b'A',
            b'a'..=b'z' => c - b'a' + 26,
            b'0'..=b'9' => c - b'0' + 52,
            b'+' => 62,
            b'/' => 63,
            b'=' | b' ' | b'\n' | b'\r' | b'\t' => continue,
            _ => return None,
        };
        acc = (acc << 6) | u32::from(v);
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push(u8::try_from((acc >> bits) & 0xff).ok()?);
        }
    }
    Some(out)
}

/// The type of an image file, from its first bytes.
fn sniff(bytes: &[u8]) -> Option<Mime> {
    if bytes.starts_with(b"\x89PNG\r\n\x1a\n") {
        return Some(Mime::Png);
    }
    if bytes.starts_with(&[0xff, 0xd8, 0xff]) {
        return Some(Mime::Jpeg);
    }
    let text = std::str::from_utf8(bytes).ok()?;
    let trimmed = text.trim_start_matches('\u{feff}').trim_start();
    (trimmed.starts_with('<') && text.contains("<svg")).then_some(Mime::Svg)
}

/// Reads `path` (relative to the diagram file's folder) through the
/// loader, refusing a path outside that folder (`PathOutsideRoot`).
// DECISION: the folder check is done twice, on the written path with `..`
// resolved by text and on the loader's canonical path (symlinks), so a
// path that leaves the folder is refused before anything is read.
fn read(files: &mut Files<'_>, path: &str) -> Result<Vec<u8>, ModelErrorKind> {
    let Some(diagram) = files.path else {
        return Err(needs_file());
    };
    let outside = || ModelErrorKind::PathOutsideRoot { path: path.into() };
    let read_error = |e: std::io::Error| ModelErrorKind::FileRead {
        path: path.into(),
        error: e.to_string(),
    };
    let folder = diagram.parent().unwrap_or_else(|| Path::new(""));
    let written = Path::new(path);
    if written.is_absolute() || lexical(written).starts_with("..") {
        return Err(outside());
    }
    let root = files.loader.canonicalize(folder).map_err(read_error)?;
    let full = files
        .loader
        .canonicalize(&folder.join(written))
        .map_err(read_error)?;
    if !full.starts_with(&root) {
        return Err(outside());
    }
    files.loader.read(&full).map_err(read_error)
}

/// `path` with `.` dropped and `..` folded by text; a path that climbs
/// above its start keeps `..` as its first component.
fn lexical(path: &Path) -> PathBuf {
    let mut out: Vec<Component<'_>> = Vec::new();
    for c in path.components() {
        match c {
            Component::CurDir => {}
            Component::ParentDir => {
                if matches!(out.last(), Some(Component::Normal(_))) {
                    out.pop();
                } else {
                    out.push(c);
                }
            }
            other => out.push(other),
        }
    }
    out.iter().collect()
}

/// `path` folded by text, with `/` separators.
fn normalise(path: &str) -> String {
    lexical(Path::new(path))
        .components()
        .map(|c| c.as_os_str().to_string_lossy().into_owned())
        .collect::<Vec<_>>()
        .join("/")
}

/// The folder part of a relative path written with `/` (or `\`).
fn parent(path: &str) -> String {
    let normal = normalise(path);
    match normal.rfind('/') {
        Some(i) => normal[..i].to_owned(),
        None => String::new(),
    }
}

/// The styles of one theme file.
struct ThemeStyles {
    elements: Vec<ElementStyle>,
    relationships: Vec<RelationshipStyle>,
}

impl ThemeStyles {
    /// Reads the `elements` and `relationships` arrays of a theme JSON
    /// object; every other key is ignored (P2-427). Each style and value
    /// carries `span`, the theme statement's.
    // TODO(srs): P2-427 names the JSON keys "from memory". Reading taken:
    // the keys of Structurizr for Java's `ElementStyle`/`RelationshipStyle`
    // JSON (as in structurizr/themes): `tag`, `background`, `color`,
    // `stroke`, `strokeWidth`, `shape`, `icon`, `width`, `height`,
    // `fontSize`, `border`, `opacity`, `metadata`, `description`;
    // `thickness`, `color`, `dashed`, `style`, `routing`, `jump`,
    // `fontSize`, `width`, `position`, `opacity`.
    fn read(value: &json::Value, theme: usize, span: Span) -> Result<ThemeStyles, String> {
        let json::Value::Object(members) = value else {
            return Err("the theme is not a JSON object".into());
        };
        let mut out = ThemeStyles {
            elements: Vec::new(),
            relationships: Vec::new(),
        };
        for (key, value) in members {
            let is_elements = key == "elements";
            if !is_elements && key != "relationships" {
                continue;
            }
            let json::Value::Array(items) = value else {
                return Err(format!("`{key}` is not an array"));
            };
            for (i, item) in items.iter().enumerate() {
                let json::Value::Object(fields) = item else {
                    return Err(format!("`{key}[{i}]` is not an object"));
                };
                let mut reader = StyleReader {
                    fields,
                    span,
                    place: format!("{key}[{i}]"),
                };
                if is_elements {
                    out.elements.push(reader.element(theme)?);
                } else {
                    out.relationships.push(reader.relationship(theme)?);
                }
            }
        }
        Ok(out)
    }
}

/// One style object of a theme file.
struct StyleReader<'v> {
    fields: &'v [(String, json::Value)],
    span: Span,
    /// `elements[2]`, for error text.
    place: String,
}

impl StyleReader<'_> {
    fn get(&self, key: &str) -> Option<&json::Value> {
        // DECISION: a repeated key takes its last value, as Jackson does.
        self.fields
            .iter()
            .rev()
            .find(|(k, _)| k == key)
            .map(|(_, v)| v)
    }

    fn bad(&self, key: &str, what: &str) -> String {
        format!("`{key}` of `{}` is not {what}", self.place)
    }

    fn spanned<T>(&self, value: T) -> Spanned<T> {
        Spanned {
            value,
            span: self.span,
        }
    }

    fn string(&self, key: &str) -> Result<Option<&str>, String> {
        match self.get(key) {
            None | Some(json::Value::Null) => Ok(None),
            Some(json::Value::String(s)) => Ok(Some(s)),
            Some(_) => Err(self.bad(key, "a string")),
        }
    }

    fn tag(&self) -> Result<Spanned<String>, String> {
        match self.string("tag")? {
            Some(tag) => Ok(self.spanned(tag.to_owned())),
            None => Err(format!("`{}` has no `tag`", self.place)),
        }
    }

    fn colour(&self, key: &str) -> Result<Option<Spanned<super::Colour>>, String> {
        let Some(text) = self.string(key)? else {
            return Ok(None);
        };
        match parse_colour(text) {
            Some(c) => Ok(Some(self.spanned(c))),
            None => Err(self.bad(key, "a `#rrggbb` colour or a CSS colour name")),
        }
    }

    fn integer(&self, key: &str) -> Result<Option<i64>, String> {
        match self.get(key) {
            None | Some(json::Value::Null) => Ok(None),
            Some(json::Value::Number(n)) => match json::integer(n) {
                Some(v) => Ok(Some(v)),
                None => Err(self.bad(key, "a whole number")),
            },
            Some(_) => Err(self.bad(key, "a number")),
        }
    }

    fn positive(&self, key: &str) -> Result<Option<Spanned<u32>>, String> {
        match self.integer(key)? {
            None => Ok(None),
            Some(v) => match u32::try_from(v) {
                Ok(v) if v > 0 => Ok(Some(self.spanned(v))),
                _ => Err(self.bad(key, "a number above 0")),
            },
        }
    }

    /// As the DSL: out-of-range numbers are clamped (B styles-themes 13).
    fn clamped(&self, key: &str, min: u8, max: u8) -> Result<Option<Spanned<u8>>, String> {
        Ok(self.integer(key)?.map(|v| {
            let v = v.clamp(i64::from(min), i64::from(max));
            self.spanned(u8::try_from(v).unwrap_or(max))
        }))
    }

    fn boolean(&self, key: &str) -> Result<Option<Spanned<bool>>, String> {
        match self.get(key) {
            None | Some(json::Value::Null) => Ok(None),
            Some(json::Value::Bool(b)) => Ok(Some(self.spanned(*b))),
            Some(_) => Err(self.bad(key, "`true` or `false`")),
        }
    }

    fn border(&self, key: &str) -> Result<Option<Spanned<Border>>, String> {
        let Some(text) = self.string(key)? else {
            return Ok(None);
        };
        match super::parse_border(text) {
            Some(b) => Ok(Some(self.spanned(b))),
            None => Err(self.bad(key, "`Solid`, `Dashed` or `Dotted`")),
        }
    }

    fn element(&mut self, theme: usize) -> Result<ElementStyle, String> {
        let shape = match self.string("shape")? {
            None => None,
            Some(text) => match Shape::ALL
                .into_iter()
                .find(|s| s.keyword().eq_ignore_ascii_case(text))
            {
                Some(s) => Some(self.spanned(s)),
                None => return Err(self.bad("shape", "a Structurizr shape")),
            },
        };
        let size = |key: &str| -> Result<Option<Spanned<i32>>, String> {
            Ok(match self.positive(key)? {
                Some(v) => Some(Spanned {
                    value: i32::try_from(v.value).map_err(|_| self.bad(key, "a size"))?,
                    span: v.span,
                }),
                None => None,
            })
        };
        Ok(ElementStyle {
            tag: self.tag()?,
            background: self.colour("background")?,
            color: self.colour("color")?.or(self.colour("colour")?),
            shape,
            span: self.span,
            stroke: self.colour("stroke")?,
            stroke_width: self.clamped("strokeWidth", 1, 10)?,
            width: size("width")?,
            height: size("height")?,
            font_size: self.positive("fontSize")?,
            border: self.border("border")?,
            opacity: self.clamped("opacity", 0, 100)?,
            metadata: self.boolean("metadata")?,
            description: self.boolean("description")?,
            icon: self.string("icon")?.map(|i| self.spanned(i.to_owned())),
            properties: Vec::new(),
            mode: None,
            theme: Some(theme),
        })
    }

    fn relationship(&mut self, theme: usize) -> Result<RelationshipStyle, String> {
        // DECISION: when a theme sets both, `style` beats the older
        // `dashed` (Structurizr for Java maps `dashed` onto `style`).
        let style = match self.border("style")? {
            Some(s) => Some(s),
            None => self.boolean("dashed")?.map(|d| {
                self.spanned(if d.value {
                    Border::Dashed
                } else {
                    Border::Solid
                })
            }),
        };
        let routing = match self.string("routing")? {
            None => None,
            Some(text) => match super::parse_routing(text) {
                Some(r) => Some(self.spanned::<Routing>(r)),
                None => return Err(self.bad("routing", "`Direct`, `Orthogonal` or `Curved`")),
            },
        };
        Ok(RelationshipStyle {
            tag: self.tag()?,
            thickness: self.positive("thickness")?,
            color: self.colour("color")?.or(self.colour("colour")?),
            style,
            routing,
            jump: self.boolean("jump")?,
            font_size: self.positive("fontSize")?,
            width: self.positive("width")?,
            position: self.clamped("position", 0, 100)?,
            opacity: self.clamped("opacity", 0, 100)?,
            properties: Vec::new(),
            mode: None,
            theme: Some(theme),
            span: self.span,
        })
    }
}

/// A std-only JSON reader (RFC 8259), enough for theme files. Objects
/// keep their members in file order (determinism, CORE-25).
mod json {
    /// A JSON value; a number keeps its text.
    #[derive(Debug, Clone, PartialEq)]
    pub(super) enum Value {
        Null,
        Bool(bool),
        Number(String),
        String(String),
        Array(Vec<Value>),
        Object(Vec<(String, Value)>),
    }

    /// The whole value of a number's text, when it has no fraction.
    pub(super) fn integer(text: &str) -> Option<i64> {
        if let Ok(v) = text.parse::<i64>() {
            return Some(v);
        }
        // `2.0`, `1e2`: whole numbers written as reals.
        let real: f64 = text.parse().ok()?;
        #[allow(clippy::cast_possible_truncation)]
        let whole = real as i64;
        #[allow(clippy::cast_precision_loss)]
        let exact = (whole as f64 - real).abs() < f64::EPSILON;
        exact.then_some(whole)
    }

    /// Parses one JSON document.
    ///
    /// # Errors
    ///
    /// What is wrong and the line it is on.
    pub(super) fn parse(text: &str) -> Result<Value, String> {
        let mut reader = Reader {
            bytes: text.trim_start_matches('\u{feff}').as_bytes(),
            at: 0,
            depth: 0,
        };
        let value = reader.value()?;
        reader.space();
        if reader.at < reader.bytes.len() {
            return Err(reader.fail("text after the JSON value"));
        }
        Ok(value)
    }

    struct Reader<'b> {
        bytes: &'b [u8],
        at: usize,
        depth: u32,
    }

    impl Reader<'_> {
        fn fail(&self, what: &str) -> String {
            let line = self.bytes[..self.at.min(self.bytes.len())]
                .split(|&b| b == b'\n')
                .count();
            format!("{what} on line {line}")
        }

        fn space(&mut self) {
            while self
                .bytes
                .get(self.at)
                .is_some_and(|b| matches!(b, b' ' | b'\t' | b'\n' | b'\r'))
            {
                self.at += 1;
            }
        }

        fn peek(&self) -> Option<u8> {
            self.bytes.get(self.at).copied()
        }

        fn expect(&mut self, b: u8) -> Result<(), String> {
            self.space();
            if self.peek() == Some(b) {
                self.at += 1;
                Ok(())
            } else {
                Err(self.fail(&format!("`{}` expected", b as char)))
            }
        }

        fn value(&mut self) -> Result<Value, String> {
            self.space();
            match self.peek() {
                None => Err(self.fail("unexpected end of the file")),
                Some(b'{') => self.nested(Self::object),
                Some(b'[') => self.nested(Self::array),
                Some(b'"') => self.string().map(Value::String),
                Some(b't') => self.word("true", Value::Bool(true)),
                Some(b'f') => self.word("false", Value::Bool(false)),
                Some(b'n') => self.word("null", Value::Null),
                Some(b'-' | b'0'..=b'9') => self.number(),
                Some(_) => Err(self.fail("a JSON value expected")),
            }
        }

        fn nested(&mut self, read: fn(&mut Self) -> Result<Value, String>) -> Result<Value, String> {
            // DECISION: nesting deeper than 128 is refused, so a hostile
            // file cannot overflow the stack.
            if self.depth >= 128 {
                return Err(self.fail("nesting too deep"));
            }
            self.depth += 1;
            let value = read(self);
            self.depth -= 1;
            value
        }

        fn word(&mut self, word: &str, value: Value) -> Result<Value, String> {
            if self.bytes[self.at..].starts_with(word.as_bytes()) {
                self.at += word.len();
                Ok(value)
            } else {
                Err(self.fail("a JSON value expected"))
            }
        }

        fn object(&mut self) -> Result<Value, String> {
            self.at += 1;
            let mut members = Vec::new();
            self.space();
            if self.peek() == Some(b'}') {
                self.at += 1;
                return Ok(Value::Object(members));
            }
            loop {
                self.space();
                if self.peek() != Some(b'"') {
                    return Err(self.fail("a quoted key expected"));
                }
                let key = self.string()?;
                self.expect(b':')?;
                let value = self.value()?;
                members.push((key, value));
                self.space();
                match self.peek() {
                    Some(b',') => self.at += 1,
                    Some(b'}') => {
                        self.at += 1;
                        return Ok(Value::Object(members));
                    }
                    _ => return Err(self.fail("`,` or `}` expected (unterminated object)")),
                }
            }
        }

        fn array(&mut self) -> Result<Value, String> {
            self.at += 1;
            let mut items = Vec::new();
            self.space();
            if self.peek() == Some(b']') {
                self.at += 1;
                return Ok(Value::Array(items));
            }
            loop {
                items.push(self.value()?);
                self.space();
                match self.peek() {
                    Some(b',') => self.at += 1,
                    Some(b']') => {
                        self.at += 1;
                        return Ok(Value::Array(items));
                    }
                    _ => return Err(self.fail("`,` or `]` expected (unterminated array)")),
                }
            }
        }

        fn number(&mut self) -> Result<Value, String> {
            let start = self.at;
            while self
                .peek()
                .is_some_and(|b| matches!(b, b'-' | b'+' | b'.' | b'e' | b'E' | b'0'..=b'9'))
            {
                self.at += 1;
            }
            let text = std::str::from_utf8(&self.bytes[start..self.at]).unwrap_or("");
            if text.parse::<f64>().is_err() {
                self.at = start;
                return Err(self.fail("a malformed number"));
            }
            Ok(Value::Number(text.to_owned()))
        }

        fn hex4(&mut self) -> Result<u32, String> {
            let digits = self
                .bytes
                .get(self.at..self.at + 4)
                .and_then(|d| std::str::from_utf8(d).ok())
                .and_then(|d| u32::from_str_radix(d, 16).ok());
            match digits {
                Some(v) => {
                    self.at += 4;
                    Ok(v)
                }
                None => Err(self.fail("a `\\u` escape needs four hex digits")),
            }
        }

        fn string(&mut self) -> Result<String, String> {
            self.at += 1;
            let mut out: Vec<u8> = Vec::new();
            loop {
                let Some(b) = self.peek() else {
                    return Err(self.fail("unterminated string"));
                };
                self.at += 1;
                match b {
                    b'"' => break,
                    b'\\' => {
                        let Some(e) = self.peek() else {
                            return Err(self.fail("unterminated string"));
                        };
                        self.at += 1;
                        let c = match e {
                            b'"' => '"',
                            b'\\' => '\\',
                            b'/' => '/',
                            b'b' => '\u{8}',
                            b'f' => '\u{c}',
                            b'n' => '\n',
                            b'r' => '\r',
                            b't' => '\t',
                            b'u' => self.unicode()?,
                            _ => return Err(self.fail("an unknown escape in a string")),
                        };
                        let mut buf = [0; 4];
                        out.extend_from_slice(c.encode_utf8(&mut buf).as_bytes());
                    }
                    0..0x20 => return Err(self.fail("a control character in a string")),
                    _ => out.push(b),
                }
            }
            String::from_utf8(out).map_err(|_| self.fail("a string that is not UTF-8"))
        }

        /// The character of a `\uXXXX` escape (surrogate pairs joined).
        fn unicode(&mut self) -> Result<char, String> {
            let first = self.hex4()?;
            let code = if (0xd800..0xdc00).contains(&first) {
                if !self.bytes[self.at..].starts_with(b"\\u") {
                    return Err(self.fail("a lone surrogate in a `\\u` escape"));
                }
                self.at += 2;
                let second = self.hex4()?;
                if !(0xdc00..0xe000).contains(&second) {
                    return Err(self.fail("a lone surrogate in a `\\u` escape"));
                }
                0x10000 + ((first - 0xd800) << 10) + (second - 0xdc00)
            } else {
                first
            };
            char::from_u32(code).ok_or_else(|| self.fail("a lone surrogate in a `\\u` escape"))
        }
    }

    #[cfg(test)]
    mod tests {
        use super::{Value, integer, parse};

        #[test]
        fn reads_every_kind_in_order() {
            let v = parse(r#" {"b": [1, -2.5e1, true, false, null], "a": "x\u00e9\n\ud83d\ude00"} "#)
                .unwrap();
            let Value::Object(members) = v else { panic!() };
            assert_eq!(members[0].0, "b");
            assert_eq!(members[1], ("a".into(), Value::String("xé\n😀".into())));
            let Value::Array(items) = &members[0].1 else { panic!() };
            assert_eq!(items.len(), 5);
            assert_eq!(items[1], Value::Number("-2.5e1".into()));
        }

        #[test]
        fn refuses_malformed_text() {
            for bad in ["", "{", "[1,]", "{\"a\" 1}", "\"abc", "tru", "{} x", "01x", "\"\\q\""] {
                assert!(parse(bad).is_err(), "{bad}");
            }
            let deep = "[".repeat(200);
            assert!(parse(&deep).unwrap_err().contains("deep"));
        }

        #[test]
        fn whole_numbers() {
            assert_eq!(integer("12"), Some(12));
            assert_eq!(integer("2.0"), Some(2));
            assert_eq!(integer("2.5"), None);
        }
    }
}

//! `parse_with` and `reparse`: the root file, the files it reads through a
//! `Loader` (`!include`, `workspace extends`), and the environment values
//! `${NAME}` used, recorded so `reparse` rebuilds the model with no I/O.

// DECISION: private submodule of `model` (plan Global Constraints); it
// reaches the builder through `super`.
// DECISION: every file is read before the model is built: a reading pass
// walks each tree in document order and reads what its `!include`s (and
// the root's `workspace extends`) name, numbering files from 1 in first-
// read order (P2-135); the builder then walks the trees, inlining an
// included file's statements where its `!include` stands (P2-5). An
// include path takes the `!const`/`!var` values defined above it in the
// same file and the environment, as the walk would.

use std::collections::BTreeMap;
use std::io;
use std::path::{Path, PathBuf};

use super::{Builder, Loader, ModelError, ModelErrorKind, Parsed, Span, net, theme};
use crate::text::{self, FileId, Statement, SyntaxTree, TokenKind};

/// What a parse read from outside its trees, kept in `Parsed` for
/// `reparse`.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Default)]
pub(super) struct Replay {
    /// Every environment variable `${NAME}` looked up, and its value.
    pub(super) env: BTreeMap<String, Option<String>>,
    /// Per file, per `!include` statement in document order, the files it
    /// inlines.
    pub(super) includes: Vec<Vec<Vec<FileId>>>,
    /// The `workspace extends` base of each file that has one (P2-16).
    pub(super) bases: BTreeMap<FileId, FileId>,
    /// Every theme and icon file read after the DSL files, by the path
    /// asked for, with its bytes (P2-13, P2-409).
    pub(super) files: BTreeMap<PathBuf, Vec<u8>>,
    /// Every path the theme and icon reads canonicalized, and the result.
    pub(super) canonical: BTreeMap<PathBuf, PathBuf>,
}

/// The `!include` statements of `tree` in document order, nested blocks
/// included.
pub(super) fn include_statements(tree: &SyntaxTree) -> Vec<&Statement> {
    fn walk<'t>(tree: &'t SyntaxTree, statements: &'t [Statement], out: &mut Vec<&'t Statement>) {
        for s in statements {
            if first_word(tree, s).is_some_and(|w| w.eq_ignore_ascii_case("!include")) {
                out.push(s);
            }
            if let Some(block) = &s.block {
                walk(tree, &block.statements, out);
            }
        }
    }
    let mut out = Vec::new();
    walk(tree, tree.statements(), &mut out);
    out
}

/// The first significant token text of `statement`'s head.
fn first_word<'t>(tree: &'t SyntaxTree, statement: &Statement) -> Option<&'t str> {
    significant(tree, statement).first().map(|&i| tree.token_text(i))
}

/// The significant token indices of `statement`'s head.
fn significant(tree: &SyntaxTree, statement: &Statement) -> Vec<usize> {
    statement
        .head
        .clone()
        .filter(|&i| !tree.tokens()[i].kind.is_trivia())
        .collect()
}

/// The reading pass of one `parse_with` call.
struct Reader<'l> {
    loader: &'l mut dyn Loader,
    trees: Vec<SyntaxTree>,
    paths: Vec<PathBuf>,
    /// The resolved path of each file read, for cycles and reuse.
    resolved: Vec<PathBuf>,
    includes: Vec<Vec<Vec<FileId>>>,
    /// The files being read, outermost first (P2-137).
    chain: Vec<PathBuf>,
    /// The environment values looked up for include paths.
    env: BTreeMap<String, Option<String>>,
    bases: BTreeMap<FileId, FileId>,
    /// `!const`/`!var` values defined so far, in document order across
    /// files (P2-150, P2-5).
    // DECISION: one set for the whole parse, an included file read where
    // its `!include` stands, as the builder sees them; so a `!const`
    // before `workspace` fills an include path in an included file
    // (corpus big-bank-plc/internet-banking-system.dsl).
    vars: BTreeMap<String, String>,
}

impl Reader<'_> {
    fn error(&self, kind: ModelErrorKind, span: Span) -> ModelError {
        let source = self
            .trees
            .get(span.file.0 as usize)
            .map_or("", SyntaxTree::source);
        ModelError {
            kind,
            span,
            position: text::position(source, span.start),
        }
    }

    /// Reads the `workspace extends` base of file `file`, if it names
    /// one (P2-16), then what its `!include`s name, recursively.
    fn read_includes(&mut self, file: FileId) -> Result<(), ModelError> {
        let tree = self.trees[file.0 as usize].clone();
        if let Some(statement) = tree.statements().first() {
            let tokens = significant(&tree, statement);
            let extends = tokens.len() >= 2
                && tree.token_text(tokens[0]).eq_ignore_ascii_case("workspace")
                && tree.token_text(tokens[1]).eq_ignore_ascii_case("extends");
            if extends {
                let base = self.read_base(&tree, &tokens)?;
                self.bases.insert(file, base);
            }
        }
        let mut vars = std::mem::take(&mut self.vars);
        let mut found = Vec::new();
        let result = self.scan(&tree, tree.statements(), &mut vars, &mut found);
        self.vars = vars;
        result?;
        self.includes[file.0 as usize] = found;
        Ok(())
    }

    /// Document-order walk of `statements`: `!const`/`!var` values and
    /// each `!include`'s files.
    fn scan(
        &mut self,
        tree: &SyntaxTree,
        statements: &[Statement],
        vars: &mut BTreeMap<String, String>,
        found: &mut Vec<Vec<FileId>>,
    ) -> Result<(), ModelError> {
        for s in statements {
            let tokens = significant(tree, s);
            let word = tokens.first().map(|&i| tree.token_text(i).to_ascii_lowercase());
            match word.as_deref() {
                Some("!const" | "!var") if tokens.len() == 3 => {
                    let value = self.value(tree, tokens[2], vars);
                    let raw = tree.token_text(tokens[1]);
                    // A quoted name is the same name (P2-150).
                    let name = match tree.tokens()[tokens[1]].kind {
                        TokenKind::String => text::unquote(raw),
                        _ => raw.to_owned(),
                    };
                    vars.insert(name, value);
                }
                Some("!include") => {
                    let Some(&arg) = tokens.get(1) else {
                        return Err(self.error(
                            ModelErrorKind::MissingArgument {
                                construct: "!include".into(),
                                argument: "path",
                            },
                            tree.span(tokens[0]),
                        ));
                    };
                    let written = self.value(tree, arg, vars);
                    // The included files see, and add to, the same set.
                    self.vars = std::mem::take(vars);
                    let files = self.include(&written, tree.span(arg));
                    *vars = std::mem::take(&mut self.vars);
                    found.push(files?);
                }
                _ => {}
            }
            if let Some(block) = &s.block {
                self.scan(tree, &block.statements, vars, found)?;
            }
        }
        Ok(())
    }

    /// The base `workspace extends <path>` names: a local DSL file, read
    /// like an included one (B dsl-breadth 6: a URL is refused, a JSON
    /// base is phase 3).
    fn read_base(&mut self, tree: &SyntaxTree, tokens: &[usize]) -> Result<FileId, ModelError> {
        let construct = "workspace extends";
        let Some(&arg) = tokens.get(2) else {
            return Err(self.error(
                ModelErrorKind::MissingArgument {
                    construct: construct.into(),
                    argument: "base workspace",
                },
                tree.span(tokens[1]),
            ));
        };
        let span = tree.span(arg);
        let vars = self.vars.clone();
        let written = self.value(tree, arg, &vars);
        let lower = written.to_ascii_lowercase();
        if lower.starts_with("http://") {
            return Err(self.error(ModelErrorKind::InsecureUrl { url: written }, span));
        }
        let json = Path::new(written.split(['?', '#']).next().unwrap_or_default())
            .extension()
            .is_some_and(|e| e.eq_ignore_ascii_case("json"));
        // P3-603: an HTTPS base (or any base of a fetched file) is fetched
        // when the network is on; `UrlNotSupported` when it is off (P3-601).
        // TODO(srs): P3-603/P3-419, a JSON base (URL or file) waits for
        // JSON input (row exporters): still `LaterPhase` below.
        if !json && let Some(url) = self.url_of(&written, span) {
            return self.fetch_file(&url, &written, span);
        }
        if json {
            // TODO(srs): P2-16, JSON bases. Reading taken (B dsl-breadth 6):
            // phase 3.
            return Err(self.error(
                ModelErrorKind::LaterPhase {
                    construct: format!("{construct} {written}"),
                    phase: 3,
                },
                span,
            ));
        }
        let resolved = self.resolve(&written, span, construct)?;
        self.read_file(&resolved, &written, span)
    }

    /// Token `i` as a string value, `${NAME}` substituted.
    fn value(&mut self, tree: &SyntaxTree, i: usize, vars: &BTreeMap<String, String>) -> String {
        let raw = tree.token_text(i);
        let raw = match tree.tokens()[i].kind {
            TokenKind::String => text::unquote(raw),
            _ => raw.to_owned(),
        };
        let mut out = String::new();
        let mut rest = raw.as_str();
        while let Some(at) = rest.find("${") {
            out.push_str(&rest[..at]);
            let after = &rest[at + 2..];
            let Some(end) = after.find('}') else {
                out.push_str("${");
                rest = after;
                continue;
            };
            let name = &after[..end];
            let value = vars.get(name).cloned().or_else(|| {
                let v = self.loader.env(name);
                self.env.insert(name.to_owned(), v.clone());
                v
            });
            if let Some(v) = value {
                out.push_str(&v);
            } else {
                out.push_str("${");
                out.push_str(name);
                out.push('}');
            }
            rest = &after[end + 1..];
        }
        out.push_str(rest);
        out
    }

    /// The files `!include <written>` at `span` names, each read (or
    /// reused when read before) and its own includes read.
    fn include(&mut self, written: &str, span: Span) -> Result<Vec<FileId>, ModelError> {
        let lower = written.to_ascii_lowercase();
        if lower.starts_with("http://") {
            return Err(self.error(ModelErrorKind::InsecureUrl { url: written.into() }, span));
        }
        // P3-603, P3-606: an HTTPS include, or a relative one inside a
        // fetched file, is fetched; with the network off it stays
        // `UrlNotSupported` (P3-601, P2-138).
        if let Some(url) = self.url_of(written, span) {
            return self.fetch_file(&url, written, span).map(|file| vec![file]);
        }
        let resolved = self.resolve(written, span, "!include")?;
        let mut files = Vec::new();
        match self.loader.read_dir(&resolved) {
            Ok(_) => {
                // TODO(srs): P2-133, order, recursion and extensions.
                // DECISION: B dsl-breadth 5, checked against Structurizr for
                // Java: every regular file below, recursively, by relative
                // path compared byte by byte, names starting with `.`
                // skipped, any extension.
                let mut entries = Vec::new();
                self.list(&resolved, &resolved, &mut entries, span)?;
                entries.sort();
                for (_, path) in entries {
                    let shown = path.display().to_string();
                    files.push(self.read_file(&path, &shown, span)?);
                }
            }
            Err(_) => files.push(self.read_file(&resolved, written, span)?),
        }
        Ok(files)
    }

    /// The URL `written` names: itself when it is an `https://` URL, else
    /// resolved against the URL of the fetched file `span` is in (P3-606);
    /// `None` for a local path in a local file.
    fn url_of(&self, written: &str, span: Span) -> Option<String> {
        if net::is_https(written) {
            return Some(written.to_owned());
        }
        let base = self.paths.get(span.file.0 as usize)?.to_str()?;
        net::is_https(base).then(|| net::join_url(base, written))
    }

    /// The file at `url`, fetched through `Loader::fetch` and parsed as the
    /// next `FileId` (named by its URL), or the id it was read as before.
    /// Network off is `UrlNotSupported` naming `written` (P3-601); a URL
    /// that is not HTTPS is `InsecureUrl`; any other failure `FetchFailed`
    /// (P3-611).
    fn fetch_file(&mut self, url: &str, written: &str, span: Span) -> Result<FileId, ModelError> {
        let resolved = PathBuf::from(url);
        if self.chain.contains(&resolved) {
            // P3-607: the cycle names the URL.
            return Err(self.error(ModelErrorKind::IncludeCycle { path: url.into() }, span));
        }
        if let Some(i) = self.resolved.iter().position(|r| *r == resolved) {
            return Ok(FileId(super::index_u32(i)));
        }
        let bytes = match self.loader.fetch(url) {
            Ok(bytes) => bytes,
            Err(e) if e.kind() == io::ErrorKind::Unsupported => {
                return Err(self.error(ModelErrorKind::UrlNotSupported { url: written.into() }, span));
            }
            Err(e) if e.kind() == io::ErrorKind::InvalidInput => {
                return Err(self.error(ModelErrorKind::InsecureUrl { url: url.into() }, span));
            }
            Err(e) => {
                return Err(self.error(
                    ModelErrorKind::FetchFailed {
                        url: url.into(),
                        reason: e.to_string(),
                    },
                    span,
                ));
            }
        };
        self.add_file(bytes, resolved, url, span)
    }

    /// `written`, relative to the folder of the file `span` is in: it
    /// exists and lies inside that folder after `..` and symlinks resolve
    /// (P2-134).
    fn resolve(&mut self, written: &str, span: Span, construct: &str) -> Result<PathBuf, ModelError> {
        let Some(path) = self.paths.get(span.file.0 as usize) else {
            return Err(self.error(
                ModelErrorKind::NeedsFile {
                    construct: construct.into(),
                },
                span,
            ));
        };
        let folder = path.parent().map_or_else(PathBuf::new, Path::to_path_buf);
        let resolved = self.loader.canonicalize(&folder.join(written)).map_err(|e| {
            self.error(
                ModelErrorKind::FileRead {
                    path: written.into(),
                    error: e.to_string(),
                },
                span,
            )
        })?;
        let root = self.loader.canonicalize(&folder).unwrap_or(folder);
        if !resolved.starts_with(&root) {
            return Err(self.error(ModelErrorKind::PathOutsideRoot { path: written.into() }, span));
        }
        Ok(resolved)
    }

    /// Every file below `dir`, with its path relative to `top`.
    fn list(
        &mut self,
        top: &Path,
        dir: &Path,
        out: &mut Vec<(Vec<u8>, PathBuf)>,
        span: Span,
    ) -> Result<(), ModelError> {
        let entries = self.loader.read_dir(dir).map_err(|e| {
            self.error(
                ModelErrorKind::FileRead {
                    path: dir.display().to_string(),
                    error: e.to_string(),
                },
                span,
            )
        })?;
        for entry in entries {
            let entry = if entry.is_absolute() { entry } else { dir.join(entry) };
            let hidden = entry
                .file_name()
                .is_some_and(|n| n.to_string_lossy().starts_with('.'));
            if hidden {
                continue;
            }
            if self.loader.read_dir(&entry).is_ok() {
                self.list(top, &entry, out, span)?;
            } else {
                let relative = entry.strip_prefix(top).unwrap_or(&entry);
                let key = relative.to_string_lossy().as_bytes().to_vec();
                out.push((key, entry));
            }
        }
        Ok(())
    }

    /// The file at `path` (written as `written`), read and parsed as the
    /// next `FileId`, or the id it was read as before.
    fn read_file(&mut self, path: &Path, written: &str, span: Span) -> Result<FileId, ModelError> {
        let resolved = self.loader.canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
        if self.chain.contains(&resolved) {
            return Err(self.error(ModelErrorKind::IncludeCycle { path: written.into() }, span));
        }
        if let Some(i) = self.resolved.iter().position(|r| *r == resolved) {
            return Ok(FileId(super::index_u32(i)));
        }
        let read_error = |e: io::Error| ModelErrorKind::FileRead {
            path: written.into(),
            error: e.to_string(),
        };
        let bytes = self
            .loader
            .read(path)
            .map_err(|e| self.error(read_error(e), span))?;
        self.add_file(bytes, resolved, written, span)
    }

    /// `bytes`, the file at `resolved` (written as `written`), parsed as
    /// the next `FileId` with its own includes read.
    fn add_file(&mut self, bytes: Vec<u8>, resolved: PathBuf, written: &str, span: Span) -> Result<FileId, ModelError> {
        let source = String::from_utf8(bytes).map_err(|_| {
            self.error(
                ModelErrorKind::FileFormat {
                    path: written.into(),
                    reason: "it is not UTF-8 text".into(),
                },
                span,
            )
        })?;
        let file = FileId(super::index_u32(self.trees.len()));
        let tree = text::parse(file, &source)?;
        self.trees.push(tree);
        self.paths.push(resolved.clone());
        self.resolved.push(resolved.clone());
        self.includes.push(Vec::new());
        self.chain.push(resolved);
        let result = self.read_includes(file);
        self.chain.pop();
        result.map(|()| file)
    }
}

/// See `model::parse_with`.
pub(super) fn parse_with(
    source: &str,
    path: Option<&Path>,
    loader: &mut dyn Loader,
) -> Result<Parsed, ModelError> {
    let root = text::parse(FileId::ROOT, source)?;
    let paths: Vec<PathBuf> = path.map(Path::to_path_buf).into_iter().collect();
    let resolved = path.map(|p| loader.canonicalize(p).unwrap_or_else(|_| p.to_path_buf()));
    let mut reader = Reader {
        loader,
        trees: vec![root],
        paths,
        resolved: resolved.clone().into_iter().collect(),
        includes: vec![Vec::new()],
        chain: resolved.into_iter().collect(),
        env: BTreeMap::new(),
        bases: BTreeMap::new(),
        vars: BTreeMap::new(),
    };
    if reader.resolved.is_empty() {
        // Keep `resolved` indexed by `FileId` without a root path.
        reader.resolved.push(PathBuf::new());
    }
    reader.read_includes(FileId::ROOT)?;
    let Reader {
        loader,
        trees,
        paths,
        includes,
        env,
        bases,
        ..
    } = reader;
    let replay = Replay {
        env,
        includes,
        bases,
        ..Replay::default()
    };
    build(trees, paths, replay, loader)
}

/// Builds the model over `trees` (already read) and packs the result.
fn build(
    trees: Vec<SyntaxTree>,
    paths: Vec<PathBuf>,
    mut replay: Replay,
    loader: &mut dyn Loader,
) -> Result<Parsed, ModelError> {
    let (model, env) = {
        let mut builder = Builder::new(&trees, loader);
        builder.paths.clone_from(&paths);
        for (f, tree) in trees.iter().enumerate() {
            let statements = include_statements(tree);
            let files = replay.includes.get(f).cloned().unwrap_or_default();
            for (s, files) in statements.into_iter().zip(files) {
                builder.included.insert((tree.file(), s.span.start), files);
            }
        }
        builder.bases.clone_from(&replay.bases);
        builder.file()?;
        (builder.model, builder.env_used.into_inner())
    };
    let mut model = model;
    // Themes and icons are read after every DSL file, so a loader that
    // numbers its reads sees the DSL files first, in `FileId` order
    // (P2-136); what they read is taped for `reparse`.
    let mut tape = Tape {
        loader,
        files: BTreeMap::new(),
        canonical: BTreeMap::new(),
    };
    let files = theme::Files {
        path: paths.first().map(PathBuf::as_path),
        loader: &mut tape,
    };
    theme::apply(&mut model, &trees, Some(files))?;
    let Tape {
        files, canonical, ..
    } = tape;
    replay.files.extend(files);
    replay.canonical.extend(canonical);
    replay.env.extend(env);
    Ok(Parsed {
        trees,
        model,
        paths,
        replay,
    })
}

/// The loader themes and icons read through: `loader`, with every read
/// and canonical path kept for `reparse`.
struct Tape<'l> {
    loader: &'l mut dyn Loader,
    files: BTreeMap<PathBuf, Vec<u8>>,
    canonical: BTreeMap<PathBuf, PathBuf>,
}

impl Loader for Tape<'_> {
    fn read(&mut self, path: &Path) -> io::Result<Vec<u8>> {
        let bytes = self.loader.read(path)?;
        self.files.insert(path.to_path_buf(), bytes.clone());
        Ok(bytes)
    }

    fn read_dir(&mut self, path: &Path) -> io::Result<Vec<PathBuf>> {
        self.loader.read_dir(path)
    }

    fn canonicalize(&mut self, path: &Path) -> io::Result<PathBuf> {
        let resolved = self.loader.canonicalize(path)?;
        self.canonical.insert(path.to_path_buf(), resolved.clone());
        Ok(resolved)
    }

    fn env(&self, name: &str) -> Option<String> {
        self.loader.env(name)
    }
}

/// The `Loader` `reparse` builds with: the recorded environment, the
/// theme and icon files the first parse read, and no other file access
/// (every DSL file is already a tree).
// DECISION: a theme or icon the edited text names for the first time is
// a `FileRead` error here; edits never add one (EDIT-21), and the editor
// re-parses from disk after its write anyway.
struct Recorded<'r> {
    env: &'r BTreeMap<String, Option<String>>,
    files: &'r BTreeMap<PathBuf, Vec<u8>>,
    canonical: &'r BTreeMap<PathBuf, PathBuf>,
}

impl Loader for Recorded<'_> {
    fn read(&mut self, path: &Path) -> io::Result<Vec<u8>> {
        self.files
            .get(path)
            .cloned()
            .ok_or_else(|| io::Error::new(io::ErrorKind::PermissionDenied, "reparse reads no file"))
    }

    fn read_dir(&mut self, _path: &Path) -> io::Result<Vec<PathBuf>> {
        Err(io::Error::new(io::ErrorKind::PermissionDenied, "reparse reads no file"))
    }

    fn canonicalize(&mut self, path: &Path) -> io::Result<PathBuf> {
        // Paths were checked by the parse being replayed.
        Ok(self.canonical.get(path).cloned().unwrap_or_else(|| path.to_path_buf()))
    }

    fn env(&self, name: &str) -> Option<String> {
        self.env.get(name).cloned().flatten()
    }
}

/// See `model::reparse`.
// DECISION: an edit never adds or removes an `!include` (EDIT-21), so the
// included files of each `!include`, matched by its place among the
// file's `!include`s, carry over unchanged.
pub(super) fn reparse(parsed: &Parsed, file: FileId, source: &str) -> Result<Parsed, ModelError> {
    let mut trees = parsed.trees.clone();
    let tree = text::parse(file, source)?;
    match trees.get_mut(file.0 as usize) {
        Some(slot) => *slot = tree,
        None => trees.push(tree),
    }
    let mut loader = Recorded {
        env: &parsed.replay.env,
        files: &parsed.replay.files,
        canonical: &parsed.replay.canonical,
    };
    build(trees, parsed.paths.clone(), parsed.replay.clone(), &mut loader)
}

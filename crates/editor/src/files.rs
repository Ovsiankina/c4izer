//! Open dialog, recent files, per-user UI state, directory watching,
//! atomic writes, and the on-disk check before each write (EDIT-1, 7, 13,
//! 22..24, 27).
//!
//! Depends on: `worker` (`ViewRef`, `ExportFormat`, `Wake`), `camera`
//! (`Camera`), `notify`, `rfd`.

// Shape: every function here does I/O and nothing else; decisions
// (reload or ignore, re-apply or write) are `state`'s. The watcher turns
// `notify` events on the file's directory into `FileEvent`s for the one
// file, read and hashed, so `state` compares hashes and never touches the
// disk. Dialogs are blocking `rfd` calls made by the shell.
//
// DECISION: `ContentHash` is FNV-1a 64 over the file bytes: no new
// dependency, stable across runs (the hash never leaves the process, but
// tests pin values), fast on a few-kB diagram file.
// DECISION: the watcher coalesces a burst of events for the file into one
// read after `QUIET` of silence (plan Review Focus: a save storm gives one
// reload with the last content). 60 ms covers writes up to 50 ms apart
// (the plan's storm) and leaves 140 ms of EDIT-29's 200 ms for the read,
// parse and build.
// DECISION: a symlinked diagram file is resolved once, when the watcher
// starts: it watches the link's directory and the canonical target's
// directory and matches the link's name in the first and the target's
// name in the second (editors write through the link, `write_atomic`
// writes the target). A link re-pointed while open is picked up on the
// next open.
// DECISION: `write_checked` reads, compares the hash, then renames; a
// write by another program between the read and the rename is lost to
// that rename. Accepted: the window is microseconds, std has no
// compare-and-swap rename, and the next watcher event shows the result
// (EDIT-24 covers the common case of an external save before the write).
// DECISION: per-user state is one plain text file,
// `$XDG_CONFIG_HOME/c4rizer/editor-state` (`$HOME/.config` when unset,
// HLD §2: no `directories` crate), one `key<TAB>value` line per entry;
// an unreadable or malformed file is ignored as empty, never an error.
// DECISION: at most `RECENT_MAX` = 10 recent files, newest first, a path
// re-opened moves to the front.
// DECISION: writes run on the UI thread: a diagram file is a few kB and
// an atomic write is two syscalls plus a rename, far under a frame.

use std::collections::BTreeMap;
use std::ffi::OsString;
use std::fs;
use std::io::{self, Write as _};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{self, Sender};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use c4rizer_core::model::ViewKind;

use crate::camera::{Camera, MAX_ZOOM, MIN_ZOOM, ScenePoint};
use crate::worker::{ExportFormat, ViewRef, Wake};

/// Hash of a file's bytes: how the app knows the file on disk is the one
/// it last read or wrote (EDIT-22, EDIT-24).
#[repr(transparent)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Default)]
pub struct ContentHash(pub u64);

/// FNV-1a 64 of `bytes`. Serves EDIT-22, EDIT-24.
#[must_use]
pub fn hash(bytes: &[u8]) -> ContentHash {
    const OFFSET: u64 = 0xcbf2_9ce4_8422_2325;
    const PRIME: u64 = 0x0000_0100_0000_01b3;
    let mut h = OFFSET;
    for &b in bytes {
        h ^= u64::from(b);
        h = h.wrapping_mul(PRIME);
    }
    ContentHash(h)
}

/// A file event for the open diagram file, as `state` consumes it.
/// Serves EDIT-1, EDIT-11, EDIT-22..24, EDIT-27.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FileEvent {
    /// The file was read on open (EDIT-13).
    Loaded {
        /// The path opened, as given (symlinks not resolved).
        path: PathBuf,
        /// Its text.
        text: String,
        /// `hash` of the text.
        hash: ContentHash,
    },
    /// Opening failed (missing, unreadable, not UTF-8).
    OpenFailed {
        /// The path.
        path: PathBuf,
        /// The I/O error in one sentence (SRS 5.1).
        error: String,
    },
    /// The watcher saw the file change, in place or by rename (EDIT-1,
    /// EDIT-23), or reappear (EDIT-27). `state` ignores it when `hash` is
    /// the last read or written one (EDIT-22).
    Changed {
        /// The new text.
        text: String,
        /// `hash` of the text.
        hash: ContentHash,
    },
    /// The file is gone (deleted or renamed away and not back) (EDIT-27).
    Removed,
    /// A `WriteRequest` was written (EDIT-22).
    Written {
        /// `hash` of the text written; the watcher's echo carries it.
        hash: ContentHash,
    },
    /// A `WriteRequest` was not written because the file on disk is not
    /// the one last read (EDIT-24): `state` reloads it and re-applies the
    /// action.
    ChangedOnDisk {
        /// The text on disk.
        text: String,
        /// `hash` of the text.
        hash: ContentHash,
    },
    /// A write failed (EDIT-27): `state` reverts the canvas to the
    /// file's state and shows the error.
    WriteFailed {
        /// The I/O error in one sentence (SRS 5.1).
        error: String,
    },
}

/// A write `state` asks for: `text` to `path`, only if the file still
/// hashes to `expected` (EDIT-24). Serves EDIT-3, EDIT-22, EDIT-24.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WriteRequest {
    /// The diagram file, as opened.
    pub path: PathBuf,
    /// The full new text (`edit::Applied::text`).
    pub text: String,
    /// The hash of the text the edits were applied to.
    pub expected: ContentHash,
}

/// Reads `path` as UTF-8 and hashes it (EDIT-13, EDIT-1).
///
/// # Errors
///
/// The I/O error, or `InvalidData` when the file is not UTF-8.
pub fn read(path: &Path) -> io::Result<(String, ContentHash)> {
    let bytes = fs::read(path)?;
    let h = hash(&bytes);
    let text = String::from_utf8(bytes)
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidData, "the file is not UTF-8 text"))?;
    Ok((text, h))
}

/// Writes `text` to `path` atomically (EDIT-22): to a temporary file in
/// the target's directory, then renamed over it. A symlink is resolved
/// first, so the link's target is written and the link stays; the
/// target's permissions are copied to the new file. Returns the hash of
/// `text`.
///
/// # Errors
///
/// Any I/O error; the original file is then untouched and the temporary
/// file removed (EDIT-27).
pub fn write_atomic(path: &Path, text: &str) -> io::Result<ContentHash> {
    // A symlink is written through: the rename replaces its target.
    let target = match fs::canonicalize(path) {
        Ok(t) => t,
        Err(e) if e.kind() == io::ErrorKind::NotFound => path.to_path_buf(),
        Err(e) => return Err(e),
    };
    let dir = match target.parent() {
        Some(d) if !d.as_os_str().is_empty() => d.to_path_buf(),
        _ => PathBuf::from("."),
    };
    let name = target
        .file_name()
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "the path names no file"))?;
    let tmp = dir.join(temp_name(name));
    let result = write_temp(&tmp, &target, text).and_then(|()| fs::rename(&tmp, &target));
    if let Err(e) = result {
        let _ = fs::remove_file(&tmp);
        return Err(e);
    }
    Ok(hash(text.as_bytes()))
}

/// Executes `request`: reads the file, and if its hash is not
/// `request.expected` returns `FileEvent::ChangedOnDisk` with its text
/// (EDIT-24); else `write_atomic` and `FileEvent::Written`; an I/O error
/// on either step is `FileEvent::WriteFailed` (EDIT-27), a missing file
/// too. Serves EDIT-22, EDIT-24, EDIT-27.
#[must_use]
pub fn write_checked(request: &WriteRequest) -> FileEvent {
    let (text, on_disk) = match read(&request.path) {
        Ok(read) => read,
        Err(e) => {
            return FileEvent::WriteFailed {
                error: io_sentence("cannot read", &request.path, &e),
            };
        }
    };
    if on_disk != request.expected {
        return FileEvent::ChangedOnDisk { text, hash: on_disk };
    }
    match write_atomic(&request.path, &request.text) {
        Ok(hash) => FileEvent::Written { hash },
        Err(e) => FileEvent::WriteFailed {
            error: io_sentence("cannot write", &request.path, &e),
        },
    }
}

/// How long the watcher waits after the last event for the file before
/// it reads it: one reload per save storm (EDIT-1), inside EDIT-29's
/// 200 ms.
pub const QUIET: Duration = Duration::from_millis(60);

/// Watches the open file's directory (EDIT-23: saves by rename are seen)
/// and sends one `FileEvent::Changed` or `FileEvent::Removed` per burst
/// of events for that file, after `QUIET`. When the path is a symlink it
/// also watches the canonical target's directory and matches the
/// target's file name there, so writes through the link and writes to
/// the target are both seen (EDIT-1, EDIT-22, EDIT-23). Events for other
/// files of those directories are ignored. Dropping it stops watching.
/// Serves EDIT-1, EDIT-23, EDIT-27.
// DECISION: the fields are held, not read: `inner` and `thread` live as
// long as the watcher (dropping `inner` ends the debounce thread), and
// the paths record what is watched for a debugger.
#[allow(dead_code, reason = "fields are held for their lifetime, not read")]
pub struct Watcher {
    /// The file watched, as opened (symlink not resolved).
    pub path: PathBuf,
    /// `path` canonicalized at start: equal to `path` for a plain file,
    /// the link's target for a symlink.
    pub target: PathBuf,
    /// The `notify` watcher on `path`'s parent directory and, when it
    /// differs, `target`'s parent directory.
    pub inner: notify::RecommendedWatcher,
    /// The debounce thread that reads the file and sends events.
    pub thread: Option<JoinHandle<()>>,
}

impl Watcher {
    /// Starts watching `path`'s directory and, for a symlink, its
    /// canonical target's directory; events go to `events`, and `wake` is
    /// called after each one.
    ///
    /// # Errors
    ///
    /// The `notify` error when the directory cannot be watched; the app
    /// then shows it in the status bar and works without live reload.
    pub fn start(path: &Path, events: Sender<FileEvent>, wake: Wake) -> notify::Result<Watcher> {
        let path = std::path::absolute(path).unwrap_or_else(|_| path.to_path_buf());
        let target = fs::canonicalize(&path).unwrap_or_else(|_| path.clone());
        let (tx, rx) = mpsc::channel::<notify::Event>();
        let mut inner = notify::recommended_watcher(move |res: notify::Result<notify::Event>| {
            if let Ok(event) = res {
                let _ = tx.send(event);
            }
        })?;
        let mut dirs = vec![parent_dir(&path)];
        let target_dir = parent_dir(&target);
        if target_dir != dirs[0] {
            dirs.push(target_dir);
        }
        for dir in &dirs {
            notify::Watcher::watch(&mut inner, dir, notify::RecursiveMode::NonRecursive)?;
        }
        let names = [path.clone(), target.clone()];
        let read_path = path.clone();
        let thread = std::thread::Builder::new()
            .name("c4rizer-watch".to_owned())
            .spawn(move || debounce(&rx, &names, &read_path, &events, &wake))
            .map_err(notify::Error::io)?;
        Ok(Watcher {
            path,
            target,
            inner,
            thread: Some(thread),
        })
    }
}

/// `model::FsLoader` that remembers every file a parse read and the hash
/// of its bytes, in read order (P2-136, P2-141): core gives each file read
/// the next `FileId` from 1 and reads each file once, so `read[i]` is
/// `FileId(i + 1)`, also when the parse fails and returns no
/// `Parsed::paths`. The hashes are the per-file own-write hashes
/// (EDIT-22).
// DECISION: the parse reads included files from disk through this loader
// while `state` handles the root file's text event: the model needs every
// file at once, and `model::parse_with` is the only way to get it. The
// root text still comes through `FileEvent`s, so EDIT-22/EDIT-24 keep
// working on it unchanged.
#[derive(Debug, Default)]
pub(crate) struct Reads {
    /// Every file read, in read order, with the hash of its bytes.
    pub(crate) read: Vec<(PathBuf, ContentHash)>,
}

impl c4rizer_core::model::Loader for Reads {
    fn read(&mut self, path: &Path) -> io::Result<Vec<u8>> {
        let bytes = c4rizer_core::model::FsLoader.read(path)?;
        self.read.push((path.to_path_buf(), hash(&bytes)));
        Ok(bytes)
    }

    fn read_dir(&mut self, path: &Path) -> io::Result<Vec<PathBuf>> {
        c4rizer_core::model::FsLoader.read_dir(path)
    }

    fn canonicalize(&mut self, path: &Path) -> io::Result<PathBuf> {
        c4rizer_core::model::FsLoader.canonicalize(path)
    }

    fn env(&self, name: &str) -> Option<String> {
        c4rizer_core::model::FsLoader.env(name)
    }
}

/// Watches the files a diagram includes (P2-141): their directories, so
/// a save by rename is seen (EDIT-23), with one `(path, Changed)` or
/// `(path, Removed)` per burst of events for each file after `QUIET`
/// (EDIT-1). Dropping it stops watching.
// DECISION: a second watcher type next to `Watcher`, not a change to
// `Watcher::start`'s signature (pub, step-4 contract): the root file
// keeps its watcher and its `FileEvent` channel, and included files come
// with their path on a channel of their own, which `state` reads through
// `State::included_file`.
#[allow(dead_code, reason = "fields are held for their lifetime, not read")]
pub(crate) struct FilesWatcher {
    /// The files watched (absolute paths as the parse read them).
    pub(crate) paths: Vec<PathBuf>,
    /// The `notify` watcher on their directories.
    inner: notify::RecommendedWatcher,
    /// The debounce thread that reads the files and sends events.
    thread: Option<JoinHandle<()>>,
}

/// Starts a `FilesWatcher` on `paths`; events go to `events`, and `wake`
/// is called after each one. Serves P2-141, EDIT-1, EDIT-23.
///
/// # Errors
///
/// The `notify` error when a directory cannot be watched.
pub(crate) fn watch_files(
    paths: &[PathBuf],
    events: Sender<(PathBuf, FileEvent)>,
    wake: Wake,
) -> notify::Result<FilesWatcher> {
    let (tx, rx) = mpsc::channel::<notify::Event>();
    let mut inner = notify::recommended_watcher(move |res: notify::Result<notify::Event>| {
        if let Ok(event) = res {
            let _ = tx.send(event);
        }
    })?;
    let mut dirs: Vec<PathBuf> = paths.iter().map(|p| parent_dir(p)).collect();
    dirs.sort();
    dirs.dedup();
    for dir in &dirs {
        notify::Watcher::watch(&mut inner, dir, notify::RecursiveMode::NonRecursive)?;
    }
    let names = paths.to_vec();
    let thread = std::thread::Builder::new()
        .name("c4rizer-watch-includes".to_owned())
        .spawn(move || debounce_files(&rx, &names, &events, &wake))
        .map_err(notify::Error::io)?;
    Ok(FilesWatcher {
        paths: paths.to_vec(),
        inner,
        thread: Some(thread),
    })
}

/// The `FilesWatcher` thread: per file of `names`, after `QUIET` with no
/// event for it, reads it and sends `(path, Changed)` or `(path,
/// Removed)`; ends when the `notify` watcher is dropped.
fn debounce_files(
    rx: &mpsc::Receiver<notify::Event>,
    names: &[PathBuf],
    events: &Sender<(PathBuf, FileEvent)>,
    wake: &Wake,
) {
    // DECISION: a `BTreeMap` of deadlines, so files due at once are read
    // in path order (deterministic order of reloads).
    let mut due: BTreeMap<PathBuf, Instant> = BTreeMap::new();
    loop {
        let event = match due.values().min().copied() {
            None => match rx.recv() {
                Ok(e) => Some(e),
                Err(_) => return,
            },
            Some(deadline) => match rx.recv_timeout(deadline.saturating_duration_since(Instant::now())) {
                Ok(e) => Some(e),
                Err(mpsc::RecvTimeoutError::Timeout) => None,
                Err(mpsc::RecvTimeoutError::Disconnected) => return,
            },
        };
        if let Some(event) = event {
            if changes_content(&event) {
                for p in event.paths.iter().filter(|p| names.contains(p)) {
                    due.insert(p.clone(), Instant::now() + QUIET);
                }
            }
            continue;
        }
        let now = Instant::now();
        let ready: Vec<PathBuf> = due.iter().filter(|(_, d)| **d <= now).map(|(p, _)| p.clone()).collect();
        for path in ready {
            due.remove(&path);
            let event = match read(&path) {
                Ok((text, hash)) => FileEvent::Changed { text, hash },
                Err(e) if e.kind() == io::ErrorKind::NotFound => FileEvent::Removed,
                // DECISION: as `debounce`: a file caught mid-write is
                // skipped; its close event brings another read.
                Err(_) => continue,
            };
            if events.send((path, event)).is_err() {
                return;
            }
            wake();
        }
    }
}

/// Camera and view remembered for one diagram file (EDIT-13). Never
/// holds diagram content.
#[derive(Debug, Clone, PartialEq)]
pub struct FileMemory {
    /// The view last shown.
    pub view: ViewRef,
    /// Pan and zoom last shown.
    pub camera: Camera,
}

/// Per-user UI state (EDIT-13): recent files and, per file, the last
/// view, zoom and pan. Serves EDIT-13.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct UiState {
    /// Recently opened files, newest first, at most `RECENT_MAX`.
    pub recent: Vec<PathBuf>,
    /// Per file, by the path as opened.
    pub files: BTreeMap<PathBuf, FileMemory>,
}

/// The most recent files kept (EDIT-13).
pub const RECENT_MAX: usize = 10;

impl UiState {
    /// Records `path` as just opened: moved or added to the front of
    /// `recent`, the list cut to `RECENT_MAX`. Serves EDIT-13.
    pub fn opened(&mut self, path: &Path) {
        self.recent.retain(|p| p != path);
        self.recent.insert(0, path.to_path_buf());
        self.recent.truncate(RECENT_MAX);
    }

    /// Parses the state file's text; malformed lines are skipped.
    /// Serves EDIT-13.
    #[must_use]
    pub fn parse(text: &str) -> UiState {
        let mut state = UiState::default();
        for line in text.lines() {
            let fields: Vec<String> = line.split('\t').map(unescape).collect();
            match fields.as_slice() {
                [key, path] if key == "recent" => {
                    if state.recent.len() < RECENT_MAX && !state.recent.iter().any(|p| p.as_os_str() == path.as_str()) {
                        state.recent.push(PathBuf::from(path));
                    }
                }
                [key, view, x, y, zoom, path] if key == "file" => {
                    let (Some(view), Ok(x), Ok(y), Ok(zoom)) =
                        (view_from_text(view), x.parse::<f32>(), y.parse::<f32>(), zoom.parse::<f32>())
                    else {
                        continue;
                    };
                    if !(x.is_finite() && y.is_finite() && zoom.is_finite() && zoom > 0.0) {
                        continue;
                    }
                    let camera = Camera {
                        pan: ScenePoint { x, y },
                        zoom: zoom.clamp(MIN_ZOOM, MAX_ZOOM),
                    };
                    state.files.insert(PathBuf::from(path), FileMemory { view, camera });
                }
                _ => {}
            }
        }
        state
    }

    /// The state file's text; `UiState::parse` of it gives `self` back.
    /// Serves EDIT-13.
    #[must_use]
    pub fn to_text(&self) -> String {
        use std::fmt::Write as _;
        let mut out = String::new();
        for path in &self.recent {
            // DECISION: a path that is not UTF-8 is not remembered: the
            // state file is text, and such paths are rare enough.
            if let Some(p) = path.to_str() {
                out.push_str("recent\t");
                out.push_str(&escape(p));
                out.push('\n');
            }
        }
        for (path, memory) in &self.files {
            let Some(p) = path.to_str() else { continue };
            let c = memory.camera;
            let _ = writeln!(
                out,
                "file\t{}\t{}\t{}\t{}\t{}",
                escape(&view_to_text(&memory.view)),
                c.pan.x,
                c.pan.y,
                c.zoom,
                escape(p)
            );
        }
        out
    }

    /// Loads from `path`; a missing or unreadable file gives the empty
    /// state. Serves EDIT-13.
    #[must_use]
    pub fn load(path: &Path) -> UiState {
        match fs::read_to_string(path) {
            Ok(text) => UiState::parse(&text),
            Err(_) => UiState::default(),
        }
    }

    /// Saves to `path` with `write_atomic`, creating its directory.
    /// Serves EDIT-13.
    ///
    /// # Errors
    ///
    /// Any I/O error; the shell reports it in the status bar and goes on.
    pub fn save(&self, path: &Path) -> io::Result<()> {
        if let Some(dir) = path.parent().filter(|d| !d.as_os_str().is_empty()) {
            fs::create_dir_all(dir)?;
        }
        write_atomic(path, &self.to_text()).map(|_| ())
    }
}

/// `$XDG_CONFIG_HOME/c4rizer/editor-state`, or
/// `$HOME/.config/c4rizer/editor-state` when `XDG_CONFIG_HOME` is unset
/// or empty; `None` when neither variable is set. Serves EDIT-13.
#[must_use]
pub fn state_file() -> Option<PathBuf> {
    state_file_from(
        std::env::var_os("XDG_CONFIG_HOME"),
        std::env::var_os("HOME"),
    )
}

/// The Open dialog (`rfd`), starting in `dir` when given, filtered to
/// `.dsl` files with an "all files" fallback; `None` on cancel. Blocking,
/// called by the shell. Serves EDIT-13.
#[must_use]
pub fn open_dialog(dir: Option<&Path>) -> Option<PathBuf> {
    let mut dialog = rfd::FileDialog::new()
        .set_title("Open diagram file")
        .add_filter("c4rizer diagram", &["dsl"])
        .add_filter("All files", &["*"]);
    if let Some(dir) = dir {
        dialog = dialog.set_directory(dir);
    }
    dialog.pick_file()
}

/// The save dialog for "Export view" (EDIT-7): starts in `dir` (the
/// diagram file's folder) with the file name `name`; `None` on cancel.
#[must_use]
pub fn save_dialog(dir: &Path, name: &str, format: ExportFormat) -> Option<PathBuf> {
    let label = match format {
        ExportFormat::Svg | ExportFormat::SvgEmbedded | ExportFormat::SvgOutlined => "SVG image",
        ExportFormat::Png => "PNG image",
        ExportFormat::PlantUml | ExportFormat::C4PlantUml => "PlantUML",
        ExportFormat::Mermaid => "Mermaid",
        ExportFormat::WebSequenceDiagrams => "WebSequenceDiagrams",
        ExportFormat::Dot => "Graphviz DOT",
        ExportFormat::Ilograph => "Ilograph",
        ExportFormat::Json | ExportFormat::Theme => "JSON",
        ExportFormat::Static => "HTML",
    };
    rfd::FileDialog::new()
        .set_title("Export view")
        .set_directory(dir)
        .set_file_name(name)
        .add_filter(label, &[format.extension()])
        .save_file()
}

/// The folder dialog for "Export all views" (EDIT-7), starting in `dir`
/// (the diagram file's folder); every declared view is then written as
/// `<view key>.svg` or `<view key>.png` in the chosen folder, replacing
/// files of the same name. `None` on cancel.
#[must_use]
pub fn folder_dialog(dir: &Path) -> Option<PathBuf> {
    rfd::FileDialog::new()
        .set_title("Export all views to folder")
        .set_directory(dir)
        .pick_folder()
}

/// `<view key>.svg` or `<view key>.png` (EDIT-7); a Structurizr text
/// format's `structurizr-<view key>.<ext>`, as `c4rizer export` names it
/// (P3-404, P3-431).
#[must_use]
pub fn export_name(view_key: &str, format: ExportFormat) -> String {
    if format.per_view_text() {
        format!("structurizr-{view_key}.{}", format.extension())
    } else {
        format!("{view_key}.{}", format.extension())
    }
}

/// The folder of the diagram file, the export dialogs' start (EDIT-7):
/// its parent directory, `.` for a bare file name.
#[must_use]
pub fn export_dir(diagram: &Path) -> PathBuf {
    match diagram.parent() {
        Some(dir) if !dir.as_os_str().is_empty() => dir.to_path_buf(),
        _ => PathBuf::from("."),
    }
}

/// The state file path from the two variables `state_file` reads.
fn state_file_from(xdg: Option<OsString>, home: Option<OsString>) -> Option<PathBuf> {
    if let Some(xdg) = xdg.filter(|v| !v.is_empty()) {
        return Some(PathBuf::from(xdg).join("c4rizer").join("editor-state"));
    }
    let home = home.filter(|v| !v.is_empty())?;
    Some(PathBuf::from(home).join(".config").join("c4rizer").join("editor-state"))
}

/// One status-bar sentence for an I/O error on `path` (SRS 5.1).
fn io_sentence(what: &str, path: &Path, e: &io::Error) -> String {
    format!("{what} {}: {e}", path.display())
}

/// A temporary file name next to the target, unique per process and call.
// DECISION: `.<name>.c4rizer-<pid>-<n>.tmp`: hidden, so file managers and
// the watcher's name match ignore it, and unique without a random source.
fn temp_name(name: &std::ffi::OsStr) -> OsString {
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let n = COUNTER.fetch_add(1, Ordering::Relaxed);
    let mut out = OsString::from(".");
    out.push(name);
    out.push(format!(".c4rizer-{}-{n}.tmp", std::process::id()));
    out
}

/// Writes `text` to the new file `tmp`, with `target`'s permissions when
/// it exists, and flushes it to disk.
fn write_temp(tmp: &Path, target: &Path, text: &str) -> io::Result<()> {
    let mut file = fs::OpenOptions::new().write(true).create_new(true).open(tmp)?;
    file.write_all(text.as_bytes())?;
    if let Ok(meta) = fs::metadata(target) {
        file.set_permissions(meta.permissions())?;
    }
    file.sync_all()
}

/// The directory holding `path` (absolute), `.` when it has none.
fn parent_dir(path: &Path) -> PathBuf {
    match path.parent() {
        Some(d) if !d.as_os_str().is_empty() => d.to_path_buf(),
        _ => PathBuf::from("."),
    }
}

/// True when `event` may have changed the content of the file: reads and
/// metadata-only changes are not (the watcher's own reads open the file).
fn changes_content(event: &notify::Event) -> bool {
    use notify::EventKind;
    use notify::event::{AccessKind, AccessMode, ModifyKind};
    match event.kind {
        EventKind::Access(AccessKind::Close(AccessMode::Write)) | EventKind::Any | EventKind::Create(_) | EventKind::Remove(_) => true,
        EventKind::Modify(ModifyKind::Metadata(_)) | EventKind::Access(_) | EventKind::Other => false,
        EventKind::Modify(_) => true,
    }
}

/// The watcher thread: waits for an event naming one of `names`, then
/// for `QUIET` without another, reads `path` and sends `Changed` or
/// `Removed`; ends when the `notify` watcher is dropped.
fn debounce(
    rx: &mpsc::Receiver<notify::Event>,
    names: &[PathBuf],
    path: &Path,
    events: &Sender<FileEvent>,
    wake: &Wake,
) {
    let concerns = |e: &notify::Event| changes_content(e) && e.paths.iter().any(|p| names.contains(p));
    loop {
        match rx.recv() {
            Ok(event) if concerns(&event) => {}
            Ok(_) => continue,
            Err(_) => return,
        }
        // Coalesce the burst: wait until `QUIET` passes with no event for
        // the file.
        let mut deadline = Instant::now() + QUIET;
        loop {
            let left = deadline.saturating_duration_since(Instant::now());
            match rx.recv_timeout(left) {
                Ok(event) if concerns(&event) => deadline = Instant::now() + QUIET,
                Ok(_) => {}
                Err(mpsc::RecvTimeoutError::Timeout) => break,
                Err(mpsc::RecvTimeoutError::Disconnected) => return,
            }
        }
        let event = match read(path) {
            Ok((text, hash)) => FileEvent::Changed { text, hash },
            Err(e) if e.kind() == io::ErrorKind::NotFound => FileEvent::Removed,
            // DECISION: a file caught mid-write (not UTF-8 yet) or
            // unreadable is skipped: the write's own close event brings
            // another read.
            Err(_) => continue,
        };
        if events.send(event).is_err() {
            return;
        }
        wake();
    }
}

/// Escapes `\\`, tab and line breaks for one state-file field.
fn escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '\\' => out.push_str("\\\\"),
            '\t' => out.push_str("\\t"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            c => out.push(c),
        }
    }
    out
}

/// Reverses `escape`.
fn unescape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut chars = s.chars();
    while let Some(c) = chars.next() {
        if c != '\\' {
            out.push(c);
            continue;
        }
        match chars.next() {
            Some('t') => out.push('\t'),
            Some('n') => out.push('\n'),
            Some('r') => out.push('\r'),
            Some(other) => out.push(other),
            None => out.push('\\'),
        }
    }
    out
}

/// `declared:<key>` or `default:<kind keyword>:<scope>`.
// DECISION: the state file names a view the way `ViewRef` does, so a
// default view not yet written is remembered too.
fn view_to_text(view: &ViewRef) -> String {
    match view {
        ViewRef::Declared(key) => format!("declared:{key}"),
        ViewRef::Default { scope, kind } => format!("default:{}:{scope}", kind.keyword()),
    }
}

/// Reverses `view_to_text`; `None` when malformed.
fn view_from_text(s: &str) -> Option<ViewRef> {
    if let Some(key) = s.strip_prefix("declared:") {
        return Some(ViewRef::Declared(key.to_owned()));
    }
    let rest = s.strip_prefix("default:")?;
    let (kind, scope) = rest.split_once(':')?;
    let kind = [ViewKind::SystemContext, ViewKind::Container, ViewKind::Component]
        .into_iter()
        .find(|k| k.keyword() == kind)?;
    Some(ViewRef::Default {
        scope: scope.to_owned(),
        kind,
    })
}

#[cfg(test)]
mod tests {
    use std::path::{Path, PathBuf};
    use std::sync::Arc;
    use std::sync::mpsc::{self, Receiver};
    use std::time::Duration;

    use c4rizer_core::model::ViewKind;

    use super::{
        ContentHash, FileEvent, FileMemory, QUIET, RECENT_MAX, UiState, Watcher, WriteRequest,
        export_dir, export_name, hash, read, state_file, state_file_from, write_atomic,
        write_checked,
    };
    use crate::camera::{Camera, ScenePoint};
    use crate::testutil::TempDir;
    use crate::worker::{ExportFormat, ViewRef};

    /// Size pin for `ContentHash`, compared on every file event.
    #[test]
    fn content_hash_is_8_bytes() {
        assert_eq!(std::mem::size_of::<super::ContentHash>(), 8);
    }

    /// Starts a watcher on `path`, returning it and its event channel.
    fn watch(path: &Path) -> (Watcher, Receiver<FileEvent>) {
        let (tx, rx) = mpsc::channel();
        let watcher = Watcher::start(path, tx, Arc::new(|| {})).expect("watch");
        // Let the backend settle before the first write.
        std::thread::sleep(Duration::from_millis(50));
        (watcher, rx)
    }

    /// The next event within 3 s.
    fn next(rx: &Receiver<FileEvent>) -> FileEvent {
        rx.recv_timeout(Duration::from_secs(3)).expect("a file event")
    }

    /// No event within four `QUIET` periods.
    fn quiet(rx: &Receiver<FileEvent>) {
        if let Ok(e) = rx.recv_timeout(QUIET * 4) {
            panic!("unexpected event {e:?}");
        }
    }

    fn changed(text: &str) -> FileEvent {
        FileEvent::Changed {
            text: text.to_owned(),
            hash: hash(text.as_bytes()),
        }
    }

    #[test]
    fn hash_is_fnv1a_64() {
        assert_eq!(hash(b""), ContentHash(0xcbf2_9ce4_8422_2325));
        assert_eq!(hash(b"a"), ContentHash(0xaf63_dc4c_8601_ec8c));
    }

    /// EDIT-13 (plan editor-01): recent files and per-file view, zoom and
    /// pan survive `save` then `load` in a temp dir; diagram text never
    /// appears in the state file.
    #[test]
    fn edit_13_recent_files_round_trip() {
        let dir = TempDir::new("ui-state");
        let diagram = dir.join("shop.dsl");
        std::fs::write(&diagram, "workspace { model { secret = person \"Operator\" } }").unwrap();
        let mut state = UiState::default();
        state.opened(&dir.join("other\tname.dsl"));
        state.opened(&diagram);
        state.files.insert(
            diagram.clone(),
            FileMemory {
                view: ViewRef::Declared("containers".to_owned()),
                camera: Camera {
                    pan: ScenePoint { x: -12.5, y: 40.25 },
                    zoom: 1.75,
                },
            },
        );
        state.files.insert(
            dir.join("other\tname.dsl"),
            FileMemory {
                view: ViewRef::Default {
                    scope: "shop".to_owned(),
                    kind: ViewKind::Container,
                },
                camera: Camera::default(),
            },
        );
        let file = dir.join("config").join("c4rizer").join("editor-state");
        state.save(&file).expect("save");
        let loaded = UiState::load(&file);
        assert_eq!(loaded, state);
        let text = std::fs::read_to_string(&file).unwrap();
        assert!(!text.contains("Operator") && !text.contains("secret"), "{text}");
        assert_eq!(UiState::load(&dir.join("missing")), UiState::default());
        assert_eq!(UiState::parse("garbage\nfile\tx\n"), UiState::default());
    }

    /// EDIT-13: re-opening a path moves it to the front; the list stops
    /// at `RECENT_MAX`.
    #[test]
    fn edit_13_recent_files_capped_newest_first() {
        let mut state = UiState::default();
        for i in 0..RECENT_MAX + 3 {
            state.opened(Path::new(&format!("/d/{i}.dsl")));
        }
        assert_eq!(state.recent.len(), RECENT_MAX);
        assert_eq!(state.recent[0], PathBuf::from(format!("/d/{}.dsl", RECENT_MAX + 2)));
        state.opened(Path::new("/d/5.dsl"));
        assert_eq!(state.recent[0], PathBuf::from("/d/5.dsl"));
        assert_eq!(state.recent.len(), RECENT_MAX);
        assert_eq!(state.recent.iter().filter(|p| p.ends_with("5.dsl")).count(), 1);
    }

    /// EDIT-13: `state_file` follows `XDG_CONFIG_HOME`, then
    /// `$HOME/.config`.
    #[test]
    fn edit_13_state_file_under_xdg_config_home() {
        assert_eq!(
            state_file_from(Some("/x".into()), Some("/h".into())),
            Some(PathBuf::from("/x/c4rizer/editor-state"))
        );
        assert_eq!(
            state_file_from(Some("".into()), Some("/h".into())),
            Some(PathBuf::from("/h/.config/c4rizer/editor-state"))
        );
        assert_eq!(
            state_file_from(None, Some("/h".into())),
            Some(PathBuf::from("/h/.config/c4rizer/editor-state"))
        );
        assert_eq!(state_file_from(None, None), None);
        assert_eq!(
            state_file(),
            state_file_from(std::env::var_os("XDG_CONFIG_HOME"), std::env::var_os("HOME"))
        );
    }

    /// EDIT-22 (plan editor-06): a write goes to a temp file and is
    /// renamed; permissions kept; through a symlink its target is written
    /// and the link stays a link; the watcher's echo of it carries the
    /// written hash (ignored by `state`).
    #[test]
    fn edit_22_atomic_write_ignored_by_watcher() {
        use std::os::unix::fs::PermissionsExt as _;
        let dir = TempDir::new("atomic");
        let file = dir.join("d.dsl");
        std::fs::write(&file, "old").unwrap();
        std::fs::set_permissions(&file, std::fs::Permissions::from_mode(0o640)).unwrap();
        let inode_before = std::os::unix::fs::MetadataExt::ino(&std::fs::metadata(&file).unwrap());
        let (_w, rx) = watch(&file);
        let h = write_atomic(&file, "new text").expect("write");
        assert_eq!(h, hash(b"new text"));
        let meta = std::fs::metadata(&file).unwrap();
        assert_eq!(meta.permissions().mode() & 0o777, 0o640);
        assert_ne!(std::os::unix::fs::MetadataExt::ino(&meta), inode_before, "renamed, not rewritten");
        assert_eq!(std::fs::read_to_string(&file).unwrap(), "new text");
        let leftovers: Vec<_> = std::fs::read_dir(dir.path()).unwrap().flatten().map(|e| e.file_name()).collect();
        assert_eq!(leftovers.len(), 1, "no temp file left: {leftovers:?}");
        assert_eq!(next(&rx), FileEvent::Changed { text: "new text".to_owned(), hash: h });
        quiet(&rx);

        let real = dir.join("real.dsl");
        let link = dir.join("link.dsl");
        std::fs::write(&real, "r").unwrap();
        std::os::unix::fs::symlink(&real, &link).unwrap();
        write_atomic(&link, "through").expect("write through link");
        assert!(std::fs::symlink_metadata(&link).unwrap().file_type().is_symlink());
        assert_eq!(std::fs::read_to_string(&real).unwrap(), "through");
    }

    /// EDIT-23 (plan editor-06): a save done by writing a sibling and
    /// renaming it over the file gives one `Changed` with the new text.
    #[test]
    fn edit_23_rename_save_detected() {
        let dir = TempDir::new("rename");
        let file = dir.join("d.dsl");
        std::fs::write(&file, "one").unwrap();
        let (_w, rx) = watch(&file);
        let sibling = dir.join("d.dsl~");
        std::fs::write(&sibling, "two").unwrap();
        std::fs::rename(&sibling, &file).unwrap();
        assert_eq!(next(&rx), changed("two"));
        quiet(&rx);
        std::fs::write(dir.join("unrelated.txt"), "x").unwrap();
        quiet(&rx);
    }

    /// EDIT-1 (plan Review Focus): several writes, each under `QUIET`
    /// after the previous (the plan's storm: within 50 ms), give one
    /// `Changed` with the last content.
    #[test]
    fn edit_1_save_storm_one_reload_last_content() {
        let dir = TempDir::new("storm");
        let file = dir.join("d.dsl");
        std::fs::write(&file, "0").unwrap();
        let (_w, rx) = watch(&file);
        for i in 1..=5 {
            std::fs::write(&file, format!("{i}")).unwrap();
            std::thread::sleep(Duration::from_millis(10));
        }
        assert_eq!(next(&rx), changed("5"));
        quiet(&rx);
    }

    /// EDIT-23, EDIT-1: with the diagram path a symlink to a file in
    /// another directory, a save to the target (in place, and by rename
    /// in the target's directory) gives one `Changed` each; the app's own
    /// `write_atomic` through the link is seen with its hash (EDIT-22).
    #[test]
    fn edit_23_symlinked_file_reloads() {
        let a = TempDir::new("link-dir");
        let b = TempDir::new("target-dir");
        let real = b.join("real.dsl");
        let link = a.join("link.dsl");
        std::fs::write(&real, "one").unwrap();
        std::os::unix::fs::symlink(&real, &link).unwrap();
        let (_w, rx) = watch(&link);
        std::fs::write(&real, "two").unwrap();
        assert_eq!(next(&rx), changed("two"));
        quiet(&rx);
        let sibling = b.join("real.tmp");
        std::fs::write(&sibling, "three").unwrap();
        std::fs::rename(&sibling, &real).unwrap();
        assert_eq!(next(&rx), changed("three"));
        quiet(&rx);
        let h = write_atomic(&link, "four").unwrap();
        assert_eq!(next(&rx), FileEvent::Changed { text: "four".to_owned(), hash: h });
        quiet(&rx);
    }

    /// EDIT-24: `write_checked` refuses to write when the disk hash is
    /// not `expected` and returns the text on disk.
    #[test]
    fn edit_24_write_checked_refuses_changed_file() {
        let dir = TempDir::new("checked");
        let file = dir.join("d.dsl");
        std::fs::write(&file, "mine").unwrap();
        let (_, h) = read(&file).unwrap();
        std::fs::write(&file, "theirs").unwrap();
        let event = write_checked(&WriteRequest {
            path: file.clone(),
            text: "mine edited".to_owned(),
            expected: h,
        });
        assert_eq!(
            event,
            FileEvent::ChangedOnDisk {
                text: "theirs".to_owned(),
                hash: hash(b"theirs")
            }
        );
        assert_eq!(std::fs::read_to_string(&file).unwrap(), "theirs");
        let event = write_checked(&WriteRequest {
            path: file.clone(),
            text: "theirs edited".to_owned(),
            expected: hash(b"theirs"),
        });
        assert_eq!(event, FileEvent::Written { hash: hash(b"theirs edited") });
        assert_eq!(std::fs::read_to_string(&file).unwrap(), "theirs edited");
    }

    /// EDIT-27: deleting the file gives `Removed`; recreating it gives
    /// `Changed`; a write into a read-only directory gives `WriteFailed`
    /// and leaves the file untouched.
    #[test]
    fn edit_27_removed_reappears_and_failed_write() {
        use std::os::unix::fs::PermissionsExt as _;
        let dir = TempDir::new("removed");
        let file = dir.join("d.dsl");
        std::fs::write(&file, "here").unwrap();
        let (w, rx) = watch(&file);
        std::fs::remove_file(&file).unwrap();
        assert_eq!(next(&rx), FileEvent::Removed);
        quiet(&rx);
        std::fs::write(&file, "back").unwrap();
        assert_eq!(next(&rx), changed("back"));
        drop(w);

        let missing = write_checked(&WriteRequest {
            path: dir.join("gone.dsl"),
            text: "x".to_owned(),
            expected: hash(b""),
        });
        assert!(matches!(missing, FileEvent::WriteFailed { .. }), "{missing:?}");

        std::fs::set_permissions(dir.path(), std::fs::Permissions::from_mode(0o555)).unwrap();
        // Root ignores directory permissions: then there is nothing to test.
        let probe = dir.join("probe");
        let writable = std::fs::write(&probe, "").is_ok();
        if !writable {
            let event = write_checked(&WriteRequest {
                path: file.clone(),
                text: "lost".to_owned(),
                expected: hash(b"back"),
            });
            assert!(matches!(&event, FileEvent::WriteFailed { error } if error.contains("d.dsl")), "{event:?}");
            assert_eq!(std::fs::read_to_string(&file).unwrap(), "back");
        }
        std::fs::set_permissions(dir.path(), std::fs::Permissions::from_mode(0o755)).unwrap();
    }

    /// EDIT-7: default export name and folder.
    #[test]
    fn edit_7_export_name_and_dir() {
        assert_eq!(export_name("containers", ExportFormat::Svg), "containers.svg");
        assert_eq!(export_name("C3-serving", ExportFormat::Png), "C3-serving.png");
        assert_eq!(export_dir(Path::new("/w/docs/shop.dsl")), PathBuf::from("/w/docs"));
        assert_eq!(export_dir(Path::new("shop.dsl")), PathBuf::from("."));
    }
}

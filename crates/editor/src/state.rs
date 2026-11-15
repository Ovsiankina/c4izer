//! Headless core of the editor: a plain state machine with no egui type
//! in its API. Input events (pointer, keys, panel commands), file events
//! and worker replies go in; file writes, worker requests, dialogs and a
//! draw list come out. Every EDIT requirement is tested here with a
//! temp-dir file, without a window (HLD §6).
//!
//! Depends on: `camera`, `worker`, `files`, `history`, `c4rizer-core`
//! (model, edit, pipeline, place, view, scene, lint, geom).

// Shape: `State::handle(Event) -> Vec<Effect>` is the only mutator. The
// shell (`app`) executes effects (`files::write_checked`, `Worker::send`,
// dialogs) and feeds their results back as events; tests do the same
// synchronously with `files` and `worker::serve`. Readers (`draw_list`,
// `views`, `breadcrumb`, `details`, `style_panel`, `findings`, `status`)
// are pure.
//
// Coordinates: pointer input is in screen px (`ScreenPoint`, f32, the
// canvas's top-left is 0,0); `Camera` maps screen px to scene px
// (`ScenePoint`, unrounded; the scene's i32 px, `scene` module header); `Scene::centres`, `place::Keep`
// and `pipeline::rebuild_after_move` take grid units (`geom::GRID_PX`);
// `edit::Edit::Move`/`Resize` take px. Conversion happens only here.
//
// Identity across a re-parse (EDIT-1, EDIT-19, EDIT-24): selection, the
// open view and every pending action are held by the keys written in the
// file (`Selected`, `ViewRef`, `Action`), never by `ElementId`, `BoxId`
// or `ItemKey`, which are valid for one parse or one scene only.
//
// EDIT-21: no gesture, command or file event produces an `edit::Edit`
// that creates or deletes an element or relationship; `edit` has no such
// variant, and the editor builds `Edit::RemoveDefaultView` only as the
// stored inverse of an `AddDefaultView` (undo), never from a gesture.
// The `edit_21_*` test walks every `Input` and `Command` over a fixture
// and checks every written batch.
//
// DECISION: drag, resize and rubber band start once the pointer travelled
// `CLICK_SLOP_PX` (4 screen px, EDIT-28) from the press; below that the
// release is a click (plan Review Focus: 1–2 px is still a click).
// DECISION: resize keeps the box centre and changes width and height in
// 20px steps (DSL-7 sizes are multiples of 20), so a resize rewrites only
// the `size` line (EDIT-18) and never the `at` line.
// DECISION: the text editor (EDIT-5) keeps one buffer per field; Tab
// moves between fields, Enter confirms and writes every changed field as
// one batch, one undo step; Escape drops all buffers.
// DECISION: lint findings are not drawn on the canvas (EDIT-12 allows
// selection and hover marks only); picking a finding in the panel selects
// its `Finding::items`, so they get selection marks.
// DECISION: "enterable boxes show a hover mark" (EDIT-15) is drawn while
// the pointer is over the box (`MarkKind::Enterable` instead of `Hover`).
// DECISION: zoom is clamped to `MIN_ZOOM..=MAX_ZOOM`, one wheel notch is
// `ZOOM_STEP`, and the point under the cursor stays put (EDIT-9).
// DECISION: alignment reference: left/right/top/bottom align to the
// outermost edge among the selected boxes, centre/middle to the centre of
// their bounding box; distribute keeps the two outermost boxes and spaces
// the centres between them evenly; results are snapped to the grid.
// DECISION: every writing `Action` names its view and carries the view's
// layout at the time of the gesture, so `edits_for` never reads
// `self.view` or the shown scene: a view switch while a write is in
// flight cannot retarget its EDIT-24 re-apply or its EDIT-15
// `AddDefaultView` decision.
// DECISION: the file's problems are stored once (`Doc::parse_error`,
// `Doc::missing`, `State::io_error`) and `State::status` derives the
// status bar from them; only the one-off notice is stored for it.
// DECISION: `Input` is hot (one per pointer event, EDIT-25) and holds no
// heap data; the in-place text field's content arrives as
// `Event::Text`, so `Input` stays `Copy`-sized and pinned at 16 bytes.
// DECISION: while a write is in flight (`State::pending`) further edits
// are refused with a status notice; a diagram write is one syscall batch,
// so this is never visible in practice.
//
// TODO(srs): EDIT-19 and EDIT-28 give no keys for undo and redo; stubbed
// against Structurizr and the platform convention: Ctrl+Z undo,
// Ctrl+Shift+Z and Ctrl+Y redo, plus menu commands (docs/deferred.md row).
// TODO(srs): EDIT-16 does not say what the workspace crumb (and Alt+Left
// or Backspace on a system context view) opens, since no view is scoped
// to the workspace in phase 1; stubbed as the first declared system
// context view, else the first declared view, else nothing
// (docs/deferred.md row).
// TODO(srs): EDIT-15 lists move, resize and style as the writes that add
// a default view to the file, but not a title or subtitle edit; stubbed
// as "a default view's title and subtitle are not editable until the
// view is written" (`text_fields` gives no view field for
// `ViewRef::Default`) (docs/deferred.md row).
// TODO(srs): EDIT-15 calls a box enterable when "a view is scoped to it",
// which by its wording includes a childless system with only a system
// context view scoped to it, a view that is not "its view" (container of
// a system, component of a container); stubbed as: only a container view
// (for a system) or a component view (for a container) counts
// (`enterable`, `view_into`) (docs/deferred.md row).

use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;
use std::sync::Arc;

use c4rizer_core::edit::{Edit, RelationshipRef, StyleProperty, StyleTarget, TextTarget};
use c4rizer_core::geom::{Point, Rect};
use c4rizer_core::lint::Finding;
use c4rizer_core::model::{Colour, ElementId, ElementKind, Model, ModelError, Parsed, Shape, ViewKind};
use c4rizer_core::pipeline::ViewBuild;
use c4rizer_core::scene::{ItemKey, Scene};
use c4rizer_core::view::{BoxId, PlanBox};

use crate::camera::{Camera, ScenePoint, ScreenPoint};
use crate::files::{ContentHash, FileEvent, UiState, WriteRequest};
use crate::history::{History, Step};
use crate::worker::{ExportFormat, Generation, Reply, Request, ViewRef};

/// Pointer travel below which a press and release is a click, in screen
/// px (EDIT-28).
pub const CLICK_SLOP_PX: f32 = 4.0;

/// Hit tolerance for lines and boundary outlines, in scene px
/// (`Scene::hit`) (EDIT-28).
pub const HIT_TOLERANCE_PX: i32 = 4;

/// Half the side of a resize handle, in screen px (EDIT-18).
pub const HANDLE_PX: f32 = 4.0;

/// A pointer button (EDIT-28).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Button {
    /// Primary.
    Left,
    /// Wheel button: pans.
    Middle,
    /// Secondary: selects without entering.
    Right,
}

/// Keys the editor reacts to (EDIT-5, EDIT-16, EDIT-19, EDIT-28).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Key {
    /// Edit text (EDIT-5); confirm while editing.
    Enter,
    /// Edit text (EDIT-5).
    F2,
    /// Next field while editing (EDIT-5).
    Tab,
    /// Cancel editing, else deselect (EDIT-5, EDIT-28).
    Escape,
    /// Parent crumb unless a text field has focus (EDIT-16).
    Backspace,
    /// With Alt: parent crumb (EDIT-16).
    ArrowLeft,
    /// Held: drag pans (EDIT-28).
    Space,
    /// With Ctrl: undo; with Ctrl+Shift: redo (EDIT-19).
    Z,
    /// With Ctrl: redo (EDIT-19).
    Y,
}

/// Modifier keys held during an event (EDIT-28).
// DECISION: repr(C) and pinned: carried by every pointer event.
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub struct Modifiers {
    /// Shift: toggle selection, never enter.
    pub shift: bool,
    /// Ctrl (Cmd on macOS): select without entering; undo/redo chords.
    pub ctrl: bool,
    /// Alt: Alt+Left goes to the parent crumb.
    pub alt: bool,
}

/// Raw input from the canvas and keyboard, already in screen px
/// (EDIT-28). Serves EDIT-2, EDIT-9, EDIT-10, EDIT-15, EDIT-16, EDIT-18,
/// EDIT-20, EDIT-28.
// DECISION: hot, no heap data, size pinned (16 bytes); text goes through
// `Event::Text`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Input {
    /// A button went down over the canvas.
    PointerDown {
        /// Where.
        at: ScreenPoint,
        /// Which button.
        button: Button,
        /// Modifiers held.
        mods: Modifiers,
    },
    /// The pointer moved over the canvas (with or without a button down).
    PointerMove {
        /// Where.
        at: ScreenPoint,
    },
    /// A button went up.
    PointerUp {
        /// Where.
        at: ScreenPoint,
        /// Which button.
        button: Button,
    },
    /// The pointer left the canvas: hover cleared.
    PointerLeft,
    /// Wheel: zoom at the cursor (EDIT-9).
    Wheel {
        /// The cursor.
        at: ScreenPoint,
        /// Notches, positive zooms in.
        notches: f32,
    },
    /// A key went down.
    KeyDown {
        /// Which key.
        key: Key,
        /// Modifiers held.
        mods: Modifiers,
        /// True when a text field of the shell has focus (EDIT-16:
        /// Backspace is then ignored).
        text_focus: bool,
    },
    /// A key went up (Space ends Space+drag panning).
    KeyUp {
        /// Which key.
        key: Key,
    },
}

/// Box alignment (EDIT-20).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Alignment {
    /// Left edges.
    Left,
    /// Vertical centre line.
    Centre,
    /// Right edges.
    Right,
    /// Top edges.
    Top,
    /// Horizontal middle line.
    Middle,
    /// Bottom edges.
    Bottom,
}

/// Distribution axis (EDIT-20).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Axis {
    /// Even horizontal spacing.
    Horizontal,
    /// Even vertical spacing.
    Vertical,
}

/// Panel, menu and dialog actions. Serves EDIT-6, EDIT-7, EDIT-8,
/// EDIT-13, EDIT-16, EDIT-17, EDIT-19, EDIT-20.
#[derive(Debug, Clone, PartialEq)]
pub enum Command {
    /// Open a diagram file: argument, recent file or Open dialog result
    /// (EDIT-13).
    Open(PathBuf),
    /// Show the Open dialog (EDIT-13).
    ShowOpenDialog,
    /// Show a view from the view list (EDIT-8).
    ShowView(ViewRef),
    /// Open the `n`-th crumb of `State::breadcrumb` (EDIT-16): the
    /// first declared view scoped to the crumb's element (container view
    /// of a system, component view of a container), else its default
    /// view, as `view_into` gives it; the workspace crumb as the
    /// `TODO(srs)` in this module's header says.
    Crumb(usize),
    /// Set a style property on every selected box, one undo step
    /// (EDIT-17, DSL-13).
    SetStyle {
        /// `all <tag>` or `this box only`.
        target: StyleTarget,
        /// The property and value.
        property: StyleProperty,
    },
    /// "Reset to tag style" on every selected box (DSL-13).
    ResetStyle,
    /// Align the selected boxes, one undo step (EDIT-20).
    Align(Alignment),
    /// Distribute the selected boxes evenly, one undo step (EDIT-20).
    Distribute(Axis),
    /// Undo (EDIT-19).
    Undo,
    /// Redo (EDIT-19).
    Redo,
    /// "Export view": shows the save dialog (EDIT-7).
    ExportView(ExportFormat),
    /// "Export all views": shows the folder dialog (EDIT-7).
    ExportAll(ExportFormat),
    /// The export dialog's answer: write these views to these files
    /// (EDIT-7).
    Export {
        /// View and file per target.
        targets: Vec<(ViewRef, PathBuf)>,
        /// SVG or PNG.
        format: ExportFormat,
    },
    /// Select the items of the `n`-th finding of `State::findings`
    /// (EDIT-6).
    PickFinding(usize),
}

/// Everything that goes into the state machine.
#[derive(Debug, Clone)]
pub enum Event {
    /// Canvas and keyboard input.
    Input(Input),
    /// The in-place text field's content changed: the whole new buffer
    /// of the field with focus (EDIT-5).
    Text(String),
    /// Panel, menu and dialog actions.
    Command(Command),
    /// A file event (open, watch, write results).
    File(FileEvent),
    /// A worker reply.
    Worker(Reply),
}

/// A dialog the shell must show; its answer comes back as a `Command`.
/// Serves EDIT-7, EDIT-13.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Dialog {
    /// Open (EDIT-13), starting in `dir`.
    Open {
        /// The current file's folder, if a file is open.
        dir: Option<PathBuf>,
    },
    /// Save one view (EDIT-7): answered by `Command::Export` with one
    /// target.
    ExportView {
        /// The view.
        view: ViewRef,
        /// The diagram file's folder (`files::export_dir`).
        dir: PathBuf,
        /// `<view key>.<ext>` (`files::export_name`).
        name: String,
        /// SVG or PNG.
        format: ExportFormat,
    },
    /// Pick a folder for every declared view (EDIT-7): answered by
    /// `Command::Export` with `<folder>/<key>.<ext>` per view.
    ExportAll {
        /// The diagram file's folder.
        dir: PathBuf,
        /// SVG or PNG.
        format: ExportFormat,
    },
}

/// Everything that comes out of the state machine, executed by the shell
/// in order.
#[derive(Debug, Clone)]
pub enum Effect {
    /// Read the file and answer `FileEvent::Loaded` or `OpenFailed`
    /// (EDIT-13).
    Read(PathBuf),
    /// Start watching this file's directory, replacing any watcher
    /// (EDIT-1, EDIT-23).
    Watch(PathBuf),
    /// `files::write_checked` (EDIT-3, EDIT-22, EDIT-24).
    Write(WriteRequest),
    /// Send to the worker (EDIT-25).
    Work(Request),
    /// Show a dialog (EDIT-7, EDIT-13).
    Dialog(Dialog),
    /// Save the per-user state (EDIT-13).
    SaveUiState(UiState),
}

/// A selected item, by the keys in the file so it survives a reload
/// (EDIT-1, EDIT-10).
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Selected {
    /// A box, by its element's canonical key.
    Element(String),
    /// A line or its label, by its winning relationship.
    Relationship(RelationshipRef),
    /// The view title (EDIT-5).
    Title,
}

/// A resize handle of a box: its sides and corners (EDIT-18).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Handle {
    /// Top side.
    N,
    /// Bottom side.
    S,
    /// Right side.
    E,
    /// Left side.
    W,
    /// Top right.
    NE,
    /// Top left.
    NW,
    /// Bottom right.
    SE,
    /// Bottom left.
    SW,
}

/// What the pointer is doing (EDIT-28).
#[derive(Debug, Clone, PartialEq)]
pub enum Gesture {
    /// No button down.
    Idle,
    /// A button is down and the pointer has not yet travelled
    /// `CLICK_SLOP_PX`: a click on release (EDIT-28).
    Pressed {
        /// Where it went down.
        at: ScreenPoint,
        /// Which button.
        button: Button,
        /// Modifiers at the press.
        mods: Modifiers,
        /// The item under the press, in the scene shown then.
        hit: Option<ItemKey>,
        /// The resize handle under the press, if any.
        handle: Option<(BoxId, Handle)>,
    },
    /// Left drag on a box: the selected boxes move together (EDIT-2,
    /// EDIT-20).
    Dragging {
        /// Where the drag started, screen px.
        from: ScreenPoint,
        /// Every moving box and its centre at the start, grid units.
        boxes: BTreeMap<BoxId, Point>,
        /// Their current snapped centres, grid units (the last sent to
        /// the worker).
        to: BTreeMap<BoxId, Point>,
    },
    /// Left drag on a handle (EDIT-18).
    Resizing {
        /// The box.
        id: BoxId,
        /// The handle.
        handle: Handle,
        /// Where the drag started, screen px.
        from: ScreenPoint,
        /// The current size, px, multiples of 20, never below
        /// `min_size`.
        size: (i32, i32),
    },
    /// Left drag on empty canvas (EDIT-20, EDIT-28).
    RubberBand {
        /// Start, screen px.
        from: ScreenPoint,
        /// Current corner, screen px.
        to: ScreenPoint,
        /// Shift held at the start: add to the selection.
        add: bool,
    },
    /// Middle drag or Space+drag (EDIT-9, EDIT-28).
    Panning {
        /// The last pointer position, screen px.
        last: ScreenPoint,
    },
}

/// One field of the in-place text editor (EDIT-5).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TextField {
    /// The literal it writes.
    pub target: TextTarget,
    /// Shown above the field: `name`, `description`, `technology`,
    /// `title`, `subtitle`.
    pub label: &'static str,
    /// The value in the file when editing began; the `expected` of the
    /// written `SetText` (EDIT-24).
    pub original: Option<String>,
    /// The text typed so far.
    pub buffer: String,
}

/// The in-place text editor (EDIT-5): the fields of the selected box
/// (name, description, technology), line (description, technology; of
/// the relationship it was lifted from) or view (title, subtitle).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TextEdit {
    /// The item edited, for placing the field on the canvas.
    pub item: Selected,
    /// Its fields, in Tab order.
    pub fields: Vec<TextField>,
    /// The field with focus.
    pub current: usize,
}

/// The open diagram file (EDIT-1, EDIT-11, EDIT-27).
#[derive(Debug, Clone)]
pub struct Doc {
    /// The path as opened.
    pub path: PathBuf,
    /// The last parse that succeeded; `None` when the file has never
    /// parsed since it was opened (EDIT-11: empty canvas).
    pub parsed: Option<Arc<Parsed>>,
    /// Hash of the text on disk as last read or written (EDIT-22,
    /// EDIT-24).
    pub hash: ContentHash,
    /// The parse error of the text on disk, if it does not parse
    /// (EDIT-11): read-only until the next good parse. The only copy;
    /// `State::status` derives `Problem::Parse` from it.
    pub parse_error: Option<ModelError>,
    /// True while the file is gone (EDIT-27): read-only until it
    /// reappears. The only copy; `State::status` derives
    /// `Problem::Missing` from it.
    pub missing: bool,
}

/// A user action that writes to the file, kept until its write lands so
/// it can be re-applied to a newer text (EDIT-24). Named by keys; every
/// layout and style action names the view it was made in, so `edits_for`
/// never depends on the view on screen (EDIT-15, EDIT-24).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Action {
    /// Move boxes to these centres, px, on the grid (EDIT-3, EDIT-20).
    Move {
        /// The view the boxes were moved in.
        view: ViewRef,
        /// Canonical element key to centre `(x, y)` px.
        boxes: BTreeMap<String, (i32, i32)>,
        /// Every box of `view` at its centre, px, as shown when the
        /// gesture ended: the `at` lines of an EDIT-26 first write.
        layout: BTreeMap<String, (i32, i32)>,
    },
    /// Resize one box, px `(width, height)` (EDIT-18).
    Resize {
        /// The view the box was resized in.
        view: ViewRef,
        /// Canonical element key.
        element: String,
        /// New size, px, multiples of 20.
        size: (i32, i32),
        /// Every box of `view` at its centre, px, as shown when the
        /// gesture ended: the `at` lines of an EDIT-26 first write.
        layout: BTreeMap<String, (i32, i32)>,
    },
    /// Write text fields, one batch (EDIT-5).
    SetText {
        /// Target, new value, value expected now (EDIT-24).
        fields: Vec<(TextTarget, Option<String>, Option<String>)>,
    },
    /// Style change on several boxes, one batch (EDIT-17).
    SetStyle {
        /// The view the change was made in (EDIT-15: a default view is
        /// written first).
        view: ViewRef,
        /// Canonical element keys.
        elements: Vec<String>,
        /// `all <tag>` or `this box only`.
        target: StyleTarget,
        /// Property and value.
        property: StyleProperty,
    },
    /// Reset to tag style on several boxes (DSL-13).
    ResetStyle {
        /// The view the change was made in (EDIT-15).
        view: ViewRef,
        /// Canonical element keys.
        elements: Vec<String>,
    },
    /// Apply an undo step (EDIT-19).
    Undo(Step),
    /// Apply a redo step (EDIT-19).
    Redo(Step),
}

/// A write in flight: the action, and the inverse its batch produced,
/// recorded in `History` once `FileEvent::Written` arrives (EDIT-19,
/// EDIT-24).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Pending {
    /// What the user did.
    pub action: Action,
    /// `edit::Applied::inverse` of the batch written.
    pub inverse: Vec<Edit>,
    /// The text written, to adopt as the new file state on `Written`.
    pub text: String,
}

/// What the status bar shows (EDIT-11, EDIT-19, EDIT-24, EDIT-27), as
/// `State::status` derives it; never stored.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Status {
    /// A one-off notice: a step skipped, an action dropped, export done
    /// (`State::notice`).
    pub notice: Option<String>,
    /// The current problem with the file, shown until it clears: the
    /// parse error, else the missing file, else the I/O error.
    pub problem: Option<Problem>,
}

/// A problem with the open file (EDIT-11, EDIT-27).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Problem {
    /// The file does not parse; `ModelError`'s `Display` gives line and
    /// column (EDIT-11).
    Parse(ModelError),
    /// The file is gone (EDIT-27).
    Missing,
    /// A read, write or watch failed, in one sentence (EDIT-27).
    Io(String),
}

/// How an item is marked on the canvas: the only things drawn that are
/// not in the scene (EDIT-12).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum MarkKind {
    /// Selected (EDIT-10, EDIT-20).
    Selected,
    /// Selected, the only selected box, editable: with resize handles
    /// (EDIT-18).
    SelectedWithHandles,
    /// Under the pointer.
    Hover,
    /// Under the pointer and enterable (EDIT-15).
    Enterable,
}

/// One mark on one scene item (EDIT-12).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Mark {
    /// The item, in `DrawList::scene`.
    pub item: ItemKey,
    /// How it is marked.
    pub kind: MarkKind,
}

/// What the canvas paints this frame: the scene and the marks, nothing
/// else (EDIT-12).
#[derive(Debug, Clone, PartialEq)]
pub struct DrawList<'a> {
    /// The scene shown (the drag preview while dragging); `None` before
    /// the first build or when the file never parsed (EDIT-11).
    pub scene: Option<&'a Scene>,
    /// Pan and zoom.
    pub camera: Camera,
    /// Selection and hover marks.
    pub marks: Vec<Mark>,
    /// The rubber band, screen px corners (EDIT-20).
    pub rubber_band: Option<(ScreenPoint, ScreenPoint)>,
    /// While resizing, the box's outline at the size it will get on
    /// release, screen px corners (EDIT-18): a mark, not a scene item.
    pub resize_outline: Option<(ScreenPoint, ScreenPoint)>,
    /// The item whose text is being edited: the shell overlays the field
    /// on its bounds (EDIT-5).
    pub editing: Option<ItemKey>,
    /// True when edits are refused (EDIT-11, EDIT-27): no handles.
    pub read_only: bool,
}

/// One entry of the view list (EDIT-8).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ViewEntry {
    /// The view.
    pub view: ViewRef,
    /// Its drawn title, else its key.
    pub label: String,
    /// Its kind.
    pub kind: ViewKind,
    /// True for the view on screen.
    pub current: bool,
}

/// One crumb of the breadcrumb (EDIT-16).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Crumb {
    /// The workspace name, or the element's name.
    pub label: String,
    /// The element's canonical key; `None` for the workspace.
    pub scope: Option<String>,
}

/// The model details of the single selected item (EDIT-10, EDIT-5).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Details {
    /// What is selected.
    pub subject: Selected,
    /// Name (element), description (relationship) or title (view).
    pub name: String,
    /// The type: `Person`, `Software System`, `External Software
    /// System`, `Container`, `Component`, `Relationship`, `View`.
    pub kind: String,
    /// Description, if any.
    pub description: Option<String>,
    /// Technology, if any.
    pub technology: Option<String>,
    /// Effective tags, model order.
    pub tags: Vec<String>,
    /// For a lifted line: the relationship it was lifted from, as
    /// `<source name> -> <destination name>` (EDIT-5).
    pub lifted_from: Option<String>,
    /// The other relationships merged into the line, one per entry, same
    /// form (EDIT-5, CORE-5).
    pub merged: Vec<String>,
}

/// The style panel for the selected boxes (EDIT-17).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StylePanel {
    /// Fill shared by every selected box, `None` when they differ.
    pub background: Option<Colour>,
    /// Text colour shared, `None` when they differ.
    pub color: Option<Colour>,
    /// Shape shared, `None` when they differ.
    pub shape: Option<Shape>,
    /// Tags every selected box carries: the `all <tag>` choices (DSL-13).
    pub tags: Vec<String>,
    /// Number of boxes the change applies to.
    pub boxes: usize,
}

/// The editor's whole state. Fields are plain data; only `handle`
/// changes them. Serves EDIT-1..29.
#[derive(Debug, Clone)]
pub struct State {
    /// Per-user state: recent files, per-file view and camera (EDIT-13).
    pub ui: UiState,
    /// The open file, `None` before the first open.
    pub doc: Option<Doc>,
    /// The view on screen (or being built).
    pub view: Option<ViewRef>,
    /// The last full build of `view`, with the parse it came from (for
    /// `pipeline::keep_across_reparse`, CORE-29).
    pub shown: Option<(Arc<Parsed>, Arc<ViewBuild>)>,
    /// The last `Reply::Dragged` build of the current drag (its scene
    /// with re-routed lines, and its lint report); `None` when not
    /// dragging or before the first reply. Cleared on release and on any
    /// `Built` of the shown view (EDIT-6, EDIT-25).
    pub dragged: Option<Arc<ViewBuild>>,
    /// The drag preview scene: `dragged`'s scene (else `shown`'s) with
    /// every dragged box moved by `Scene::move_box` to its current
    /// centre, so the box moves at once and its lines follow when the
    /// next `Dragged` reply lands (EDIT-25).
    pub preview: Option<Scene>,
    /// Background builds of the other views of the current parse
    /// (EDIT-14), cleared on each reload.
    pub cache: BTreeMap<ViewRef, Arc<ViewBuild>>,
    /// Pan and zoom.
    pub camera: Camera,
    /// The selection.
    pub selection: BTreeSet<Selected>,
    /// The item under the pointer, in the scene shown.
    pub hover: Option<ItemKey>,
    /// The pointer gesture.
    pub gesture: Gesture,
    /// True while Space is held (EDIT-28 Space+drag).
    pub space: bool,
    /// The in-place text editor, when open (EDIT-5).
    pub text_edit: Option<TextEdit>,
    /// Undo and redo (EDIT-19).
    pub history: History,
    /// The write in flight (EDIT-24).
    pub pending: Option<Pending>,
    /// The last generation issued (EDIT-25).
    pub generation: Generation,
    /// The generation of the newest shown `Build` request; older `Built`
    /// replies for the shown view are dropped.
    pub awaited_build: Option<Generation>,
    /// The generation of the newest `Drag` request; older `Dragged`
    /// replies are dropped.
    pub awaited_drag: Option<Generation>,
    /// The one-off status notice (EDIT-19, EDIT-24), cleared by the next
    /// user action.
    pub notice: Option<String>,
    /// The last read, write or watch failure, in one sentence (EDIT-27),
    /// cleared by the next successful read or write.
    pub io_error: Option<String>,
}

impl State {
    /// A fresh state with no file, holding the loaded per-user state
    /// (EDIT-13).
    #[must_use]
    pub fn new(ui: UiState) -> State {
        todo!()
    }

    /// Feeds one event and returns the effects to execute, in order.
    /// Covers the whole input table of EDIT-28, the commands, the file
    /// events (EDIT-1, 11, 22..24, 27) and worker replies (EDIT-25).
    /// Never blocks and never routes (EDIT-25). Serves EDIT-1..29.
    pub fn handle(&mut self, event: Event) -> Vec<Effect> {
        todo!()
    }

    /// The scene on screen: the drag preview, else the shown build's.
    /// Serves EDIT-12, EDIT-25.
    #[must_use]
    pub fn scene(&self) -> Option<&Scene> {
        todo!()
    }

    /// What the canvas paints: the scene and the selection and hover
    /// marks only (EDIT-12).
    #[must_use]
    pub fn draw_list(&self) -> DrawList<'_> {
        todo!()
    }

    /// Every view of the file in declaration order, plus the open default
    /// view when one is shown (EDIT-8, EDIT-15). Empty with no parse.
    #[must_use]
    pub fn views(&self) -> Vec<ViewEntry> {
        todo!()
    }

    /// The breadcrumb from the open view's scope in the model: workspace,
    /// then the scope's ancestors and the scope (EDIT-16).
    #[must_use]
    pub fn breadcrumb(&self) -> Vec<Crumb> {
        todo!()
    }

    /// The details of the single selected item, `None` for no selection
    /// or several items (EDIT-10, EDIT-5).
    #[must_use]
    pub fn details(&self) -> Option<Details> {
        todo!()
    }

    /// The style panel for the selected boxes, `None` when no box is
    /// selected (EDIT-17, EDIT-10).
    #[must_use]
    pub fn style_panel(&self) -> Option<StylePanel> {
        todo!()
    }

    /// The lint findings of the scene on screen, updated with each render
    /// (EDIT-6): `dragged`'s report while dragging and one has arrived,
    /// else the shown build's; empty with no build.
    #[must_use]
    pub fn findings(&self) -> &[Finding] {
        todo!()
    }

    /// The status bar (EDIT-11, EDIT-19, EDIT-24, EDIT-27), derived from
    /// `notice`, `Doc::parse_error`, `Doc::missing` and `io_error`.
    #[must_use]
    pub fn status(&self) -> Status {
        todo!()
    }

    /// True when edits are refused: no parse, a parse error, the file
    /// missing (EDIT-11, EDIT-27). Undo and redo are refused too.
    #[must_use]
    pub fn read_only(&self) -> bool {
        todo!()
    }

    /// The `edit` batch for `action` against `parsed`, the text it will
    /// be applied to (EDIT-24 calls it again on the newer text). Reads
    /// only `action` and `parsed`, never `self.view` or the shown scene,
    /// so a view switch while the write is in flight changes nothing:
    /// - when the action's view is a `ViewRef::Default` not declared in
    ///   `parsed` (EDIT-15), a layout or style write starts with
    ///   `Edit::AddDefaultView` for that view's scope and kind, and its
    ///   other edits target `ViewRef::key`;
    /// - a move or resize whose view has no `at` line for any of its
    ///   boxes in `parsed` is the first write (EDIT-26): one `Edit::Move`
    ///   per entry of the action's `layout` (the moved boxes at their new
    ///   centres), in `ElementId` order of `parsed`, entries whose element
    ///   is gone skipped, then the action's own edits;
    /// - otherwise the action's edits only (`Move` per moved box,
    ///   `Resize`, `SetText`, `SetStyle` per element, `ResetStyle` per
    ///   element); `Undo`/`Redo` give their step's edits unchanged, except
    ///   that a recorded EDIT-26 batch is rebuilt when its view lost every
    ///   `at` line (edit.rs header).
    ///
    /// Never yields an edit that creates or deletes an element or
    /// relationship (EDIT-21). Serves EDIT-3, EDIT-15, EDIT-19, EDIT-24,
    /// EDIT-26.
    #[must_use]
    pub fn edits_for(&self, action: &Action, parsed: &Parsed) -> Vec<Edit> {
        todo!()
    }
}

/// A centre in grid units from a pointer position in scene px, snapped to
/// the 10px grid (EDIT-2): `geom::snap_px_to_grid` per axis, in grid
/// units.
#[must_use]
pub fn snap_centre(scene_px: ScenePoint) -> Point {
    todo!()
}

/// The smallest size, px `(width, height)` in multiples of 20, at which
/// `plan_box`'s wrapped texts fit (`scene::texts_fit`, CORE-30): the
/// floor of a resize (EDIT-18). Searches upward from 20x20; never larger
/// than the box's current size when that already fails (the box may keep
/// its size, scene.rs `texts_fit`).
#[must_use]
pub fn min_size(plan_box: &PlanBox) -> (i32, i32) {
    todo!()
}

/// The resize handle of `rect` (scene px) under `at` (screen px) through
/// `camera`, within `HANDLE_PX`; corners win over sides (EDIT-18).
#[must_use]
pub fn handle_at(rect: Rect, camera: &Camera, at: ScreenPoint) -> Option<Handle> {
    todo!()
}

/// New centres, grid units, aligning `boxes` (id to rect in scene px) as
/// `how` says (EDIT-20); a single box or none gives an empty map.
#[must_use]
pub fn align(boxes: &BTreeMap<BoxId, Rect>, how: Alignment) -> BTreeMap<BoxId, Point> {
    todo!()
}

/// New centres, grid units, spacing `boxes` evenly along `axis`, the two
/// outermost kept (EDIT-20); fewer than three boxes give an empty map.
#[must_use]
pub fn distribute(boxes: &BTreeMap<BoxId, Rect>, axis: Axis) -> BTreeMap<BoxId, Point> {
    todo!()
}

/// True when `element` is enterable (EDIT-15): it has a child in the
/// model, or `view_into` finds a declared view of the counting kind
/// scoped to it (a container view for a system, a component view for a
/// container; a system context view does not count, `TODO(srs)` in this
/// module's header).
#[must_use]
pub fn enterable(model: &Model, element: ElementId) -> bool {
    todo!()
}

/// The view a click on enterable `element` opens (EDIT-15): the first
/// declared view scoped to it of the counting kind only (a container
/// view for a system, a component view for a container; a system
/// context view scoped to a system is ignored here), else its default
/// view (`ViewRef::Default`, `Container` for a system, `Component` for a
/// container). The other declared views scoped to it stay in the view
/// list (EDIT-8). `None` for a person, a component, or an element out of
/// range. `Command::Crumb` uses it too.
#[must_use]
pub fn view_into(model: &Model, element: ElementId) -> Option<ViewRef> {
    todo!()
}

/// The view shown on opening `model` (EDIT-13, EDIT-15, EDIT-27):
/// `remembered` if it still exists (declared, or a default view that
/// still opens), else the first declared view, else (a model-only file)
/// the system context default view of the first software system; `None`
/// when the model has no software system.
#[must_use]
pub fn initial_view(model: &Model, remembered: Option<&ViewRef>) -> Option<ViewRef> {
    todo!()
}

/// The text fields of `selected` in Tab order, with the values in
/// `model` (EDIT-5): element name, description, technology (the last for
/// containers and components only); relationship description,
/// technology (of the winning relationship, so a lifted line edits the
/// relationship it was lifted from); view title, subtitle of
/// `ViewRef::Declared` only: none for a `ViewRef::Default` not yet
/// written (`TODO(srs)` in this module's header).
#[must_use]
pub fn text_fields(model: &Model, view: &ViewRef, selected: &Selected) -> Vec<TextField> {
    todo!()
}

/// The element kind's type line for the details panel (EDIT-10):
/// `Person`, `Software System`, `External Software System`, `Container`,
/// `Component`.
#[must_use]
pub fn kind_label(kind: ElementKind, external: bool) -> &'static str {
    todo!()
}

#[cfg(test)]
mod tests {
    // Layout pins for the hot structs.

    #[test]
    fn input_is_16_bytes() {
        assert_eq!(std::mem::size_of::<super::Input>(), 16);
    }

    #[test]
    fn modifiers_is_3_bytes() {
        assert_eq!(std::mem::size_of::<super::Modifiers>(), 3);
    }

    // One test per EDIT requirement served by `state` (plan names where
    // the plan gives one). Each drives `State::handle` with a temp-dir
    // file, `files` and `worker::serve`, no window (HLD §6).

    /// EDIT-1 (plan editor-06): an external save reloads the view, keeping
    /// the selected view, zoom, pan and selection.
    #[test]
    fn edit_1_external_save_reloads_keeping_view() {
        todo!()
    }

    /// EDIT-2 (plan editor-04): dragging snaps the box centre to the 10px
    /// grid and sends `Drag` requests while dragging.
    #[test]
    fn edit_2_drag_snaps_to_grid() {
        todo!()
    }

    /// EDIT-3 (plan editor-04): release writes the box's `at` line
    /// (DSL-9), the file holds it, the next reload shows it there.
    #[test]
    fn edit_3_release_writes_at_line() {
        todo!()
    }

    /// EDIT-4: an auto-placed box is dragged, released, written, and
    /// keeps the dragged position after the reload.
    #[test]
    fn edit_4_auto_placed_box_keeps_dragged_position() {
        todo!()
    }

    /// EDIT-5 (plan editor-05): Enter on a selected box opens the name
    /// field, Tab moves to description then technology, Enter writes
    /// (DSL-10), Escape cancels; on a lifted line it edits the
    /// relationship it was lifted from.
    #[test]
    fn edit_5_enter_edits_text() {
        todo!()
    }

    /// EDIT-5: the details panel of a lifted line names its relationship
    /// and lists the merged ones.
    #[test]
    fn edit_5_lifted_line_details_name_relationship() {
        todo!()
    }

    /// EDIT-6 (plan editor-07): findings follow each render, the drag
    /// preview's included (`State::dragged`'s report while dragging);
    /// picking one selects its items.
    #[test]
    fn edit_6_lint_findings_update() {
        todo!()
    }

    /// EDIT-7 (plan editor-07 `edit_7_export_svg_and_png`, SVG half):
    /// "Export view" asks the save dialog with the diagram's folder and
    /// `<key>.svg`; the answer sends an `Export` job; the file written
    /// equals `export::svg` of the shown scene.
    #[test]
    fn edit_7_export_svg() {
        todo!()
    }

    /// EDIT-7 (PNG half): `ExportFormat::Png` asks for `<key>.png`, and
    /// the job writes `export::png` at twice the scene width. Needs core
    /// feature `png` on the editor's `c4rizer-core` dependency and the
    /// bundled font bytes (docs/deferred.md row "editor: EDIT-7 PNG and
    /// canvas font").
    #[test]
    #[ignore = "deferred"]
    fn edit_7_export_png() {
        todo!()
    }

    /// EDIT-8: every view is listed; picking one shows it.
    #[test]
    fn edit_8_views_listed_and_picked() {
        todo!()
    }

    /// EDIT-9 (plan editor-03): wheel zooms at the cursor and middle drag
    /// pans; the scene is unchanged.
    #[test]
    fn edit_9_zoom_keeps_diagram() {
        todo!()
    }

    /// EDIT-10 (plan editor-04): Ctrl-click and right-click select without
    /// entering; details and style panel show the selected box.
    #[test]
    fn edit_10_ctrl_click_selects() {
        todo!()
    }

    /// EDIT-11 (plan editor-06): a parse error keeps the last good scene,
    /// shows line and column, refuses every edit and undo; the next good
    /// parse resumes editing; opening a broken file shows an empty canvas
    /// and still reloads.
    #[test]
    fn edit_11_parse_error_keeps_last_good() {
        todo!()
    }

    /// EDIT-13 (plan editor-01): open remembers the file in `recent`,
    /// restores the last view if it still exists, else the first
    /// declared.
    #[test]
    fn edit_13_open_restores_last_view() {
        todo!()
    }

    /// EDIT-14: on open, the shown view's build is requested before any
    /// background build of the other views.
    #[test]
    fn edit_14_shown_view_requested_first() {
        todo!()
    }

    /// EDIT-15 (plan editor-03): a click on an enterable system opens its
    /// container view; a non-enterable box is selected; hover marks
    /// enterable boxes.
    #[test]
    fn edit_15_click_system_opens_container_view() {
        todo!()
    }

    /// EDIT-15 (plan editor-03): with no view scoped to the box, its
    /// default view opens; the first write in it starts with
    /// `AddDefaultView`; undoing that write removes it again.
    #[test]
    fn edit_15_default_view_when_none() {
        todo!()
    }

    /// EDIT-15: with several container views scoped to a system, a click
    /// opens the first declared one and the view list shows the others;
    /// a childless system with only a system context view scoped to it
    /// is not enterable (`TODO(srs)` in this module's header).
    #[test]
    fn edit_15_several_views_first_declared_opens() {
        todo!()
    }

    /// EDIT-15 + EDIT-24: a move in a default view, then a switch to
    /// another view before `ChangedOnDisk` arrives: the re-applied batch
    /// still starts with `AddDefaultView` for the view the move was made
    /// in and moves its boxes, not the new view's.
    #[test]
    fn edit_15_reapply_after_view_switch_targets_action_view() {
        todo!()
    }

    /// EDIT-15: a model-only file shows the system context default view
    /// of its first software system.
    #[test]
    fn edit_15_model_only_file_shows_default_context_view() {
        todo!()
    }

    /// EDIT-16 (plan editor-03): the breadcrumb follows the scope in the
    /// model; Alt+Left and Backspace open the parent crumb; Backspace is
    /// ignored while a text field has focus.
    #[test]
    fn edit_16_back_goes_up() {
        todo!()
    }

    /// EDIT-17 (plan editor-05): the style panel writes colour, text
    /// colour and shape for every selected box as one undo step, with the
    /// DSL-13 target.
    #[test]
    fn edit_17_style_panel_writes_style() {
        todo!()
    }

    /// EDIT-18: resizing by a handle snaps to 20px steps about the centre,
    /// never below `min_size`, shows `DrawList::resize_outline` while
    /// dragging, and writes only the `size` line.
    #[test]
    fn edit_18_resize_on_grid_never_below_text() {
        todo!()
    }

    /// EDIT-19 (plan editor-05): 100 writes undo and redo in order through
    /// external reloads; a step whose target is gone is skipped with a
    /// status notice; a multi-box move is one step.
    #[test]
    fn edit_19_undo_redo_100_steps() {
        todo!()
    }

    /// EDIT-20 (plan editor-04): Shift-click and rubber band select
    /// several boxes, which move together and align and distribute as one
    /// step.
    #[test]
    fn edit_20_align_and_distribute() {
        todo!()
    }

    /// EDIT-21: no `Input` or `Command` yields an edit that creates or
    /// deletes an element or relationship; `RemoveDefaultView` only
    /// appears as an undo of `AddDefaultView`.
    #[test]
    fn edit_21_no_gesture_creates_or_deletes() {
        todo!()
    }

    /// EDIT-22: the watcher's echo of the app's own write (same hash) does
    /// not reload.
    #[test]
    fn edit_22_own_write_not_reloaded() {
        todo!()
    }

    /// EDIT-23: a `Changed` after a rename save reloads like any save.
    #[test]
    fn edit_23_rename_save_reloads() {
        todo!()
    }

    /// EDIT-24 (plan editor-06): a write refused by the on-disk check
    /// reloads and re-applies the action to the new text; an action whose
    /// target is gone is dropped with a status notice.
    #[test]
    fn edit_24_changed_on_disk_reapplies() {
        todo!()
    }

    /// EDIT-25: 100 drag events with a stalled worker (no reply) each move
    /// the box at once and return without waiting; stale `Dragged`
    /// replies are dropped; after release the shown scene equals the
    /// build of the saved file.
    #[test]
    fn edit_25_drag_never_waits_on_worker() {
        todo!()
    }

    /// EDIT-26: the first move in a view writes an `at` line for every box
    /// as one step; the next move changes only the moved box's line.
    #[test]
    fn edit_26_first_write_writes_every_box() {
        todo!()
    }

    /// EDIT-27: the file disappearing keeps the last diagram read-only
    /// until it reappears; a failed write reverts the canvas and shows the
    /// error; a vanished open view falls back to the first declared one.
    #[test]
    fn edit_27_file_removed_keeps_last_read_only() {
        todo!()
    }

    /// EDIT-28 (plan Review Focus): a press and release moving under 4 px
    /// is a click (enters or selects), 4 px or more is a drag; Shift-click
    /// toggles and never enters; empty-canvas click and Escape deselect;
    /// Space+drag pans.
    #[test]
    fn edit_28_click_under_4px_enters_not_drags() {
        todo!()
    }

    /// EDIT-29: on `example/`, a `Changed` event to the scene handed to
    /// the canvas takes under 200 ms (release perf oracle).
    #[test]
    #[ignore = "perf: release perf oracle only"]
    fn perf_edit_29_reload_under_200ms() {
        todo!()
    }
}

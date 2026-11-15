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
// EDIT-28: Ctrl+Z (Cmd+Z on macOS) undoes; Ctrl+Shift+Z and Ctrl+Y redo
// (EDIT-19); menu commands do the same.
// EDIT-16: the workspace crumb, and Alt+Left or Backspace on a system
// context view, open the first declared system context view, else the
// first declared view (nothing when the file declares no view).
// EDIT-15: a title or subtitle edit is also a write that adds the default
// view, like a move, resize or style; a default view has no title line
// and its drawn title is the scope's name.
// EDIT-15: a box is enterable when its element has at least one child in
// the model (opening the default view when no view is scoped to it), or
// a container view scoped to it (for a system) or a component view
// scoped to it (for a container); a system context view does not count
// (`enterable`, `view_into`).

use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;
use std::sync::Arc;

use c4rizer_core::edit::{
    Edit, ElementField, RelationshipField, RelationshipRef, StyleProperty, StyleTarget, TextTarget,
    ViewField, apply,
};
use c4rizer_core::geom::{GRID_PX, Point, Rect, snap_px_to_grid};
use c4rizer_core::lint::Finding;
use c4rizer_core::model::{
    Colour, ElementId, ElementKind, Model, ModelError, Parsed, RelationshipId, Shape, ViewKind, parse,
};
use c4rizer_core::pipeline::{ViewBuild, default_view, keep_across_reparse};
use c4rizer_core::place::Keep;
use c4rizer_core::scene::{ItemKey, ModelId, Scene, texts_fit};
use c4rizer_core::view::{BoxId, PlanBox};

use crate::camera::{Camera, ScenePoint, ScreenPoint, ZOOM_STEP};
use crate::files::{ContentHash, FileEvent, FileMemory, UiState, WriteRequest, export_dir, export_name};
use crate::history::{History, Step};
use crate::worker::{ExportFormat, ExportTarget, Generation, Job, Reply, Request, ViewRef};

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
    /// view, as `view_into` gives it; the workspace crumb opens the first
    /// declared system context view, else the first declared view.
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
    // DECISION: the EDIT-30 camera bookkeeping is private fields, not
    // pub ones, so the step-4 contract (every pub field of `State`)
    // stays as it is; only `new` builds a `State`.
    /// The canvas size in screen px, once the shell reported it
    /// (EDIT-30 fit).
    canvas: Option<ScreenPoint>,
    /// The file and view `camera` belongs to (EDIT-30).
    camera_of: Option<(PathBuf, ViewRef)>,
    /// The camera each view shown in this session had when it was left
    /// (EDIT-30: coming back restores it).
    view_cameras: BTreeMap<(PathBuf, ViewRef), Camera>,
    /// True while the view on screen waits for its first fit (EDIT-30).
    fit_pending: bool,
}

impl State {
    /// A fresh state with no file, holding the loaded per-user state
    /// (EDIT-13).
    #[must_use]
    pub fn new(ui: UiState) -> State {
        State {
            ui,
            doc: None,
            view: None,
            shown: None,
            dragged: None,
            preview: None,
            cache: BTreeMap::new(),
            camera: Camera::default(),
            selection: BTreeSet::new(),
            hover: None,
            gesture: Gesture::Idle,
            space: false,
            text_edit: None,
            history: History::default(),
            pending: None,
            generation: Generation::default(),
            awaited_build: None,
            awaited_drag: None,
            notice: None,
            io_error: None,
            canvas: None,
            camera_of: None,
            view_cameras: BTreeMap::new(),
            fit_pending: false,
        }
    }

    /// Feeds one event and returns the effects to execute, in order.
    /// Covers the whole input table of EDIT-28, the commands, the file
    /// events (EDIT-1, 11, 22..24, 27) and worker replies (EDIT-25).
    /// Never blocks and never routes (EDIT-25). Serves EDIT-1..29.
    pub fn handle(&mut self, event: Event) -> Vec<Effect> {
        let before = self.camera;
        let effects = self.handle_event(event);
        self.settle_camera(before);
        effects
    }

    /// The canvas size in screen px, reported by the shell each frame; a
    /// view waiting for its first fit is fitted once it is known
    /// (EDIT-30).
    // DECISION: a crate-private method, not an `Event` variant, so the
    // pub `Event` enum is unchanged; like `io_error`, the shell writes it
    // directly.
    pub(crate) fn set_canvas_size(&mut self, size: ScreenPoint) {
        let old = self.canvas.replace(size);
        // DECISION: a fitted camera the user has not moved since (it
        // still equals the fit for the old size) follows a resize: tiling
        // window managers resize the window right after its first frame,
        // which would leave the first-show fit stale. Any pan or zoom
        // makes the camera the user's, and resizes leave it alone.
        if let (Some(old), Some(scene)) = (old, self.scene())
            && old != size
            && self.camera == Camera::fit(scene.bounds, old)
        {
            self.camera = Camera::fit(scene.bounds, size);
            self.remember();
        }
        self.settle_camera(self.camera);
    }

    /// "Fit view": the camera shows the whole scene on screen, drawing,
    /// title and key, with the fit margin, never above 100% (EDIT-30).
    /// Nothing happens before a scene and a canvas size exist.
    pub(crate) fn fit_view(&mut self) {
        if let (Some(scene), Some(canvas)) = (self.scene(), self.canvas) {
            self.camera = Camera::fit(scene.bounds, canvas);
            self.fit_pending = false;
            self.remember();
        }
    }

    /// Keeps one camera per view of the session (EDIT-30): when the file
    /// or view on screen changed, the old one's camera is kept and the new
    /// one's restored, or the new one is fitted once its scene and the
    /// canvas size are known. Reloads keep the view, so they keep the
    /// camera (EDIT-1). `before` is the camera before the event that may
    /// have changed the view (an open replaces `camera` itself).
    // DECISION: the first show of a view in a session fits even when the
    // per-user state remembers a camera for the file (EDIT-13): that
    // camera came from another window size, and the EDIT-30 defect was
    // exactly a remembered origin camera showing one box. Within a
    // session the per-view camera wins, which keeps EDIT-13's "zoom and
    // pan per file" on reopening.
    fn settle_camera(&mut self, before: Camera) {
        let now = match (&self.doc, &self.view) {
            (Some(doc), Some(view)) => Some((doc.path.clone(), view.clone())),
            _ => None,
        };
        if now != self.camera_of {
            if let Some(old) = self.camera_of.take() {
                self.view_cameras.insert(old, before);
            }
            match now.as_ref().and_then(|key| self.view_cameras.get(key)) {
                Some(camera) => {
                    self.camera = *camera;
                    self.fit_pending = false;
                    self.remember();
                }
                None => self.fit_pending = now.is_some(),
            }
            self.camera_of = now;
        }
        let shown = match (self.scene(), &self.view) {
            (Some(scene), Some(view)) => scene.key == view.key(),
            _ => false,
        };
        if self.fit_pending && shown {
            self.fit_view();
        }
    }

    /// `handle` before the camera is settled.
    fn handle_event(&mut self, event: Event) -> Vec<Effect> {
        match event {
            Event::Input(input) => self.input(input),
            Event::Text(text) => {
                if let Some(edit) = self.text_edit.as_mut()
                    && let Some(field) = edit.fields.get_mut(edit.current)
                {
                    field.buffer = text;
                }
                Vec::new()
            }
            Event::Command(command) => {
                self.notice = None;
                self.command(command)
            }
            Event::File(file) => self.file(file),
            Event::Worker(reply) => self.reply(reply),
        }
    }

    /// The scene on screen: the drag preview, else the shown build's.
    /// Serves EDIT-12, EDIT-25.
    #[must_use]
    pub fn scene(&self) -> Option<&Scene> {
        self.preview
            .as_ref()
            .or_else(|| self.shown.as_ref().map(|(_, b)| &b.scene))
    }

    /// What the canvas paints: the scene and the selection and hover
    /// marks only (EDIT-12).
    #[must_use]
    pub fn draw_list(&self) -> DrawList<'_> {
        let scene = self.scene();
        let read_only = self.read_only();
        let mut marks = Vec::new();
        let mut resize_outline = None;
        let mut editing = None;
        if let (Some(scene), Some(model)) = (scene, self.model()) {
            let items: Vec<ItemKey> = self
                .selection
                .iter()
                .flat_map(|s| items_of(scene, model, s))
                .collect();
            let only_box = items.len() == 1 && matches!(items[0], ItemKey::Box(_));
            for item in &items {
                let kind = if only_box && !read_only {
                    MarkKind::SelectedWithHandles
                } else {
                    MarkKind::Selected
                };
                marks.push(Mark { item: *item, kind });
            }
            if let Some(hover) = self.hover
                && !items.contains(&hover)
                && scene.item_bounds(hover).is_some()
            {
                let kind = match scene.model_id(hover) {
                    Some(ModelId::Element(e)) if matches!(hover, ItemKey::Box(_)) && enterable(model, e) => {
                        MarkKind::Enterable
                    }
                    _ => MarkKind::Hover,
                };
                marks.push(Mark { item: hover, kind });
            }
            if let Gesture::Resizing { id, size, .. } = &self.gesture
                && let Some(b) = scene.boxes.get(id.0 as usize)
            {
                let c = b.rect.centre();
                let (hw, hh) = (size.0 / 2, size.1 / 2);
                resize_outline = Some((
                    self.camera.to_screen(Point::new(c.x - hw, c.y - hh)),
                    self.camera.to_screen(Point::new(c.x + hw, c.y + hh)),
                ));
            }
            if let Some(edit) = &self.text_edit {
                editing = items_of(scene, model, &edit.item).into_iter().next();
            }
        }
        let rubber_band = match &self.gesture {
            Gesture::RubberBand { from, to, .. } => Some((*from, *to)),
            _ => None,
        };
        DrawList {
            scene,
            camera: self.camera,
            marks,
            rubber_band,
            resize_outline,
            editing,
            read_only,
        }
    }

    /// Every view of the file in declaration order, plus the open default
    /// view when one is shown (EDIT-8, EDIT-15). Empty with no parse.
    #[must_use]
    pub fn views(&self) -> Vec<ViewEntry> {
        let Some(model) = self.model() else {
            return Vec::new();
        };
        let mut out: Vec<ViewEntry> = model
            .views
            .iter()
            .map(|v| {
                let view = ViewRef::Declared(v.key.clone());
                ViewEntry {
                    current: self.view.as_ref() == Some(&view),
                    label: v.drawn_title().map_or_else(|| v.key.clone(), |t| t.value.clone()),
                    kind: v.kind,
                    view,
                }
            })
            .collect();
        if let Some(view @ ViewRef::Default { scope, kind }) = &self.view
            && model.view(&view.key()).is_none()
        {
            let label = model
                .find(scope)
                .map_or_else(|| view.key(), |id| model.element(id).name.value.clone());
            out.push(ViewEntry {
                view: view.clone(),
                label,
                kind: *kind,
                current: true,
            });
        }
        out
    }

    /// The breadcrumb from the open view's scope in the model: workspace,
    /// then the scope's ancestors and the scope (EDIT-16).
    #[must_use]
    pub fn breadcrumb(&self) -> Vec<Crumb> {
        let Some(model) = self.model() else {
            return Vec::new();
        };
        let mut out = vec![Crumb {
            label: model
                .name
                .as_ref()
                .map_or_else(|| "Workspace".to_owned(), |n| n.value.clone()),
            scope: None,
        }];
        let Some(scope) = self.view.as_ref().and_then(|v| view_scope(model, v)) else {
            return out;
        };
        let mut chain = Vec::new();
        let mut at = Some(scope);
        while let Some(id) = at {
            chain.push(id);
            at = model.element(id).parent;
        }
        for id in chain.into_iter().rev() {
            let e = model.element(id);
            out.push(Crumb {
                label: e.name.value.clone(),
                scope: e.key.as_ref().map(|k| k.value.clone()),
            });
        }
        out
    }

    /// The details of the single selected item, `None` for no selection
    /// or several items (EDIT-10, EDIT-5).
    #[must_use]
    pub fn details(&self) -> Option<Details> {
        if self.selection.len() != 1 {
            return None;
        }
        let model = self.model()?;
        let subject = self.selection.iter().next()?.clone();
        match &subject {
            Selected::Element(key) => {
                let e = model.element(model.find(key)?);
                Some(Details {
                    name: e.name.value.clone(),
                    kind: kind_label(e.kind, e.is_external()).to_owned(),
                    description: e.description.as_ref().map(|d| d.value.clone()),
                    technology: e.technology.as_ref().map(|t| t.value.clone()),
                    tags: e.tags.clone(),
                    lifted_from: None,
                    merged: Vec::new(),
                    subject,
                })
            }
            Selected::Relationship(r) => {
                let id = resolve_relationship(model, r)?;
                let rel = model.relationship(id);
                let arrow = |rid: RelationshipId| {
                    let x = model.relationship(rid);
                    format!(
                        "{} -> {}",
                        model.element(x.source.value).name.value,
                        model.element(x.destination.value).name.value
                    )
                };
                let mut lifted_from = None;
                let mut merged = Vec::new();
                if let Some(scene) = self.scene()
                    && let Some(line) = scene.lines.iter().find(|l| l.relationship == id)
                {
                    let ends = (
                        scene.boxes.get(line.source.0 as usize).map(|b| b.element),
                        scene.boxes.get(line.destination.0 as usize).map(|b| b.element),
                    );
                    if ends != (Some(rel.source.value), Some(rel.destination.value)) {
                        lifted_from = Some(arrow(id));
                    }
                    merged = line
                        .relationships
                        .iter()
                        .filter(|&&r| r != id)
                        .map(|&r| arrow(r))
                        .collect();
                }
                Some(Details {
                    name: rel.description.as_ref().map(|d| d.value.clone()).unwrap_or_default(),
                    kind: "Relationship".to_owned(),
                    description: rel.description.as_ref().map(|d| d.value.clone()),
                    technology: rel.technology.as_ref().map(|t| t.value.clone()),
                    tags: rel.tags.clone(),
                    lifted_from,
                    merged,
                    subject,
                })
            }
            Selected::Title => {
                let view = self.view.as_ref()?;
                let (name, description) = match model.view(&view.key()) {
                    Some(spec) => (
                        spec.drawn_title().map(|t| t.value.clone()).unwrap_or_default(),
                        spec.subtitle.as_ref().map(|s| s.value.clone()),
                    ),
                    None => (
                        view_scope(model, view).map(|id| model.element(id).name.value.clone()).unwrap_or_default(),
                        None,
                    ),
                };
                Some(Details {
                    name,
                    kind: "View".to_owned(),
                    description,
                    technology: None,
                    tags: Vec::new(),
                    lifted_from: None,
                    merged: Vec::new(),
                    subject,
                })
            }
        }
    }

    /// The style panel for the selected boxes, `None` when no box is
    /// selected (EDIT-17, EDIT-10).
    #[must_use]
    pub fn style_panel(&self) -> Option<StylePanel> {
        let model = self.model()?;
        let ids: Vec<ElementId> = self.selected_elements().iter().filter_map(|k| model.find(k)).collect();
        let first = *ids.first()?;
        let styles: Vec<_> = ids.iter().map(|&id| model.style_of(id)).collect();
        let shared = |f: &dyn Fn(&c4rizer_core::model::ResolvedStyle) -> String| {
            styles.windows(2).all(|w| f(&w[0]) == f(&w[1]))
        };
        let background = shared(&|s| s.background.to_string()).then_some(styles[0].background);
        let color = shared(&|s| s.color.to_string()).then_some(styles[0].color);
        let shape = styles.iter().all(|s| s.shape == styles[0].shape).then_some(styles[0].shape);
        let tags = model
            .element(first)
            .tags
            .iter()
            .filter(|t| ids.iter().all(|&id| model.element(id).has_tag(t)))
            .cloned()
            .collect();
        Some(StylePanel {
            background,
            color,
            shape,
            tags,
            boxes: ids.len(),
        })
    }

    /// The lint findings of the scene on screen, updated with each render
    /// (EDIT-6): `dragged`'s report while dragging and one has arrived,
    /// else the shown build's; empty with no build.
    #[must_use]
    pub fn findings(&self) -> &[Finding] {
        if matches!(self.gesture, Gesture::Dragging { .. })
            && let Some(d) = &self.dragged
        {
            return &d.report.findings;
        }
        self.shown.as_ref().map_or(&[], |(_, b)| &b.report.findings)
    }

    /// The status bar (EDIT-11, EDIT-19, EDIT-24, EDIT-27), derived from
    /// `notice`, `Doc::parse_error`, `Doc::missing` and `io_error`.
    #[must_use]
    pub fn status(&self) -> Status {
        let problem = match &self.doc {
            Some(Doc {
                parse_error: Some(e), ..
            }) => Some(Problem::Parse(e.clone())),
            Some(Doc { missing: true, .. }) => Some(Problem::Missing),
            _ => self.io_error.clone().map(Problem::Io),
        };
        Status {
            notice: self.notice.clone(),
            problem,
        }
    }

    /// True when edits are refused: no parse, a parse error, the file
    /// missing (EDIT-11, EDIT-27). Undo and redo are refused too.
    #[must_use]
    pub fn read_only(&self) -> bool {
        match &self.doc {
            Some(doc) => doc.parsed.is_none() || doc.parse_error.is_some() || doc.missing,
            None => true,
        }
    }

    /// The `edit` batch for `action` against `parsed`, the text it will
    /// be applied to (EDIT-24 calls it again on the newer text). Reads
    /// only `action` and `parsed`, never `self.view` or the shown scene,
    /// so a view switch while the write is in flight changes nothing:
    /// - when the action's view is a `ViewRef::Default` not declared in
    ///   `parsed` (EDIT-15), a layout, style, title or subtitle write
    ///   starts with
    ///   `Edit::AddDefaultView` for that view's scope and kind, and its
    ///   other edits target `ViewRef::key`;
    /// - a move or resize whose view has a box without an `at` line in
    ///   `parsed` is the first write (EDIT-26, so that every box has an
    ///   `at` line after it, EDIT-25): one `Edit::Move` per entry of the
    ///   action's `layout` whose element has no `at` line in that view
    ///   (the moved boxes at their new centres), in `ElementId` order of
    ///   `parsed`, entries whose element is gone skipped, then the
    ///   action's own edits; hand-written `at` lines are left alone;
    /// - otherwise the action's edits only (`Move` per moved box,
    ///   `Resize`, `SetText`, `SetStyle` per element, `ResetStyle` per
    ///   element); `Undo`/`Redo` give their step's edits unchanged, except
    ///   that a recorded move is rebuilt as an EDIT-26 batch when its
    ///   view has a box without an `at` line again (edit.rs header).
    ///
    /// Never yields an edit that creates or deletes an element or
    /// relationship (EDIT-21). Serves EDIT-3, EDIT-15, EDIT-19, EDIT-24,
    /// EDIT-26.
    #[must_use]
    #[expect(clippy::unused_self, reason = "contract signature: a method of the state machine")]
    pub fn edits_for(&self, action: &Action, parsed: &Parsed) -> Vec<Edit> {
        let model = &parsed.model;
        match action {
            Action::Move { view, boxes, layout } => {
                let own: Vec<Edit> = boxes
                    .iter()
                    .map(|(element, &centre)| Edit::Move {
                        view: view.key(),
                        element: element.clone(),
                        centre: Some(centre),
                    })
                    .collect();
                layout_batch(model, view, layout, Some(boxes), own)
            }
            Action::Resize {
                view,
                element,
                size,
                layout,
            } => {
                let own = vec![Edit::Resize {
                    view: view.key(),
                    element: element.clone(),
                    size: Some(*size),
                }];
                layout_batch(model, view, layout, None, own)
            }
            Action::SetText { fields } => {
                let mut out = Vec::new();
                for (target, value, expected) in fields {
                    if let TextTarget::View { view, .. } = target
                        && model.view(view).is_none()
                        && let Some(default) = default_from_key(model, view)
                    {
                        let add = add_default(model, &default);
                        for e in add {
                            if !out.contains(&e) {
                                out.push(e);
                            }
                        }
                    }
                    out.push(Edit::SetText {
                        target: target.clone(),
                        value: value.clone(),
                        expected: expected.clone(),
                    });
                }
                out
            }
            Action::SetStyle {
                view,
                elements,
                target,
                property,
            } => {
                let mut out = add_default(model, view);
                out.extend(elements.iter().map(|e| Edit::SetStyle {
                    element: e.clone(),
                    target: target.clone(),
                    property: *property,
                }));
                out
            }
            Action::ResetStyle { view, elements } => {
                let mut out = add_default(model, view);
                out.extend(elements.iter().map(|e| Edit::ResetStyle { element: e.clone() }));
                out
            }
            Action::Undo(step) | Action::Redo(step) => rebuild_first_write(parsed, &step.edits),
        }
    }
}

/// A centre in grid units from a pointer position in scene px, snapped to
/// the 10px grid (EDIT-2): `geom::snap_px_to_grid` per axis, in grid
/// units.
#[must_use]
pub fn snap_centre(scene_px: ScenePoint) -> Point {
    let p = round_point(scene_px);
    Point::new(snap_px_to_grid(p.x), snap_px_to_grid(p.y))
}

/// The smallest size, px `(width, height)` in multiples of 20, at which
/// `plan_box`'s wrapped texts fit (`scene::texts_fit`, CORE-30): the
/// floor of a resize (EDIT-18). Searches upward from 20x20; never larger
/// than the box's current size when that already fails (the box may keep
/// its size, scene.rs `texts_fit`).
#[must_use]
pub fn min_size(plan_box: &PlanBox) -> (i32, i32) {
    // DECISION: the floor is the smallest-area fitting size, ties to the
    // narrower one; each width's smallest fitting height is found by
    // bisection (`texts_fit` is monotone), so a resize costs a few dozen
    // fits, not thousands.
    let current = (plan_box.half_width * 2 * GRID_PX, plan_box.half_height * 2 * GRID_PX);
    let fits = |w: i32, h: i32| texts_fit(plan_box, w / 2 / GRID_PX, h / 2 / GRID_PX);
    let (w_max, h_max) = (current.0.max(1200), current.1.max(1200));
    let mut best: Option<(i32, i32)> = None;
    let mut w = 20;
    while w <= w_max {
        if fits(w, h_max) {
            // Smallest h in 20px steps with fits(w, h).
            let (mut lo, mut hi) = (1, h_max / 20);
            while lo < hi {
                let mid = i32::midpoint(lo, hi);
                if fits(w, mid * 20) {
                    hi = mid;
                } else {
                    lo = mid + 1;
                }
            }
            let h = lo * 20;
            if best.is_none_or(|(bw, bh)| i64::from(w) * i64::from(h) < i64::from(bw) * i64::from(bh)) {
                best = Some((w, h));
            }
        }
        w += 20;
    }
    let found = best.unwrap_or(current);
    if fits(current.0, current.1) {
        found
    } else {
        (found.0.min(current.0), found.1.min(current.1))
    }
}

/// The resize handle of `rect` (scene px) under `at` (screen px) through
/// `camera`, within `HANDLE_PX`; corners win over sides (EDIT-18).
#[must_use]
pub fn handle_at(rect: Rect, camera: &Camera, at: ScreenPoint) -> Option<Handle> {
    let a = camera.to_screen(rect.min);
    let b = camera.to_screen(rect.max);
    let (mx, my) = (f32::midpoint(a.x, b.x), f32::midpoint(a.y, b.y));
    let near = |x: f32, y: f32| (at.x - x).abs() <= HANDLE_PX && (at.y - y).abs() <= HANDLE_PX;
    [
        (Handle::NW, a.x, a.y),
        (Handle::NE, b.x, a.y),
        (Handle::SE, b.x, b.y),
        (Handle::SW, a.x, b.y),
        (Handle::N, mx, a.y),
        (Handle::E, b.x, my),
        (Handle::S, mx, b.y),
        (Handle::W, a.x, my),
    ]
    .into_iter()
    .find(|&(_, x, y)| near(x, y))
    .map(|(h, ..)| h)
}

/// New centres, grid units, aligning `boxes` (id to rect in scene px) as
/// `how` says (EDIT-20); a single box or none gives an empty map.
#[must_use]
pub fn align(boxes: &BTreeMap<BoxId, Rect>, how: Alignment) -> BTreeMap<BoxId, Point> {
    if boxes.len() < 2 {
        return BTreeMap::new();
    }
    let left = boxes.values().map(|r| r.min.x).min().unwrap_or(0);
    let right = boxes.values().map(|r| r.max.x).max().unwrap_or(0);
    let top = boxes.values().map(|r| r.min.y).min().unwrap_or(0);
    let bottom = boxes.values().map(|r| r.max.y).max().unwrap_or(0);
    let (cx, cy) = (i32::midpoint(left, right), i32::midpoint(top, bottom));
    boxes
        .iter()
        .map(|(&id, r)| {
            let c = r.centre();
            let (hw, hh) = (r.width() / 2, r.height() / 2);
            let (x, y) = match how {
                Alignment::Left => (left + hw, c.y),
                Alignment::Right => (right - hw, c.y),
                Alignment::Centre => (cx, c.y),
                Alignment::Top => (c.x, top + hh),
                Alignment::Bottom => (c.x, bottom - hh),
                Alignment::Middle => (c.x, cy),
            };
            (id, Point::new(snap_px_to_grid(x), snap_px_to_grid(y)))
        })
        .collect()
}

/// New centres, grid units, spacing `boxes` evenly along `axis`, the two
/// outermost kept (EDIT-20); fewer than three boxes give an empty map.
#[must_use]
pub fn distribute(boxes: &BTreeMap<BoxId, Rect>, axis: Axis) -> BTreeMap<BoxId, Point> {
    if boxes.len() < 3 {
        return BTreeMap::new();
    }
    let along = |r: &Rect| match axis {
        Axis::Horizontal => r.centre().x,
        Axis::Vertical => r.centre().y,
    };
    let mut order: Vec<(BoxId, Rect)> = boxes.iter().map(|(&id, &r)| (id, r)).collect();
    order.sort_by_key(|(id, r)| (along(r), *id));
    let first = along(&order[0].1);
    let last = along(&order[order.len() - 1].1);
    let n = i64::try_from(order.len() - 1).unwrap_or(1);
    order
        .iter()
        .zip(0_i64..)
        .map(|((id, r), i)| {
            let span = i64::from(last - first);
            let pos = i32::try_from(i64::from(first) + span * i / n).unwrap_or(first);
            let c = r.centre();
            let (x, y) = match axis {
                Axis::Horizontal => (pos, c.y),
                Axis::Vertical => (c.x, pos),
            };
            (*id, Point::new(snap_px_to_grid(x), snap_px_to_grid(y)))
        })
        .collect()
}

/// True when `element` is enterable (EDIT-15): it has a child in the
/// model, or `view_into` finds a declared view of the counting kind
/// scoped to it (a container view for a system, a component view for a
/// container; a system context view does not count).
#[must_use]
pub fn enterable(model: &Model, element: ElementId) -> bool {
    let Some(e) = model.elements.get(element.0 as usize) else {
        return false;
    };
    if !matches!(e.kind, ElementKind::SoftwareSystem | ElementKind::Container) {
        return false;
    }
    let counting = counting_kind(e.kind);
    !model.children(element).is_empty()
        || model
            .views
            .iter()
            .any(|v| Some(v.kind) == counting && v.scope.element() == Some(element))
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
    let e = model.elements.get(element.0 as usize)?;
    let kind = counting_kind(e.kind)?;
    if let Some(v) = model
        .views
        .iter()
        .find(|v| v.kind == kind && v.scope.element() == Some(element))
    {
        return Some(ViewRef::Declared(v.key.clone()));
    }
    Some(ViewRef::Default {
        scope: e.key.as_ref()?.value.clone(),
        kind,
    })
}

/// The view shown on opening `model` (EDIT-13, EDIT-15, EDIT-27):
/// `remembered` if it still exists (declared, or a default view that
/// still opens), else the first declared view, else (a model-only file)
/// the system context default view of the first software system; `None`
/// when the model has no software system.
#[must_use]
pub fn initial_view(model: &Model, remembered: Option<&ViewRef>) -> Option<ViewRef> {
    if let Some(view) = remembered
        && view_exists(model, view)
    {
        return Some(match view {
            ViewRef::Default { .. } if model.view(&view.key()).is_some() => ViewRef::Declared(view.key()),
            _ => view.clone(),
        });
    }
    if let Some(v) = model.views.first() {
        return Some(ViewRef::Declared(v.key.clone()));
    }
    let system = model
        .elements
        .iter()
        .find(|e| e.kind == ElementKind::SoftwareSystem)?;
    default_view(model, system.id, ViewKind::SystemContext)?;
    Some(ViewRef::Default {
        scope: system.key.as_ref()?.value.clone(),
        kind: ViewKind::SystemContext,
    })
}

/// The text fields of `selected` in Tab order, with the values in
/// `model` (EDIT-5): element name, description, technology (the last for
/// containers and components only); relationship description,
/// technology (of the winning relationship, so a lifted line edits the
/// relationship it was lifted from); view title, subtitle, for a
/// `ViewRef::Default` not yet written too: confirming one writes the
/// default view first (EDIT-15).
#[must_use]
pub fn text_fields(model: &Model, view: &ViewRef, selected: &Selected) -> Vec<TextField> {
    let field = |target: TextTarget, label: &'static str, original: Option<String>| TextField {
        target,
        label,
        buffer: original.clone().unwrap_or_default(),
        original,
    };
    match selected {
        Selected::Element(key) => {
            let Some(id) = model.find(key) else { return Vec::new() };
            let e = model.element(id);
            let target = |f: ElementField| TextTarget::Element {
                element: key.clone(),
                field: f,
            };
            let mut out = vec![
                field(target(ElementField::Name), "name", Some(e.name.value.clone())),
                field(
                    target(ElementField::Description),
                    "description",
                    e.description.as_ref().map(|d| d.value.clone()),
                ),
            ];
            if matches!(e.kind, ElementKind::Container | ElementKind::Component) {
                out.push(field(
                    target(ElementField::Technology),
                    "technology",
                    e.technology.as_ref().map(|t| t.value.clone()),
                ));
            }
            out
        }
        Selected::Relationship(r) => {
            let Some(id) = resolve_relationship(model, r) else { return Vec::new() };
            let rel = model.relationship(id);
            let target = |f: RelationshipField| TextTarget::Relationship {
                relationship: r.clone(),
                field: f,
            };
            vec![
                field(
                    target(RelationshipField::Description),
                    "description",
                    rel.description.as_ref().map(|d| d.value.clone()),
                ),
                field(
                    target(RelationshipField::Technology),
                    "technology",
                    rel.technology.as_ref().map(|t| t.value.clone()),
                ),
            ]
        }
        Selected::Title => {
            let key = view.key();
            let spec = model.view(&key);
            if spec.is_none() && !view_exists(model, view) {
                return Vec::new();
            }
            let target = |f: ViewField| TextTarget::View {
                view: key.clone(),
                field: f,
            };
            vec![
                field(
                    target(ViewField::Title),
                    "title",
                    spec.and_then(|s| s.title.as_ref()).map(|t| t.value.clone()),
                ),
                field(
                    target(ViewField::Subtitle),
                    "subtitle",
                    spec.and_then(|s| s.subtitle.as_ref()).map(|t| t.value.clone()),
                ),
            ]
        }
    }
}

/// The element kind's type line for the details panel (EDIT-10):
/// `Person`, `Software System`, `External Software System`, `Container`,
/// `Component`.
#[must_use]
pub fn kind_label(kind: ElementKind, external: bool) -> &'static str {
    match kind {
        ElementKind::Person => "Person",
        ElementKind::SoftwareSystem if external => "External Software System",
        ElementKind::SoftwareSystem => "Software System",
        ElementKind::Container => "Container",
        ElementKind::Component => "Component",
        _ => "Element",
    }
}

// ------------------------------------------------------------ internals

impl State {
    /// The model of the last good parse.
    fn model(&self) -> Option<&Model> {
        self.doc.as_ref()?.parsed.as_ref().map(|p| &p.model)
    }

    /// The model the scene on screen was built from: the shown build's
    /// parse, which lags `model` between a reload and its build.
    fn scene_model(&self) -> Option<&Model> {
        self.shown.as_ref().map(|(p, _)| &p.model).or_else(|| self.model())
    }

    /// The last good parse.
    fn parsed(&self) -> Option<Arc<Parsed>> {
        self.doc.as_ref()?.parsed.clone()
    }

    /// The element keys of the selection, in selection order.
    fn selected_elements(&self) -> Vec<String> {
        self.selection
            .iter()
            .filter_map(|s| match s {
                Selected::Element(k) => Some(k.clone()),
                _ => None,
            })
            .collect()
    }

    /// The next generation, recorded as issued.
    fn next_generation(&mut self) -> Generation {
        self.generation = self.generation.next();
        self.generation
    }

    /// The item under `at` (screen px) in the scene on screen.
    fn hit(&self, at: ScreenPoint) -> Option<ItemKey> {
        let scene = self.scene()?;
        let p = self.camera.to_scene(at);
        scene.hit(round_point(p), HIT_TOLERANCE_PX)
    }

    /// Remembers the open view and camera for the open file (EDIT-13).
    fn remember(&mut self) {
        if let (Some(doc), Some(view)) = (&self.doc, &self.view) {
            self.ui.files.insert(
                doc.path.clone(),
                FileMemory {
                    view: view.clone(),
                    camera: self.camera,
                },
            );
        }
    }

    /// Ends any pointer gesture and its preview (reload, file gone).
    fn cancel_gesture(&mut self) {
        if matches!(
            self.gesture,
            Gesture::Dragging { .. } | Gesture::Resizing { .. } | Gesture::RubberBand { .. } | Gesture::Pressed { .. }
        ) {
            self.gesture = Gesture::Idle;
        }
        self.dragged = None;
        self.preview = None;
        self.awaited_drag = None;
    }

    /// Re-keys the drag in progress from the scene of a previous parse
    /// to the newly shown build, by element key (EDIT-3, EDIT-24). A drag
    /// none of whose boxes is still on screen is dropped with a notice.
    fn carry_drag(&mut self, old_model: &Model, old_scene: &Scene) {
        let Gesture::Dragging { from, boxes, to } = self.gesture.clone() else { return };
        let (Some(model), Some((_, build))) = (self.model(), self.shown.as_ref()) else { return };
        let scene = &build.scene;
        let carry = |id: BoxId| -> Option<BoxId> {
            let element = old_scene.boxes.get(id.0 as usize)?.element;
            let key = &old_model.element(element).key.as_ref()?.value;
            scene.boxes_of(model.find(key)?).first().copied()
        };
        // DECISION: in a multi-box drag, boxes whose element is gone are
        // left out and the rest still move (EDIT-19 skips gone targets the
        // same way); only a drag with nothing left is dropped.
        let mut new_boxes = BTreeMap::new();
        let mut new_to = BTreeMap::new();
        for (&id, &start) in &boxes {
            if let (Some(new), Some(&end)) = (carry(id), to.get(&id)) {
                new_boxes.insert(new, start);
                new_to.insert(new, end);
            }
        }
        self.awaited_drag = None;
        if new_boxes.is_empty() {
            self.cancel_gesture();
            self.gesture = Gesture::Idle;
            self.notice = Some("move dropped: the dragged element is no longer in the file".to_owned());
            return;
        }
        self.gesture = Gesture::Dragging {
            from,
            boxes: new_boxes,
            to: new_to.clone(),
        };
        self.refresh_preview(&new_to);
    }

    // ---------------------------------------------------------- builds

    /// The shown-view build request for the current parse and view, with
    /// the session's previous centres when the same view is on screen
    /// (CORE-29).
    fn request_shown(&mut self) -> Vec<Effect> {
        let (Some(parsed), Some(view)) = (self.parsed(), self.view.clone()) else {
            return Vec::new();
        };
        let keep = match &self.shown {
            Some((old, build)) if build.scene.key == view.key() => {
                let centres = &self.scene().unwrap_or(&build.scene).centres;
                keep_across_reparse(&old.model, &build.plan, centres, &parsed.model)
            }
            _ => Keep::default(),
        };
        let generation = self.next_generation();
        self.awaited_build = Some(generation);
        vec![Effect::Work(Request {
            generation,
            job: Job::Build {
                parsed,
                view,
                keep,
                shown: true,
            },
        })]
    }

    /// Background builds of every declared view not yet cached and not on
    /// screen (EDIT-14), sent once the shown view is built.
    fn request_background(&mut self) -> Vec<Effect> {
        let Some(parsed) = self.parsed() else {
            return Vec::new();
        };
        let views: Vec<ViewRef> = parsed
            .model
            .views
            .iter()
            .map(|v| ViewRef::Declared(v.key.clone()))
            .filter(|v| Some(v) != self.view.as_ref() && !self.cache.contains_key(v))
            .collect();
        views
            .into_iter()
            .map(|view| {
                let generation = self.next_generation();
                Effect::Work(Request {
                    generation,
                    job: Job::Build {
                        parsed: Arc::clone(&parsed),
                        view,
                        keep: Keep::default(),
                        shown: false,
                    },
                })
            })
            .collect()
    }

    /// Shows `view`: from the cache at once when built, else requested
    /// (EDIT-8, EDIT-14, EDIT-15).
    fn show(&mut self, view: ViewRef) -> Vec<Effect> {
        if self.view.as_ref() == Some(&view) {
            return Vec::new();
        }
        let cached = self.cache.get(&view).cloned();
        self.view = Some(view);
        self.cancel_gesture();
        self.gesture = Gesture::Idle;
        self.hover = None;
        self.text_edit = None;
        self.remember();
        let mut effects = vec![Effect::SaveUiState(self.ui.clone())];
        if let (Some(build), Some(parsed)) = (cached, self.parsed()) {
            self.shown = Some((parsed, build));
            // A build in flight for the previous view is now stale.
            self.awaited_build = Some(self.next_generation());
        } else {
            self.shown = None;
            effects.extend(self.request_shown());
        }
        effects
    }

    /// Re-anchors the open view after a parse: a written default view is
    /// declared now; a removed one opens again as a default view when it
    /// can; a vanished view falls back to `initial_view` (EDIT-15,
    /// EDIT-27).
    fn normalise_view(&mut self) {
        let Some(model) = self.model() else { return };
        let next = match &self.view {
            Some(v) if view_exists(model, v) => Some(match v {
                ViewRef::Default { .. } if model.view(&v.key()).is_some() => ViewRef::Declared(v.key()),
                _ => v.clone(),
            }),
            Some(ViewRef::Declared(key)) => match default_from_key(model, key) {
                Some(d) if view_exists(model, &d) => Some(d),
                _ => initial_view(model, None),
            },
            _ => initial_view(model, None),
        };
        if next != self.view {
            self.view = next;
            self.shown = None;
            self.remember();
        }
    }

    // ---------------------------------------------------------- writes

    /// Writes `action`: its batch applied to the last good parse and sent
    /// as a checked write (EDIT-3, EDIT-24); refused while read-only or a
    /// write is in flight.
    fn write(&mut self, action: Action) -> Vec<Effect> {
        if self.read_only() {
            self.notice = Some("change refused: the file is read-only until it parses and is on disk".to_owned());
            self.give_back(action);
            return Vec::new();
        }
        if self.pending.is_some() {
            self.notice = Some("change refused: the previous change is still being written".to_owned());
            self.give_back(action);
            return Vec::new();
        }
        let (Some(parsed), Some(doc)) = (self.parsed(), self.doc.as_ref()) else {
            return Vec::new();
        };
        let (path, expected) = (doc.path.clone(), doc.hash);
        self.write_to(action, &parsed, path, expected)
    }

    /// `write` against a given parse and hash (also the EDIT-24 re-apply).
    fn write_to(&mut self, action: Action, parsed: &Parsed, path: PathBuf, expected: ContentHash) -> Vec<Effect> {
        let edits = self.edits_for(&action, parsed);
        if edits.is_empty() {
            return Vec::new();
        }
        match apply(parsed, &edits) {
            Ok(applied) => {
                let text = applied.text.clone();
                self.pending = Some(Pending {
                    action,
                    inverse: applied.inverse,
                    text: applied.text,
                });
                vec![Effect::Write(WriteRequest { path, text, expected })]
            }
            Err(e) => {
                // EDIT-19, EDIT-24: the action or step is dropped.
                self.notice = Some(match action {
                    Action::Undo(_) => format!("undo step skipped: {e}"),
                    Action::Redo(_) => format!("redo step skipped: {e}"),
                    _ => e.to_string(),
                });
                self.preview = None;
                Vec::new()
            }
        }
    }

    /// Puts an undo or redo step back on its stack when it was not
    /// applied for a reason that is not the step's own (read-only, write
    /// in flight, I/O error).
    fn give_back(&mut self, action: Action) {
        match action {
            Action::Undo(step) => self.history.undo.push(step),
            Action::Redo(step) => self.history.redo.push(step),
            _ => self.preview = None,
        }
    }

    /// Every box of the scene on screen at its centre in px, by element
    /// key: the `layout` of a move or resize (EDIT-26).
    fn layout(&self) -> BTreeMap<String, (i32, i32)> {
        let (Some(scene), Some(model)) = (self.scene(), self.scene_model()) else {
            return BTreeMap::new();
        };
        scene
            .boxes
            .iter()
            .filter_map(|b| {
                let key = model.element(b.element).key.as_ref()?.value.clone();
                let c = scene.centres[b.id.0 as usize];
                Some((key, (c.x * GRID_PX, c.y * GRID_PX)))
            })
            .collect()
    }

    /// Moves boxes to `to` (grid centres) as one step (EDIT-3, EDIT-20).
    fn write_moves(&mut self, to: &BTreeMap<BoxId, Point>) -> Vec<Effect> {
        let (Some(view), Some(scene), Some(model)) = (self.view.clone(), self.scene(), self.scene_model()) else {
            return Vec::new();
        };
        let mut boxes = BTreeMap::new();
        for (id, c) in to {
            let Some(b) = scene.boxes.get(id.0 as usize) else { continue };
            if let Some(key) = &model.element(b.element).key {
                boxes.insert(key.value.clone(), (c.x * GRID_PX, c.y * GRID_PX));
            }
        }
        if boxes.is_empty() {
            return Vec::new();
        }
        let mut layout = self.layout();
        for (k, c) in &boxes {
            layout.insert(k.clone(), *c);
        }
        self.write(Action::Move { view, boxes, layout })
    }

    // ---------------------------------------------------------- input

    /// One input event (EDIT-28).
    fn input(&mut self, input: Input) -> Vec<Effect> {
        match input {
            Input::PointerDown { at, button, mods } => {
                self.notice = None;
                self.pointer_down(at, button, mods)
            }
            Input::PointerMove { at } => self.pointer_move(at),
            Input::PointerUp { at, button } => self.pointer_up(at, button),
            Input::PointerLeft => {
                self.hover = None;
                Vec::new()
            }
            Input::Wheel { at, notches } => {
                self.camera.zoom_at(at, ZOOM_STEP.powf(notches));
                self.remember();
                Vec::new()
            }
            Input::KeyDown { key, mods, text_focus } => {
                self.notice = None;
                self.key_down(key, mods, text_focus)
            }
            Input::KeyUp { key } => {
                if key == Key::Space {
                    self.space = false;
                }
                Vec::new()
            }
        }
    }

    fn pointer_down(&mut self, at: ScreenPoint, button: Button, mods: Modifiers) -> Vec<Effect> {
        if button == Button::Middle || (button == Button::Left && self.space) {
            self.gesture = Gesture::Panning { last: at };
            return Vec::new();
        }
        let hit = self.hit(at);
        let handle = if button == Button::Left && !self.read_only() {
            self.handle_under(at)
        } else {
            None
        };
        self.gesture = Gesture::Pressed {
            at,
            button,
            mods,
            hit,
            handle,
        };
        Vec::new()
    }

    /// The resize handle of the single selected box under `at`.
    fn handle_under(&self, at: ScreenPoint) -> Option<(BoxId, Handle)> {
        let scene = self.scene()?;
        let model = self.model()?;
        let mut items = self.selection.iter().flat_map(|s| items_of(scene, model, s));
        let (Some(ItemKey::Box(id)), None) = (items.next(), items.next()) else {
            return None;
        };
        let rect = scene.boxes.get(id.0 as usize)?.rect;
        handle_at(rect, &self.camera, at).map(|h| (id, h))
    }

    fn pointer_move(&mut self, at: ScreenPoint) -> Vec<Effect> {
        match self.gesture.clone() {
            Gesture::Idle => {
                self.hover = self.hit(at);
                Vec::new()
            }
            Gesture::Pressed {
                at: from,
                button,
                mods,
                hit,
                handle,
            } => {
                self.hover = self.hit(at);
                if button != Button::Left || distance(from, at) < CLICK_SLOP_PX {
                    return Vec::new();
                }
                self.start_drag(from, at, mods, hit, handle)
            }
            Gesture::Dragging { from, boxes, to } => self.drag_to(from, &boxes, &to, at),
            Gesture::Resizing { id, handle, from, size } => {
                self.resize_to(id, handle, from, size, at);
                Vec::new()
            }
            Gesture::RubberBand { from, add, .. } => {
                self.gesture = Gesture::RubberBand { from, to: at, add };
                Vec::new()
            }
            Gesture::Panning { last } => {
                self.camera.pan_by(ScreenPoint {
                    x: at.x - last.x,
                    y: at.y - last.y,
                });
                self.gesture = Gesture::Panning { last: at };
                self.remember();
                Vec::new()
            }
        }
    }

    /// A left press travelled `CLICK_SLOP_PX`: resize, drag or rubber band.
    #[expect(clippy::too_many_arguments, reason = "the fields of `Gesture::Pressed`, passed through")]
    fn start_drag(
        &mut self,
        from: ScreenPoint,
        at: ScreenPoint,
        mods: Modifiers,
        hit: Option<ItemKey>,
        handle: Option<(BoxId, Handle)>,
    ) -> Vec<Effect> {
        let read_only = self.read_only();
        if let Some((id, handle)) = handle
            && !read_only
            && let Some(b) = self.scene().and_then(|s| s.boxes.get(id.0 as usize))
        {
            let size = (b.rect.width(), b.rect.height());
            self.text_edit = None;
            self.gesture = Gesture::Resizing { id, handle, from, size };
            self.resize_to(id, handle, from, size, at);
            return Vec::new();
        }
        match hit {
            Some(ItemKey::Box(id)) if !read_only => {
                let (Some(scene), Some(model)) = (self.scene(), self.model()) else {
                    return Vec::new();
                };
                let Some(key) = scene
                    .boxes
                    .get(id.0 as usize)
                    .and_then(|b| model.element(b.element).key.as_ref())
                    .map(|k| k.value.clone())
                else {
                    return Vec::new();
                };
                // Dragging a selected box moves every selected box; an
                // unselected one is selected first (EDIT-20).
                if !self.selection.contains(&Selected::Element(key.clone())) {
                    if !mods.shift {
                        self.selection.clear();
                    }
                    self.selection.insert(Selected::Element(key));
                }
                let (Some(scene), Some(model)) = (self.scene(), self.model()) else {
                    return Vec::new();
                };
                let boxes: BTreeMap<BoxId, Point> = self
                    .selection
                    .iter()
                    .flat_map(|s| items_of(scene, model, s))
                    .filter_map(|k| match k {
                        ItemKey::Box(b) => Some((b, scene.centres[b.0 as usize])),
                        _ => None,
                    })
                    .collect();
                self.text_edit = None;
                self.gesture = Gesture::Dragging {
                    from,
                    to: boxes.clone(),
                    boxes: boxes.clone(),
                };
                let to = boxes.clone();
                self.drag_to(from, &boxes, &to, at)
            }
            Some(ItemKey::Box(_)) => {
                // Read-only: a drag on a box does nothing (EDIT-11).
                self.gesture = Gesture::Idle;
                Vec::new()
            }
            _ => {
                self.gesture = Gesture::RubberBand {
                    from,
                    to: at,
                    add: mods.shift,
                };
                Vec::new()
            }
        }
    }

    /// The dragged boxes follow the pointer, snapped (EDIT-2); the box
    /// moves at once and a drag preview is requested (EDIT-25).
    fn drag_to(
        &mut self,
        from: ScreenPoint,
        boxes: &BTreeMap<BoxId, Point>,
        to: &BTreeMap<BoxId, Point>,
        at: ScreenPoint,
    ) -> Vec<Effect> {
        let (dx, dy) = ((at.x - from.x) / self.camera.zoom, (at.y - from.y) / self.camera.zoom);
        let next: BTreeMap<BoxId, Point> = boxes
            .iter()
            .map(|(&id, &c)| {
                #[expect(clippy::cast_precision_loss, reason = "scene px fit f32 exactly")]
                let (x, y) = ((c.x * GRID_PX) as f32 + dx, (c.y * GRID_PX) as f32 + dy);
                (id, snap_centre(ScenePoint { x, y }))
            })
            .collect();
        self.gesture = Gesture::Dragging {
            from,
            boxes: boxes.clone(),
            to: next.clone(),
        };
        if &next == to && self.preview.is_some() {
            return Vec::new();
        }
        self.refresh_preview(&next);
        if &next == to {
            return Vec::new();
        }
        let Some((parsed, previous)) = self.shown.clone() else {
            return Vec::new();
        };
        let generation = self.next_generation();
        self.awaited_drag = Some(generation);
        vec![Effect::Work(Request {
            generation,
            job: Job::Drag {
                parsed,
                previous,
                moved: next,
            },
        })]
    }

    /// The preview: the last drag reply's scene (else the shown one) with
    /// every dragged box at its current centre (EDIT-25).
    fn refresh_preview(&mut self, to: &BTreeMap<BoxId, Point>) {
        let base = match (&self.dragged, &self.shown) {
            (Some(d), _) => d.scene.clone(),
            (None, Some((_, b))) => b.scene.clone(),
            (None, None) => return,
        };
        let mut scene = base;
        for (&id, &c) in to {
            if (id.0 as usize) < scene.boxes.len() {
                scene.move_box(id, c);
            }
        }
        self.preview = Some(scene);
    }

    /// The resized box's size for the pointer at `at` (EDIT-18): 20px
    /// steps about the centre, never where the texts stop fitting.
    #[expect(clippy::too_many_arguments, reason = "the fields of `Gesture::Resizing` plus the pointer")]
    fn resize_to(&mut self, id: BoxId, handle: Handle, from: ScreenPoint, size: (i32, i32), at: ScreenPoint) {
        let Some((_, build)) = &self.shown else { return };
        let (Some(b), Some(plan_box)) = (build.scene.boxes.get(id.0 as usize), build.plan.boxes.get(id.0 as usize)) else {
            return;
        };
        let start = (b.rect.width(), b.rect.height());
        let (dx, dy) = ((at.x - from.x) / self.camera.zoom, (at.y - from.y) / self.camera.zoom);
        let (sx, sy) = match handle {
            Handle::E => (1.0, 0.0),
            Handle::W => (-1.0, 0.0),
            Handle::N => (0.0, -1.0),
            Handle::S => (0.0, 1.0),
            Handle::NE => (1.0, -1.0),
            Handle::NW => (-1.0, -1.0),
            Handle::SE => (1.0, 1.0),
            Handle::SW => (-1.0, 1.0),
        };
        let step = |base: i32, delta: f32| -> i32 {
            // The centre stays, so the box grows by twice the drag.
            #[expect(clippy::cast_precision_loss, reason = "scene px fit f32 exactly")]
            let raw = base as f32 + 2.0 * delta;
            #[expect(clippy::cast_possible_truncation, reason = "rounded px in range")]
            let snapped = ((raw / 20.0).round() as i32) * 20;
            snapped.max(20)
        };
        let want = (step(start.0, dx * sx), step(start.1, dy * sy));
        let fits = |(w, h): (i32, i32)| {
            let ok = texts_fit(plan_box, w / 2 / GRID_PX, h / 2 / GRID_PX);
            // A box that already fails may keep its size and grow.
            ok || (w >= start.0 && h >= start.1)
        };
        // DECISION: a size the texts do not fit is clamped up to the
        // `min_size` floor per axis (a larger box still fits, `texts_fit`
        // is monotone), so shrinking past the floor stops at it.
        let floor = min_size(plan_box);
        let clamped = (want.0.max(floor.0), want.1.max(floor.1));
        let next = [want, clamped, (want.0, size.1), (size.0, want.1), size]
            .into_iter()
            .find(|&s| fits(s))
            .unwrap_or(size);
        self.gesture = Gesture::Resizing {
            id,
            handle,
            from,
            size: next,
        };
    }

    fn pointer_up(&mut self, at: ScreenPoint, button: Button) -> Vec<Effect> {
        let gesture = std::mem::replace(&mut self.gesture, Gesture::Idle);
        match gesture {
            Gesture::Pressed {
                at: from,
                button: b,
                mods,
                hit,
                ..
            } if b == button => {
                if distance(from, at) >= CLICK_SLOP_PX && b == Button::Left {
                    return Vec::new();
                }
                self.click(b, mods, hit)
            }
            Gesture::Dragging { boxes, to, .. } => {
                self.dragged = None;
                self.awaited_drag = None;
                if to == boxes {
                    self.preview = None;
                    return Vec::new();
                }
                // The preview stays until the saved file's build arrives.
                self.write_moves(&to)
            }
            Gesture::Resizing { id, size, .. } => {
                let (Some(view), Some(scene), Some(model)) = (self.view.clone(), self.scene(), self.model()) else {
                    return Vec::new();
                };
                let Some(b) = scene.boxes.get(id.0 as usize) else { return Vec::new() };
                if (b.rect.width(), b.rect.height()) == size {
                    return Vec::new();
                }
                let Some(element) = model.element(b.element).key.as_ref().map(|k| k.value.clone()) else {
                    return Vec::new();
                };
                let layout = self.layout();
                self.write(Action::Resize {
                    view,
                    element,
                    size,
                    layout,
                })
            }
            Gesture::RubberBand { from, to, add } => {
                self.band_select(from, to, add);
                Vec::new()
            }
            Gesture::Panning { .. } | Gesture::Idle | Gesture::Pressed { .. } => Vec::new(),
        }
    }

    /// A click (EDIT-15, EDIT-28): enter an enterable box, else select;
    /// Ctrl or right selects only; Shift toggles; empty canvas deselects.
    fn click(&mut self, button: Button, mods: Modifiers, hit: Option<ItemKey>) -> Vec<Effect> {
        self.text_edit = None;
        let (Some(scene), Some(model)) = (self.scene(), self.model()) else {
            self.selection.clear();
            return Vec::new();
        };
        let selected = hit.and_then(|h| selected_of(scene, model, h));
        if mods.shift {
            if let Some(s) = selected
                && !self.selection.remove(&s)
            {
                self.selection.insert(s);
            }
            return Vec::new();
        }
        let plain = button == Button::Left && !mods.ctrl;
        if plain
            && let Some(ItemKey::Box(id)) = hit
            && let Some(b) = scene.boxes.get(id.0 as usize)
            && enterable(model, b.element)
            && let Some(view) = view_into(model, b.element)
        {
            return self.show(view);
        }
        self.selection.clear();
        if let Some(s) = selected {
            self.selection.insert(s);
        }
        Vec::new()
    }

    /// Selects the boxes inside the rubber band (EDIT-20).
    fn band_select(&mut self, from: ScreenPoint, to: ScreenPoint, add: bool) {
        let (Some(scene), Some(model)) = (self.scene(), self.model()) else { return };
        let (a, b) = (self.camera.to_scene(from), self.camera.to_scene(to));
        let band = Rect::from_corners(round_point(a), round_point(b));
        let inside: Vec<Selected> = scene
            .boxes
            .iter()
            .filter(|bx| band.contains(bx.rect.min) && band.contains(bx.rect.max))
            .filter_map(|bx| model.element(bx.element).key.as_ref().map(|k| Selected::Element(k.value.clone())))
            .collect();
        if !add {
            self.selection.clear();
        }
        self.selection.extend(inside);
    }

    fn key_down(&mut self, key: Key, mods: Modifiers, text_focus: bool) -> Vec<Effect> {
        if self.text_edit.is_some() {
            return match key {
                Key::Enter => self.confirm_text(),
                Key::Tab => {
                    if let Some(edit) = self.text_edit.as_mut()
                        && !edit.fields.is_empty()
                    {
                        edit.current = (edit.current + 1) % edit.fields.len();
                    }
                    Vec::new()
                }
                Key::Escape => {
                    self.text_edit = None;
                    Vec::new()
                }
                _ => Vec::new(),
            };
        }
        match key {
            Key::Enter | Key::F2 => {
                self.open_text_edit();
                Vec::new()
            }
            Key::Escape => {
                self.selection.clear();
                Vec::new()
            }
            Key::Backspace if !text_focus => self.go_up(),
            Key::ArrowLeft if mods.alt => self.go_up(),
            Key::Space => {
                self.space = true;
                Vec::new()
            }
            Key::Z if mods.ctrl && mods.shift => self.command(Command::Redo),
            Key::Z if mods.ctrl => self.command(Command::Undo),
            Key::Y if mods.ctrl => self.command(Command::Redo),
            _ => Vec::new(),
        }
    }

    /// Opens the in-place editor on the single selected item (EDIT-5).
    fn open_text_edit(&mut self) {
        if self.read_only() || self.selection.len() != 1 {
            return;
        }
        let (Some(model), Some(view)) = (self.model(), self.view.as_ref()) else { return };
        let Some(item) = self.selection.iter().next().cloned() else { return };
        let fields = text_fields(model, view, &item);
        if !fields.is_empty() {
            self.text_edit = Some(TextEdit {
                item,
                fields,
                current: 0,
            });
        }
    }

    /// Writes every changed field as one batch (EDIT-5, DSL-10).
    fn confirm_text(&mut self) -> Vec<Effect> {
        let Some(edit) = self.text_edit.take() else { return Vec::new() };
        let fields: Vec<_> = edit
            .fields
            .into_iter()
            .filter(|f| f.original.as_deref().unwrap_or("") != f.buffer)
            .map(|f| {
                let is_name = matches!(
                    f.target,
                    TextTarget::Element {
                        field: ElementField::Name,
                        ..
                    }
                );
                let value = (is_name || !f.buffer.is_empty()).then_some(f.buffer);
                (f.target, value, f.original)
            })
            .collect();
        if fields.is_empty() {
            return Vec::new();
        }
        self.write(Action::SetText { fields })
    }

    /// Alt+Left or Backspace: the parent crumb (EDIT-16).
    fn go_up(&mut self) -> Vec<Effect> {
        let context = match (self.model(), &self.view) {
            (Some(model), Some(v)) => view_kind(model, v) == Some(ViewKind::SystemContext),
            _ => return Vec::new(),
        };
        let crumbs = self.breadcrumb().len();
        if context || crumbs < 2 {
            return self.command(Command::Crumb(0));
        }
        self.command(Command::Crumb(crumbs - 2))
    }

    // ---------------------------------------------------------- commands

    #[expect(clippy::too_many_lines, reason = "one flat arm per command reads best")]
    fn command(&mut self, command: Command) -> Vec<Effect> {
        match command {
            Command::Open(path) => {
                self.remember();
                vec![Effect::Read(path)]
            }
            Command::ShowOpenDialog => vec![Effect::Dialog(Dialog::Open {
                dir: self.doc.as_ref().map(|d| export_dir(&d.path)),
            })],
            Command::ShowView(view) => self.show(view),
            Command::Crumb(n) => {
                let Some(model) = self.model() else { return Vec::new() };
                let target = if n == 0 {
                    model
                        .views
                        .iter()
                        .find(|v| v.kind == ViewKind::SystemContext)
                        .or_else(|| model.views.first())
                        .map(|v| ViewRef::Declared(v.key.clone()))
                } else {
                    self.breadcrumb()
                        .get(n)
                        .and_then(|c| c.scope.as_deref())
                        .and_then(|k| model.find(k))
                        .and_then(|id| view_into(model, id))
                };
                target.map_or_else(Vec::new, |v| self.show(v))
            }
            Command::SetStyle { target, property } => {
                let elements = self.selected_elements();
                let Some(view) = self.view.clone() else { return Vec::new() };
                if elements.is_empty() {
                    return Vec::new();
                }
                self.write(Action::SetStyle {
                    view,
                    elements,
                    target,
                    property,
                })
            }
            Command::ResetStyle => {
                let elements = self.selected_elements();
                let Some(view) = self.view.clone() else { return Vec::new() };
                if elements.is_empty() {
                    return Vec::new();
                }
                self.write(Action::ResetStyle { view, elements })
            }
            Command::Align(how) => {
                let rects = self.selected_rects();
                let to = align(&rects, how);
                self.write_moves(&to)
            }
            Command::Distribute(axis) => {
                let rects = self.selected_rects();
                let to = distribute(&rects, axis);
                self.write_moves(&to)
            }
            Command::Undo => {
                if self.read_only() || self.pending.is_some() {
                    self.notice = Some("undo refused: the file is read-only or a change is being written".to_owned());
                    return Vec::new();
                }
                match self.history.pop_undo() {
                    Some(step) => self.write(Action::Undo(step)),
                    None => Vec::new(),
                }
            }
            Command::Redo => {
                if self.read_only() || self.pending.is_some() {
                    self.notice = Some("redo refused: the file is read-only or a change is being written".to_owned());
                    return Vec::new();
                }
                match self.history.pop_redo() {
                    Some(step) => self.write(Action::Redo(step)),
                    None => Vec::new(),
                }
            }
            Command::ExportView(format) => {
                let (Some(doc), Some(view)) = (&self.doc, &self.view) else { return Vec::new() };
                vec![Effect::Dialog(Dialog::ExportView {
                    view: view.clone(),
                    dir: export_dir(&doc.path),
                    name: export_name(&view.key(), format),
                    format,
                })]
            }
            Command::ExportAll(format) => {
                let Some(doc) = &self.doc else { return Vec::new() };
                vec![Effect::Dialog(Dialog::ExportAll {
                    dir: export_dir(&doc.path),
                    format,
                })]
            }
            Command::Export { targets, format } => {
                let Some(parsed) = self.parsed() else { return Vec::new() };
                let shown_keep = self.shown.as_ref().map(|(p, b)| {
                    (b.scene.key.clone(), if Arc::ptr_eq(p, &parsed) {
                        Keep::from_previous(&b.plan, &b.scene.centres)
                    } else {
                        keep_across_reparse(&p.model, &b.plan, &b.scene.centres, &parsed.model)
                    })
                });
                let targets = targets
                    .into_iter()
                    .map(|(view, path)| ExportTarget {
                        keep: match &shown_keep {
                            Some((key, keep)) if *key == view.key() => keep.clone(),
                            _ => Keep::default(),
                        },
                        view,
                        path,
                    })
                    .collect();
                let generation = self.next_generation();
                vec![Effect::Work(Request {
                    generation,
                    job: Job::Export {
                        parsed,
                        targets,
                        format,
                        png_width: None,
                    },
                })]
            }
            Command::PickFinding(n) => {
                let (Some(scene), Some(model)) = (self.scene(), self.model()) else { return Vec::new() };
                let Some(finding) = self.findings().get(n) else { return Vec::new() };
                let picked: BTreeSet<Selected> =
                    finding.items.iter().filter_map(|&k| selected_of(scene, model, k)).collect();
                self.selection = picked;
                Vec::new()
            }
        }
    }

    /// The selected boxes' rectangles, scene px (EDIT-20).
    fn selected_rects(&self) -> BTreeMap<BoxId, Rect> {
        let (Some(scene), Some(model)) = (self.scene(), self.model()) else {
            return BTreeMap::new();
        };
        self.selection
            .iter()
            .flat_map(|s| items_of(scene, model, s))
            .filter_map(|k| match k {
                ItemKey::Box(id) => scene.boxes.get(id.0 as usize).map(|b| (id, b.rect)),
                _ => None,
            })
            .collect()
    }

    // ---------------------------------------------------------- files

    fn file(&mut self, event: FileEvent) -> Vec<Effect> {
        match event {
            FileEvent::Loaded { path, text, hash } => self.loaded(path, &text, hash),
            FileEvent::OpenFailed { path, error } => {
                self.io_error = Some(error);
                self.ui.recent.retain(|p| p != &path);
                vec![Effect::SaveUiState(self.ui.clone())]
            }
            FileEvent::Changed { text, hash } => {
                if self.doc.as_ref().is_none_or(|d| d.hash == hash && !d.missing) {
                    if let Some(doc) = self.doc.as_mut() {
                        doc.missing = false;
                    }
                    return Vec::new();
                }
                self.reload(&text, hash)
            }
            FileEvent::Removed => {
                if let Some(doc) = self.doc.as_mut() {
                    doc.missing = true;
                    self.cancel_gesture();
                    self.text_edit = None;
                }
                Vec::new()
            }
            FileEvent::Written { hash } => self.written(hash),
            FileEvent::ChangedOnDisk { text, hash } => {
                let pending = self.pending.take();
                let mut effects = self.reload(&text, hash);
                let Some(pending) = pending else { return effects };
                let (Some(parsed), Some(doc)) = (self.parsed(), self.doc.as_ref()) else {
                    return effects;
                };
                if doc.parse_error.is_some() {
                    self.notice = Some("change dropped: the file on disk does not parse".to_owned());
                    return effects;
                }
                let (path, expected) = (doc.path.clone(), doc.hash);
                effects.extend(self.write_to(pending.action, &parsed, path, expected));
                effects
            }
            FileEvent::WriteFailed { error } => {
                self.io_error = Some(error);
                if let Some(pending) = self.pending.take() {
                    self.give_back(pending.action);
                }
                // The canvas goes back to the file's state (EDIT-27).
                self.preview = None;
                self.dragged = None;
                Vec::new()
            }
        }
    }

    /// A file was opened (EDIT-13, EDIT-11).
    fn loaded(&mut self, path: PathBuf, text: &str, hash: ContentHash) -> Vec<Effect> {
        let (parsed, parse_error) = match parse(text) {
            Ok(p) => (Some(Arc::new(p)), None),
            Err(e) => (None, Some(e)),
        };
        let memory = self.ui.files.get(&path).cloned();
        self.doc = Some(Doc {
            path: path.clone(),
            parsed,
            hash,
            parse_error,
            missing: false,
        });
        self.shown = None;
        self.cache.clear();
        self.selection.clear();
        self.hover = None;
        self.text_edit = None;
        self.pending = None;
        self.history = History::default();
        self.cancel_gesture();
        self.gesture = Gesture::Idle;
        self.io_error = None;
        self.notice = None;
        self.camera = memory.as_ref().map_or_else(Camera::default, |m| m.camera);
        self.view = self.model().and_then(|m| initial_view(m, memory.as_ref().map(|m| &m.view)));
        self.ui.opened(&path);
        self.remember();
        let mut effects = vec![Effect::Watch(path), Effect::SaveUiState(self.ui.clone())];
        effects.extend(self.request_shown());
        effects
    }

    /// The file changed on disk (EDIT-1, EDIT-11, EDIT-23, EDIT-27).
    fn reload(&mut self, text: &str, hash: ContentHash) -> Vec<Effect> {
        let Some(doc) = self.doc.as_mut() else { return Vec::new() };
        doc.hash = hash;
        doc.missing = false;
        match parse(text) {
            Ok(p) => {
                doc.parsed = Some(Arc::new(p));
                doc.parse_error = None;
            }
            Err(e) => {
                // EDIT-11: the last good diagram stays, read-only.
                doc.parse_error = Some(e);
                self.cancel_gesture();
                self.text_edit = None;
                return Vec::new();
            }
        }
        self.io_error = None;
        self.cache.clear();
        // EDIT-3, EDIT-24: a drag survives the reload; its boxes are
        // carried over to the new build by element key when it lands
        // (`carry_drag`), and a release before that writes by key against
        // the new text.
        if matches!(self.gesture, Gesture::Resizing { .. }) {
            self.cancel_gesture();
        }
        let first = self.shown.is_none() && self.view.is_none();
        self.normalise_view();
        let mut effects = self.request_shown();
        if first {
            effects.push(Effect::SaveUiState(self.ui.clone()));
        }
        effects
    }

    /// The pending write landed (EDIT-22): its text is the file now, its
    /// inverse goes into the history (EDIT-19).
    fn written(&mut self, hash: ContentHash) -> Vec<Effect> {
        let Some(pending) = self.pending.take() else { return Vec::new() };
        self.io_error = None;
        let step = Step { edits: pending.inverse };
        match pending.action {
            Action::Undo(_) => self.history.push_undone(step),
            Action::Redo(_) => self.history.push_redone(step),
            _ => self.history.record(step),
        }
        let Some(doc) = self.doc.as_mut() else { return Vec::new() };
        doc.hash = hash;
        match parse(&pending.text) {
            Ok(p) => {
                doc.parsed = Some(Arc::new(p));
                doc.parse_error = None;
            }
            Err(e) => doc.parse_error = Some(e),
        }
        self.cache.clear();
        self.normalise_view();
        self.request_shown()
    }

    // ---------------------------------------------------------- replies

    fn reply(&mut self, reply: Reply) -> Vec<Effect> {
        match reply {
            Reply::Built {
                generation,
                view,
                build,
                shown,
            } => {
                let current = self.awaited_build.is_some_and(|g| g == generation) && self.view.as_ref() == Some(&view);
                if shown && current {
                    let Some(parsed) = self.parsed() else { return Vec::new() };
                    self.cache.insert(view, Arc::clone(&build));
                    let old = self.shown.replace((Arc::clone(&parsed), build));
                    self.dragged = None;
                    if matches!(self.gesture, Gesture::Dragging { .. }) {
                        if let Some((old_parsed, old_build)) = old
                            && !Arc::ptr_eq(&old_parsed, &parsed)
                        {
                            self.carry_drag(&old_parsed.model, &old_build.scene);
                        }
                    } else {
                        self.preview = None;
                    }
                    self.hover = None;
                    return self.request_background();
                }
                if !shown && self.awaited_build.is_none_or(|g| generation > g) {
                    self.cache.insert(view, build);
                }
                Vec::new()
            }
            Reply::Dragged { generation, build } => {
                if self.awaited_drag != Some(generation) {
                    return Vec::new();
                }
                if let Gesture::Dragging { to, .. } = self.gesture.clone() {
                    self.dragged = Some(build);
                    self.refresh_preview(&to);
                }
                Vec::new()
            }
            Reply::NoView { generation, view } => {
                if self.awaited_build != Some(generation) || self.view.as_ref() != Some(&view) {
                    return Vec::new();
                }
                // EDIT-27: the open view is gone; the first declared opens.
                let next = self.model().and_then(|m| initial_view(m, None));
                if next.is_none() || next == self.view {
                    self.view = None;
                    self.shown = None;
                    return Vec::new();
                }
                self.view = next;
                self.shown = None;
                self.remember();
                self.request_shown()
            }
            Reply::Exported { result, .. } => {
                self.notice = Some(match result {
                    Ok(paths) if paths.len() == 1 => format!("exported {}", paths[0].display()),
                    Ok(paths) => format!("exported {} files", paths.len()),
                    Err(e) => e,
                });
                Vec::new()
            }
        }
    }
}

/// Rounds an unrounded scene point to scene px.
fn round_point(p: ScenePoint) -> Point {
    #[expect(clippy::cast_possible_truncation, reason = "scene px are small")]
    Point::new(p.x.round() as i32, p.y.round() as i32)
}

/// Screen distance between two points.
fn distance(a: ScreenPoint, b: ScreenPoint) -> f32 {
    (a.x - b.x).hypot(a.y - b.y)
}

/// The scene items standing for `selected`.
fn items_of(scene: &Scene, model: &Model, selected: &Selected) -> Vec<ItemKey> {
    match selected {
        Selected::Element(key) => model
            .find(key)
            .map(|id| scene.boxes_of(id).into_iter().map(ItemKey::Box).collect())
            .unwrap_or_default(),
        Selected::Relationship(r) => resolve_relationship(model, r)
            .map(|id| {
                scene
                    .lines
                    .iter()
                    .filter(|l| l.relationship == id)
                    .map(|l| ItemKey::Line(l.edge))
                    .collect()
            })
            .unwrap_or_default(),
        Selected::Title => {
            if scene.title.is_some() {
                vec![ItemKey::Title]
            } else {
                Vec::new()
            }
        }
    }
}

/// The selection a hit item makes: a box's element, a line's or label's
/// winning relationship, the title; nothing for the rest.
fn selected_of(scene: &Scene, model: &Model, item: ItemKey) -> Option<Selected> {
    match item {
        ItemKey::Box(_) | ItemKey::Line(_) | ItemKey::Label(_) => match scene.model_id(item)? {
            ModelId::Element(e) => model.element(e).key.as_ref().map(|k| Selected::Element(k.value.clone())),
            ModelId::Relationship(r) => Some(Selected::Relationship(RelationshipRef::of(model, r))),
            _ => None,
        },
        ItemKey::Title | ItemKey::Subtitle => Some(Selected::Title),
        _ => None,
    }
}

/// The relationship `r` names in `model`.
fn resolve_relationship(model: &Model, r: &RelationshipRef) -> Option<RelationshipId> {
    let (s, d) = (model.find(&r.source)?, model.find(&r.destination)?);
    model.find_relationship(s, d).get(r.ordinal as usize).copied()
}

/// The scope element of `view` in `model`.
fn view_scope(model: &Model, view: &ViewRef) -> Option<ElementId> {
    match view {
        ViewRef::Declared(key) => model.view(key)?.scope.element(),
        ViewRef::Default { scope, .. } => model.find(scope),
    }
}

/// The kind of `view` in `model`.
fn view_kind(model: &Model, view: &ViewRef) -> Option<ViewKind> {
    match view {
        ViewRef::Declared(key) => model.view(key).map(|v| v.kind),
        ViewRef::Default { kind, .. } => Some(*kind),
    }
}

/// True when `view` can be shown from `model`: declared, or a default
/// view written or still opening.
fn view_exists(model: &Model, view: &ViewRef) -> bool {
    match view {
        ViewRef::Declared(key) => model.view(key).is_some(),
        ViewRef::Default { scope, kind } => {
            model.view(&view.key()).is_some_and(|v| v.kind == *kind && v.scope.element() == model.find(scope))
                || model.find(scope).and_then(|id| default_view(model, id, *kind)).is_some()
        }
    }
}

/// The default view a key `<scope>-<kind>` names, when the scope exists
/// (EDIT-15).
// DECISION: a title edit carries only the view key (`TextTarget::View`),
// so a default view is recognised by its key shape, as `edit` writes it.
fn default_from_key(model: &Model, key: &str) -> Option<ViewRef> {
    let (scope, kind) = key.rsplit_once('-')?;
    let kind = [ViewKind::SystemContext, ViewKind::Container, ViewKind::Component]
        .into_iter()
        .find(|k| k.keyword() == kind)?;
    model.find(scope)?;
    Some(ViewRef::Default {
        scope: scope.to_owned(),
        kind,
    })
}

/// `[AddDefaultView]` when `view` is a default view not declared in
/// `model` (EDIT-15), else nothing.
fn add_default(model: &Model, view: &ViewRef) -> Vec<Edit> {
    match view {
        ViewRef::Default { scope, kind } if model.view(&view.key()).is_none() => vec![Edit::AddDefaultView {
            scope: scope.clone(),
            kind: *kind,
        }],
        _ => Vec::new(),
    }
}

/// A move or resize batch: the default view first when needed, the
/// EDIT-26 first write for every box of the view still without an `at`
/// line, then `own`.
fn layout_batch(
    model: &Model,
    view: &ViewRef,
    layout: &BTreeMap<String, (i32, i32)>,
    moved: Option<&BTreeMap<String, (i32, i32)>>,
    own: Vec<Edit>,
) -> Vec<Edit> {
    let mut out = add_default(model, view);
    // DECISION: the first write is any layout write while a box of the
    // view lacks an `at` line, not only when the view has none (EDIT-25's
    // "every box has an `at` line after the first write"); boxes with a
    // hand-written line keep it byte for byte (DSL-9), so only the missing
    // lines are added.
    let placed: Vec<ElementId> = model
        .view(&view.key())
        .map(|v| v.positions.iter().map(|a| a.element.value).collect())
        .unwrap_or_default();
    let mut entries: Vec<(ElementId, &String, (i32, i32))> = layout
        .iter()
        .filter_map(|(k, &c)| {
            let c = moved.and_then(|m| m.get(k)).copied().unwrap_or(c);
            model.find(k).filter(|id| !placed.contains(id)).map(|id| (id, k, c))
        })
        .collect();
    entries.sort_by_key(|(id, ..)| *id);
    out.extend(entries.into_iter().map(|(_, k, c)| Edit::Move {
        view: view.key(),
        element: k.clone(),
        centre: Some(c),
    }));
    // DECISION: an own edit already in the batch (a moved box's `at` line
    // of the first write) is not repeated: same file, half the inverse.
    for e in own {
        if !out.contains(&e) {
            out.push(e);
        }
    }
    out
}

/// An undo or redo step, with its moves rebuilt as an EDIT-26 first
/// write when their view has a box without an `at` line (edit.rs
/// header): every such box at its auto-placed centre, then the step.
fn rebuild_first_write(parsed: &Parsed, edits: &[Edit]) -> Vec<Edit> {
    let model = &parsed.model;
    let mut views: Vec<&String> = Vec::new();
    for e in edits {
        if let Edit::Move {
            view, centre: Some(_), ..
        } = e
            && !views.contains(&view)
        {
            views.push(view);
        }
    }
    let adds_view = |key: &str| {
        edits.iter().any(|e| matches!(e, Edit::AddDefaultView { scope, kind } if format!("{scope}-{}", kind.keyword()) == key))
    };
    let mut out: Vec<Edit> = Vec::new();
    for key in views {
        let Some(spec) = model.view(key) else { continue };
        if adds_view(key) {
            continue;
        }
        let placed: Vec<ElementId> = spec.positions.iter().map(|a| a.element.value).collect();
        let moved: Vec<ElementId> = edits
            .iter()
            .filter_map(|e| match e {
                Edit::Move {
                    view,
                    element,
                    centre: Some(_),
                } if view == key => model.find(element),
                _ => None,
            })
            .collect();
        // DECISION: the positions of the other boxes come from the placer
        // (deterministic, no routing on the UI thread, EDIT-25): the view
        // as the CLI would place it from the file now, written `at` lines
        // kept by the plan.
        let plan = c4rizer_core::view::plan(model, spec);
        if plan.boxes.iter().all(|b| placed.contains(&b.element)) {
            continue;
        }
        let centres = c4rizer_core::place::place(&plan, &Keep::default());
        let mut entries: BTreeMap<ElementId, (String, (i32, i32))> = BTreeMap::new();
        for b in &plan.boxes {
            if placed.contains(&b.element) || moved.contains(&b.element) {
                continue;
            }
            if let Some(k) = &model.element(b.element).key {
                let c = centres[b.id.0 as usize];
                entries.entry(b.element).or_insert((k.value.clone(), (c.x * GRID_PX, c.y * GRID_PX)));
            }
        }
        out.extend(entries.into_values().map(|(element, c)| Edit::Move {
            view: key.clone(),
            element,
            centre: Some(c),
        }));
    }
    if out.is_empty() {
        return edits.to_vec();
    }
    out.extend(edits.iter().cloned());
    out
}

/// The view kind a click into an element of `kind` opens (EDIT-15).
fn counting_kind(kind: ElementKind) -> Option<ViewKind> {
    match kind {
        ElementKind::SoftwareSystem => Some(ViewKind::Container),
        ElementKind::Container => Some(ViewKind::Component),
        _ => None,
    }
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

    use std::collections::BTreeMap;
    use std::path::PathBuf;
    use std::sync::atomic::AtomicBool;
    use std::time::{Duration, Instant};

    use c4rizer_core::edit::{RelationshipRef, StyleProperty, StyleTarget, TextTarget, ViewField};
    use c4rizer_core::geom::{GRID_PX, Point, Rect, snap_px_to_grid};
    use c4rizer_core::lint::Rule;
    use c4rizer_core::model::{Colour, Model, Parsed, Shape, ViewKind, parse};
    use c4rizer_core::pipeline::build_view;
    use c4rizer_core::place::Keep;
    use c4rizer_core::scene::{ItemKey, Scene, texts_fit};
    use c4rizer_core::view::BoxId;

    use super::{
        Action, Alignment, Axis, Button, Command, Dialog, Effect, Event, Gesture, HIT_TOLERANCE_PX, Input, Key, Mark,
        MarkKind, Modifiers, Problem, Selected, State, enterable, min_size,
    };
    use crate::camera::{Camera, ScreenPoint, ZOOM_STEP};
    use crate::files::{self, FileEvent, UiState, WriteRequest};
    use crate::testutil::{SHOP, TempDir};
    use crate::worker::{self, ExportFormat, Job, Request, ViewRef};

    const NONE: Modifiers = Modifiers {
        shift: false,
        ctrl: false,
        alt: false,
    };
    const CTRL: Modifiers = Modifiers {
        shift: false,
        ctrl: true,
        alt: false,
    };
    const SHIFT: Modifiers = Modifiers {
        shift: true,
        ctrl: false,
        alt: false,
    };
    const ALT: Modifiers = Modifiers {
        shift: false,
        ctrl: false,
        alt: true,
    };
    const CTRL_SHIFT: Modifiers = Modifiers {
        shift: true,
        ctrl: true,
        alt: false,
    };

    /// The context view's closing, where `at` lines go.
    const CONTEXT_END: &str = "            include *\n        }\n        container";
    /// The container view, whole.
    const CONTAINERS: &str = "        container shop \"containers\" \"Inside the shop\" {\n            include *\n        }\n";

    /// `SHOP` with `at` lines for the given boxes of the context view.
    fn shop_with(lines: &[(&str, i32, i32)]) -> String {
        use std::fmt::Write as _;
        let mut at = String::from("            include *\n");
        for (key, x, y) in lines {
            let _ = writeln!(at, "            at {key} {x} {y}");
        }
        at.push_str("        }\n        container");
        let text = SHOP.replacen(CONTEXT_END, &at, 1);
        assert_ne!(text, SHOP, "fixture edit applied");
        text
    }

    /// `SHOP` with every box of the context view placed.
    fn shop_at() -> String {
        shop_with(&[("customer", 300, 200), ("shop", 300, 700), ("bank", 1000, 700)])
    }

    fn declared(key: &str) -> ViewRef {
        ViewRef::Declared(key.to_owned())
    }

    fn pt(x: f32, y: f32) -> ScreenPoint {
        ScreenPoint { x, y }
    }

    fn colour(r: u8, g: u8, b: u8) -> Colour {
        Colour { r, g, b }
    }

    /// The shell, synchronously: runs every effect at once with `files`
    /// and `worker::serve`, unless told to hold writes or stall the
    /// worker.
    struct H {
        dir: TempDir,
        path: PathBuf,
        s: State,
        work: Vec<Request>,
        stall: bool,
        held_work: Vec<Request>,
        writes: Vec<WriteRequest>,
        hold_writes: bool,
        held_writes: Vec<WriteRequest>,
        dialogs: Vec<Dialog>,
        saved: Vec<UiState>,
    }

    impl H {
        fn empty() -> H {
            let dir = TempDir::new("state");
            let path = dir.join("shop.dsl");
            H {
                dir,
                path,
                s: State::new(UiState::default()),
                work: Vec::new(),
                stall: false,
                held_work: Vec::new(),
                writes: Vec::new(),
                hold_writes: false,
                held_writes: Vec::new(),
                dialogs: Vec::new(),
                saved: Vec::new(),
            }
        }

        fn with_file(text: &str) -> H {
            let mut h = H::empty();
            std::fs::write(&h.path, text).expect("write fixture");
            let path = h.path.clone();
            h.cmd(Command::Open(path));
            h
        }

        fn event(&mut self, event: Event) {
            let effects = self.s.handle(event);
            self.run(effects);
        }

        fn cmd(&mut self, command: Command) {
            self.event(Event::Command(command));
        }

        fn input(&mut self, input: Input) {
            self.event(Event::Input(input));
        }

        fn run(&mut self, effects: Vec<Effect>) {
            for effect in effects {
                let next = match effect {
                    Effect::Read(path) => {
                        let event = match files::read(&path) {
                            Ok((text, hash)) => FileEvent::Loaded { path, text, hash },
                            Err(e) => FileEvent::OpenFailed {
                                path,
                                error: e.to_string(),
                            },
                        };
                        self.s.handle(Event::File(event))
                    }
                    Effect::Watch(_) => Vec::new(),
                    Effect::Write(request) => {
                        self.writes.push(request.clone());
                        if self.hold_writes {
                            self.held_writes.push(request);
                            Vec::new()
                        } else {
                            let event = files::write_checked(&request);
                            self.s.handle(Event::File(event))
                        }
                    }
                    Effect::Work(request) => {
                        self.work.push(request.clone());
                        if self.stall {
                            self.held_work.push(request);
                            Vec::new()
                        } else {
                            self.serve(request)
                        }
                    }
                    Effect::Dialog(dialog) => {
                        self.dialogs.push(dialog);
                        Vec::new()
                    }
                    Effect::SaveUiState(ui) => {
                        self.saved.push(ui);
                        Vec::new()
                    }
                };
                self.run(next);
            }
        }

        fn serve(&mut self, request: Request) -> Vec<Effect> {
            match worker::serve(request, &AtomicBool::new(false)) {
                Some(reply) => self.s.handle(Event::Worker(reply)),
                None => Vec::new(),
            }
        }

        /// Executes the held writes and stops holding.
        fn release_writes(&mut self) {
            self.hold_writes = false;
            for request in std::mem::take(&mut self.held_writes) {
                let event = files::write_checked(&request);
                self.event(Event::File(event));
            }
        }

        fn model(&self) -> &Model {
            &self.s.doc.as_ref().expect("a file").parsed.as_ref().expect("a parse").model
        }

        fn scene(&self) -> &Scene {
            self.s.scene().expect("a scene")
        }

        fn box_id(&self, key: &str) -> BoxId {
            let id = self.model().find(key).expect("element");
            *self.scene().boxes_of(id).first().expect("box on screen")
        }

        fn rect(&self, key: &str) -> Rect {
            self.scene().boxes[self.box_id(key).0 as usize].rect
        }

        fn centre_px(&self, key: &str) -> (i32, i32) {
            let c = self.scene().centres[self.box_id(key).0 as usize];
            (c.x * GRID_PX, c.y * GRID_PX)
        }

        fn screen(&self, key: &str) -> ScreenPoint {
            self.s.camera.to_screen(self.rect(key).centre())
        }

        fn press(&mut self, at: ScreenPoint, button: Button, mods: Modifiers) {
            self.input(Input::PointerDown { at, button, mods });
        }

        fn move_to(&mut self, at: ScreenPoint) {
            self.input(Input::PointerMove { at });
        }

        fn up(&mut self, at: ScreenPoint, button: Button) {
            self.input(Input::PointerUp { at, button });
        }

        fn click_at(&mut self, at: ScreenPoint, button: Button, mods: Modifiers) {
            self.press(at, button, mods);
            self.up(at, button);
        }

        fn click(&mut self, key: &str, button: Button, mods: Modifiers) {
            let at = self.screen(key);
            self.click_at(at, button, mods);
        }

        /// A left drag from `from` by `(dx, dy)` screen px, in steps.
        fn drag(&mut self, from: ScreenPoint, dx: f32, dy: f32) {
            self.press(from, Button::Left, NONE);
            for i in 1..=4_u8 {
                let f = f32::from(i) / 4.0;
                self.move_to(pt(from.x + dx * f, from.y + dy * f));
            }
            self.up(pt(from.x + dx, from.y + dy), Button::Left);
        }

        fn drag_box(&mut self, key: &str, dx: f32, dy: f32) {
            let from = self.screen(key);
            self.drag(from, dx, dy);
        }

        fn key(&mut self, key: Key, mods: Modifiers) {
            self.input(Input::KeyDown {
                key,
                mods,
                text_focus: false,
            });
        }

        fn text(&self) -> String {
            std::fs::read_to_string(&self.path).expect("read diagram")
        }

        fn on_disk(&self) -> Parsed {
            parse(&self.text()).expect("file on disk parses")
        }

        /// An external editor saves `text` (watcher event included).
        fn external(&mut self, text: &str) {
            std::fs::write(&self.path, text).expect("external write");
            self.event(Event::File(FileEvent::Changed {
                text: text.to_owned(),
                hash: files::hash(text.as_bytes()),
            }));
        }

        /// The `at` lines of `view` in the file on disk, by element key.
        fn at_lines(&self, view: &str) -> BTreeMap<String, (i32, i32)> {
            let parsed = self.on_disk();
            let model = &parsed.model;
            model
                .view(view)
                .expect("view on disk")
                .positions
                .iter()
                .map(|a| {
                    let key = model.element(a.element.value).key.as_ref().expect("key").value.clone();
                    (key, (a.x.value, a.y.value))
                })
                .collect()
        }

        /// A screen point over no scene item.
        fn empty_point(&self) -> ScreenPoint {
            let scene = self.scene();
            let b = scene.bounds;
            let mut y = b.min.y + 2;
            while y < b.max.y {
                let mut x = b.min.x + 2;
                while x < b.max.x {
                    let p = Point::new(x, y);
                    if scene.hit(p, HIT_TOLERANCE_PX + 6).is_none() {
                        return self.s.camera.to_screen(p);
                    }
                    x += 10;
                }
                y += 10;
            }
            panic!("no empty canvas point");
        }

        fn selection(&self) -> Vec<Selected> {
            self.s.selection.iter().cloned().collect()
        }
    }

    fn el(key: &str) -> Selected {
        Selected::Element(key.to_owned())
    }

    /// Lines of `b` not in `a`, and of `a` not in `b`.
    fn line_diff(a: &str, b: &str) -> (Vec<String>, Vec<String>) {
        let la: Vec<&str> = a.lines().collect();
        let lb: Vec<&str> = b.lines().collect();
        let added = lb.iter().filter(|l| !la.contains(l)).map(|l| (*l).to_owned()).collect();
        let removed = la.iter().filter(|l| !lb.contains(l)).map(|l| (*l).to_owned()).collect();
        (added, removed)
    }

    /// EDIT-1 (plan editor-06): an external save reloads the view, keeping
    /// the selected view, zoom, pan and selection.
    #[test]
    fn edit_1_external_save_reloads_keeping_view() {
        let mut h = H::with_file(SHOP);
        h.cmd(Command::ShowView(declared("containers")));
        h.input(Input::Wheel {
            at: pt(120.0, 80.0),
            notches: 2.0,
        });
        h.press(pt(10.0, 10.0), Button::Middle, NONE);
        h.move_to(pt(40.0, 30.0));
        h.up(pt(40.0, 30.0), Button::Middle);
        h.click("db", Button::Left, CTRL);
        let camera = h.s.camera;
        let builds = h.work.len();

        h.external(&SHOP.replace("Stores orders.", "Stores every order."));

        assert_eq!(h.s.view, Some(declared("containers")));
        assert_eq!(h.s.camera, camera);
        assert_eq!(h.selection(), vec![el("db")]);
        assert!(h.work.len() > builds, "the view was rebuilt");
        let details = h.s.details().expect("details of the selected box");
        assert_eq!(details.description.as_deref(), Some("Stores every order."));
        let shown = h.scene().boxes[h.box_id("db").0 as usize].description.clone().expect("description");
        assert_eq!(shown.lines.join(" "), "Stores every order.");
    }

    /// EDIT-2 (plan editor-04): dragging snaps the box centre to the 10px
    /// grid and sends `Drag` requests while dragging.
    #[test]
    fn edit_2_drag_snaps_to_grid() {
        let mut h = H::with_file(SHOP);
        let id = h.box_id("customer");
        let (cx, cy) = h.centre_px("customer");
        let from = h.screen("customer");
        let sent = h.work.len();
        h.press(from, Button::Left, NONE);
        h.move_to(pt(from.x + 20.0, from.y + 10.0));
        h.move_to(pt(from.x + 37.0, from.y + 23.0));

        let want = Point::new((cx + 40) / GRID_PX, (cy + 20) / GRID_PX);
        match &h.s.gesture {
            Gesture::Dragging { to, .. } => assert_eq!(to.get(&id), Some(&want)),
            g => panic!("not dragging: {g:?}"),
        }
        assert_eq!(h.centre_px("customer"), (cx + 40, cy + 20), "the box is drawn snapped");
        let drags: Vec<&Request> = h.work[sent..].iter().filter(|r| matches!(r.job, Job::Drag { .. })).collect();
        assert_eq!(drags.len(), 2, "one drag request per new snapped centre");
        match &drags[1].job {
            Job::Drag { moved, .. } => assert_eq!(moved.get(&id), Some(&want)),
            _ => unreachable!(),
        }
        // A move that snaps to the same centre sends nothing.
        h.move_to(pt(from.x + 38.0, from.y + 22.0));
        assert_eq!(h.work[sent..].iter().filter(|r| matches!(r.job, Job::Drag { .. })).count(), 2);
    }

    /// EDIT-3 (plan editor-04): release writes the box's `at` line
    /// (DSL-9), the file holds it, the next reload shows it there.
    #[test]
    fn edit_3_release_writes_at_line() {
        let mut h = H::with_file(SHOP);
        let (cx, cy) = h.centre_px("customer");
        h.drag_box("customer", 100.0, 0.0);

        assert_eq!(h.writes.len(), 1);
        assert!(h.text().contains(&format!("at customer {} {cy}", cx + 100)), "{}", h.text());
        assert_eq!(h.at_lines("context").get("customer"), Some(&(cx + 100, cy)));
        assert!(h.s.preview.is_none(), "the saved file's build replaced the preview");
        assert_eq!(h.centre_px("customer"), (cx + 100, cy));
    }

    /// EDIT-4: an auto-placed box is dragged, released, written, and
    /// keeps the dragged position after the reload.
    #[test]
    fn edit_4_auto_placed_box_keeps_dragged_position() {
        let mut h = H::with_file(&shop_with(&[("customer", 300, 200), ("shop", 300, 700)]));
        assert!(!h.at_lines("context").contains_key("bank"), "bank is auto-placed");
        let (bx, by) = h.centre_px("bank");
        h.drag_box("bank", 0.0, 100.0);

        let at = h.at_lines("context");
        assert_eq!(at.get("bank"), Some(&(bx, by + 100)));
        assert_eq!(at.get("customer"), Some(&(300, 200)), "other lines untouched");
        assert_eq!(h.centre_px("bank"), (bx, by + 100));

        let text = h.text().replace("Buys things.", "Buys many things.");
        h.external(&text);
        assert_eq!(h.centre_px("bank"), (bx, by + 100), "kept after the reload");
    }

    /// EDIT-5 (plan editor-05): Enter on a selected box opens the name
    /// field, Tab moves to description then technology, Enter writes
    /// (DSL-10), Escape cancels; on a lifted line it edits the
    /// relationship it was lifted from.
    #[test]
    fn edit_5_enter_edits_text() {
        let mut h = H::with_file(SHOP);
        h.cmd(Command::ShowView(declared("containers")));
        h.click("web", Button::Left, CTRL);
        h.key(Key::Enter, NONE);
        let edit = h.s.text_edit.clone().expect("editor open");
        let labels: Vec<&str> = edit.fields.iter().map(|f| f.label).collect();
        assert_eq!(labels, ["name", "description", "technology"]);
        assert_eq!(edit.current, 0);
        assert_eq!(edit.fields[0].buffer, "Web app");

        h.event(Event::Text("Storefront".to_owned()));
        h.key(Key::Escape, NONE);
        assert!(h.s.text_edit.is_none());
        assert!(h.writes.is_empty(), "Escape writes nothing");
        assert_eq!(h.text(), SHOP);

        h.key(Key::F2, NONE);
        h.event(Event::Text("Storefront".to_owned()));
        h.key(Key::Tab, NONE);
        assert_eq!(h.s.text_edit.as_ref().map(|e| e.current), Some(1));
        h.key(Key::Tab, NONE);
        assert_eq!(h.s.text_edit.as_ref().map(|e| e.current), Some(2));
        h.event(Event::Text("Rust 2024".to_owned()));
        h.key(Key::Enter, NONE);
        assert!(h.s.text_edit.is_none());
        assert_eq!(h.writes.len(), 1, "one batch");
        assert!(h.text().contains("container \"Storefront\" \"Serves pages.\" \"Rust 2024\""), "{}", h.text());
        assert_eq!(h.s.history.undo.len(), 1, "one undo step");

        // A lifted line edits the relationship it was lifted from.
        h.cmd(Command::ShowView(declared("context")));
        let model = h.model();
        let (shop, bank) = (model.find("shop").expect("shop"), model.find("bank").expect("bank"));
        let scene = h.scene();
        let line = scene
            .lines
            .iter()
            .find(|l| scene.boxes[l.source.0 as usize].element == shop && scene.boxes[l.destination.0 as usize].element == bank)
            .expect("lifted line shop -> bank")
            .clone();
        let winner = RelationshipRef::of(model, line.relationship);
        let longest = line
            .points
            .windows(2)
            .max_by_key(|w| (w[0].x - w[1].x).abs() + (w[0].y - w[1].y).abs())
            .expect("a segment");
        let mid = Point::new(i32::midpoint(longest[0].x, longest[1].x), i32::midpoint(longest[0].y, longest[1].y));
        assert!(matches!(scene.hit(mid, HIT_TOLERANCE_PX), Some(ItemKey::Line(_) | ItemKey::Label(_))));
        let at = h.s.camera.to_screen(mid);
        h.click_at(at, Button::Left, CTRL);
        assert_eq!(h.selection(), vec![Selected::Relationship(winner.clone())]);
        h.key(Key::Enter, NONE);
        let labels: Vec<&str> = h.s.text_edit.as_ref().expect("editor").fields.iter().map(|f| f.label).collect();
        assert_eq!(labels, ["description", "technology"]);
        h.event(Event::Text("Settles through".to_owned()));
        h.key(Key::Enter, NONE);
        assert!(
            h.text().contains(&format!("{} -> bank \"Settles through\"", winner.source)),
            "{}",
            h.text()
        );
        assert!(h.text().contains("customer -> shop \"Visits\" \"HTTPS\""));
    }

    /// EDIT-5: the details panel of a lifted line names its relationship
    /// and lists the merged ones.
    #[test]
    fn edit_5_lifted_line_details_name_relationship() {
        let mut h = H::with_file(SHOP);
        let model = h.model();
        let (shop, bank) = (model.find("shop").expect("shop"), model.find("bank").expect("bank"));
        let scene = h.scene();
        let line = scene
            .lines
            .iter()
            .find(|l| scene.boxes[l.source.0 as usize].element == shop && scene.boxes[l.destination.0 as usize].element == bank)
            .expect("lifted line")
            .clone();
        let name = |r: c4rizer_core::model::RelationshipId| {
            let rel = model.relationship(r);
            format!(
                "{} -> {}",
                model.element(rel.source.value).name.value,
                model.element(rel.destination.value).name.value
            )
        };
        let lifted = name(line.relationship);
        let other: Vec<String> = line.relationships.iter().filter(|&&r| r != line.relationship).map(|&r| name(r)).collect();
        let lifted_ref = RelationshipRef::of(model, line.relationship);
        let customer = model.find("customer").expect("customer");
        let direct = RelationshipRef::of(model, model.find_relationship(customer, shop)[0]);
        h.s.selection.insert(Selected::Relationship(lifted_ref));

        let d = h.s.details().expect("details");
        assert_eq!(d.kind, "Relationship");
        assert_eq!(d.lifted_from.as_deref(), Some(lifted.as_str()));
        assert!(lifted == "Payments -> Bank" || lifted == "Cart -> Bank", "{lifted}");
        assert_eq!(d.merged, other);
        assert_eq!(d.merged.len(), 1);
        assert_ne!(d.merged[0], lifted);

        // A direct line is not lifted.
        h.s.selection.clear();
        h.s.selection.insert(Selected::Relationship(direct));
        let d = h.s.details().expect("details");
        assert_eq!(d.lifted_from, None);
        assert_eq!(d.name, "Visits");
    }

    /// EDIT-6 (plan editor-07): findings follow each render, the drag
    /// preview's included (`State::dragged`'s report while dragging);
    /// picking one selects its items.
    #[test]
    fn edit_6_lint_findings_update() {
        let mut h = H::with_file(&shop_at());
        let shown = h.s.shown.as_ref().expect("built").1.report.findings.clone();
        assert_eq!(h.s.findings(), shown.as_slice());
        assert!(!shown.iter().any(|f| f.rule == Rule::TooClose));

        let from = h.screen("customer");
        let onto = h.screen("shop");
        h.press(from, Button::Left, NONE);
        h.move_to(onto);
        assert!(h.s.dragged.is_some(), "the drag preview build arrived");
        let n = h
            .s
            .findings()
            .iter()
            .position(|f| f.rule == Rule::TooClose)
            .expect("the preview's overlap is reported");
        h.cmd(Command::PickFinding(n));
        let picked = h.selection();
        assert!(picked.contains(&el("customer")) && picked.contains(&el("shop")), "{picked:?}");

        h.up(onto, Button::Left);
        let shown = h.s.shown.as_ref().expect("built").1.report.findings.clone();
        assert_eq!(h.s.findings(), shown.as_slice(), "the saved file's findings");
        assert!(shown.iter().any(|f| f.rule == Rule::TooClose));
    }

    /// EDIT-7 (plan editor-07 `edit_7_export_svg_and_png`, SVG half):
    /// "Export view" asks the save dialog with the diagram's folder and
    /// `<key>.svg`; the answer sends an `Export` job; the file written
    /// equals `export::svg` of the shown scene.
    #[test]
    fn edit_7_export_svg() {
        let mut h = H::with_file(SHOP);
        h.cmd(Command::ExportView(ExportFormat::Svg));
        assert_eq!(
            h.dialogs.last(),
            Some(&Dialog::ExportView {
                view: declared("context"),
                dir: h.dir.path().to_path_buf(),
                name: "context.svg".to_owned(),
                format: ExportFormat::Svg,
            })
        );
        let out = h.dir.join("context.svg");
        h.cmd(Command::Export {
            targets: vec![(declared("context"), out.clone())],
            format: ExportFormat::Svg,
        });
        assert!(matches!(h.work.last().map(|r| &r.job), Some(Job::Export { .. })));
        let written = std::fs::read_to_string(&out).expect("svg written");
        assert_eq!(written, c4rizer_core::export::svg(h.scene()));
        assert!(h.s.status().notice.is_some_and(|n| n.contains("exported")));

        h.cmd(Command::ExportAll(ExportFormat::Svg));
        assert_eq!(
            h.dialogs.last(),
            Some(&Dialog::ExportAll {
                dir: h.dir.path().to_path_buf(),
                format: ExportFormat::Svg,
            })
        );
    }

    /// EDIT-7 (PNG half): `ExportFormat::Png` asks for `<key>.png`, and
    /// the job writes `export::png` at twice the scene width, through
    /// core feature `png` (on in the editor's dependency) and the bundled
    /// font (`scene::FONT_REGULAR`, `scene::FONT_BOLD`).
    #[test]
    fn edit_7_export_png() {
        let mut h = H::with_file(SHOP);
        h.cmd(Command::ExportView(ExportFormat::Png));
        assert!(matches!(h.dialogs.last(), Some(Dialog::ExportView { name, .. }) if name == "context.png"));
        let out = h.dir.join("context.png");
        h.cmd(Command::Export {
            targets: vec![(declared("context"), out.clone())],
            format: ExportFormat::Png,
        });
        let bytes = std::fs::read(&out).expect("png written");
        assert_eq!(&bytes[..8], b"\x89PNG\r\n\x1a\n");
        let width = u32::from_be_bytes([bytes[16], bytes[17], bytes[18], bytes[19]]);
        let scene_width = u32::try_from(h.scene().bounds.width()).expect("positive width");
        assert_eq!(width, 2 * scene_width);
        let expected = c4rizer_core::export::png(h.scene(), 2 * scene_width).expect("png");
        assert_eq!(bytes, expected);
    }

    /// EDIT-8: every view is listed; picking one shows it.
    #[test]
    fn edit_8_views_listed_and_picked() {
        let mut h = H::with_file(SHOP);
        let views = h.s.views();
        let listed: Vec<(&str, bool)> = views.iter().map(|v| (v.label.as_str(), v.current)).collect();
        assert_eq!(listed, [("The shop and its users", true), ("Inside the shop", false)]);
        assert_eq!(views[1].kind, ViewKind::Container);

        h.cmd(Command::ShowView(views[1].view.clone()));
        assert_eq!(h.s.view, Some(declared("containers")));
        assert_eq!(h.scene().key, "containers");
        let current: Vec<bool> = h.s.views().iter().map(|v| v.current).collect();
        assert_eq!(current, [false, true]);
    }

    /// EDIT-9 (plan editor-03): wheel zooms at the cursor and middle drag
    /// pans; the scene is unchanged.
    #[test]
    fn edit_9_zoom_keeps_diagram() {
        let mut h = H::with_file(SHOP);
        let scene = h.scene().clone();
        let builds = h.work.len();
        let cursor = pt(150.0, 90.0);
        let under = h.s.camera.to_scene(cursor);
        h.input(Input::Wheel { at: cursor, notches: 1.0 });
        assert!((h.s.camera.zoom - ZOOM_STEP).abs() < 1e-5);
        let still = h.s.camera.to_scene(cursor);
        assert!((still.x - under.x).abs() < 1e-3 && (still.y - under.y).abs() < 1e-3, "the point under the cursor stays");

        let pan = h.s.camera.pan;
        h.press(pt(50.0, 50.0), Button::Middle, NONE);
        h.move_to(pt(80.0, 90.0));
        h.up(pt(80.0, 90.0), Button::Middle);
        let zoom = h.s.camera.zoom;
        assert!((h.s.camera.pan.x - (pan.x - 30.0 / zoom)).abs() < 1e-3);
        assert!((h.s.camera.pan.y - (pan.y - 40.0 / zoom)).abs() < 1e-3);

        assert_eq!(h.scene(), &scene, "the diagram is unchanged");
        assert_eq!(h.work.len(), builds, "nothing rebuilt");
        assert!(h.writes.is_empty());
    }

    /// EDIT-10 (plan editor-04): Ctrl-click and right-click select without
    /// entering; details and style panel show the selected box.
    #[test]
    fn edit_10_ctrl_click_selects() {
        let mut h = H::with_file(SHOP);
        h.click("shop", Button::Left, CTRL);
        assert_eq!(h.s.view, Some(declared("context")), "Ctrl-click never enters");
        assert_eq!(h.selection(), vec![el("shop")]);
        let d = h.s.details().expect("details");
        assert_eq!((d.name.as_str(), d.kind.as_str()), ("Shop", "Software System"));
        assert_eq!(d.description.as_deref(), Some("Sells things."));
        assert_eq!(h.s.style_panel().map(|p| p.boxes), Some(1));

        h.click("shop", Button::Right, NONE);
        assert_eq!(h.s.view, Some(declared("context")), "right-click never enters");
        h.click("bank", Button::Right, NONE);
        assert_eq!(h.selection(), vec![el("bank")]);
        let d = h.s.details().expect("details");
        assert_eq!(d.kind, "External Software System");
        assert!(d.tags.contains(&"External".to_owned()));
        let panel = h.s.style_panel().expect("style panel");
        assert_eq!(panel.boxes, 1);
        assert_eq!(panel.background, Some(h.model().style_of(h.model().find("bank").expect("bank")).background));
    }

    /// EDIT-11 (plan editor-06): a parse error keeps the last good scene,
    /// shows line and column, refuses every edit and undo; the next good
    /// parse resumes editing; opening a broken file shows an empty canvas
    /// and still reloads.
    #[test]
    fn edit_11_parse_error_keeps_last_good() {
        let mut h = H::with_file(SHOP);
        h.click("customer", Button::Left, CTRL);
        h.cmd(Command::SetStyle {
            target: StyleTarget::ThisBox,
            property: StyleProperty::Background(Some(colour(0x11, 0x22, 0x33))),
        });
        assert!(h.s.history.can_undo());
        let good = h.scene().clone();
        let broken = SHOP.replacen("    model {\n", "    model {\n        )\n", 1);

        h.external(&broken);
        match h.s.status().problem {
            Some(Problem::Parse(e)) => {
                assert_eq!(e.position.line, 3);
                assert!(e.to_string().starts_with(&format!("{}:{}", e.position.line, e.position.column)), "{e}");
            }
            p => panic!("no parse problem: {p:?}"),
        }
        assert_eq!(h.scene(), &good, "the last good diagram stays");
        assert!(h.s.read_only() && h.s.draw_list().read_only);
        let writes = h.writes.len();
        h.drag_box("customer", 100.0, 0.0);
        h.cmd(Command::Undo);
        assert!(h.s.status().notice.is_some(), "the refused undo is reported");
        h.key(Key::Enter, NONE);
        assert!(h.s.text_edit.is_none());
        assert_eq!(h.writes.len(), writes, "every edit and undo refused");
        assert!(h.s.history.can_undo(), "the undo step is kept");
        assert_eq!(h.text(), broken);

        h.external(SHOP);
        assert_eq!(h.s.status().problem, None);
        assert!(!h.s.read_only());
        h.drag_box("customer", 100.0, 0.0);
        assert_eq!(h.writes.len(), writes + 1, "editing resumes");

        // Opening a broken file: empty canvas, reload still works.
        let mut h = H::with_file(&broken);
        assert!(h.s.scene().is_none());
        assert!(matches!(h.s.status().problem, Some(Problem::Parse(_))));
        h.external(SHOP);
        assert!(h.s.scene().is_some());
        assert_eq!(h.s.view, Some(declared("context")));
    }

    /// EDIT-13 (plan editor-01): open remembers the file in `recent`,
    /// restores the last view if it still exists, else the first
    /// declared.
    #[test]
    fn edit_13_open_restores_last_view() {
        let mut h = H::empty();
        let (a, b) = (h.dir.join("a.dsl"), h.dir.join("b.dsl"));
        std::fs::write(&a, SHOP).expect("a");
        std::fs::write(&b, SHOP).expect("b");

        h.cmd(Command::Open(a.clone()));
        assert_eq!(h.s.ui.recent, vec![a.clone()]);
        assert_eq!(h.saved.last().map(|u| u.recent.clone()), Some(vec![a.clone()]));
        h.cmd(Command::ShowView(declared("containers")));
        h.input(Input::Wheel {
            at: pt(10.0, 10.0),
            notches: 3.0,
        });
        let camera = h.s.camera;

        h.cmd(Command::Open(b.clone()));
        assert_eq!(h.s.ui.recent, vec![b.clone(), a.clone()]);
        assert_eq!(h.s.view, Some(declared("context")));

        h.cmd(Command::Open(a.clone()));
        assert_eq!(h.s.view, Some(declared("containers")), "last view restored");
        assert_eq!(h.s.camera, camera, "zoom and pan restored");
        assert_eq!(h.s.ui.recent, vec![a.clone(), b.clone()]);

        std::fs::write(&a, SHOP.replace(CONTAINERS, "")).expect("a without containers");
        h.cmd(Command::Open(b.clone()));
        h.cmd(Command::Open(a.clone()));
        assert_eq!(h.s.view, Some(declared("context")), "a vanished view gives the first declared");

        let gone = h.dir.join("gone.dsl");
        h.s.ui.opened(&gone);
        h.cmd(Command::Open(gone.clone()));
        assert!(matches!(h.s.status().problem, Some(Problem::Io(_))));
        assert!(!h.s.ui.recent.contains(&gone), "a missing file leaves the recent list");
    }

    /// EDIT-14: on open, the shown view's build is requested before any
    /// background build of the other views.
    #[test]
    fn edit_14_shown_view_requested_first() {
        let mut h = H::with_file(SHOP);
        let builds: Vec<(ViewRef, bool)> = h
            .work
            .iter()
            .filter_map(|r| match &r.job {
                Job::Build { view, shown, .. } => Some((view.clone(), *shown)),
                _ => None,
            })
            .collect();
        assert_eq!(builds.first(), Some(&(declared("context"), true)));
        assert!(builds[1..].iter().all(|(_, shown)| !shown));
        assert!(builds.contains(&(declared("containers"), false)));

        let sent = h.work.len();
        h.cmd(Command::ShowView(declared("containers")));
        assert_eq!(h.work.len(), sent, "the background build is shown from the cache");
        assert_eq!(h.scene().key, "containers");
    }

    /// EDIT-15 (plan editor-03): a click on an enterable system opens its
    /// container view; a non-enterable box is selected; hover marks
    /// enterable boxes.
    #[test]
    fn edit_15_click_system_opens_container_view() {
        let mut h = H::with_file(SHOP);
        let shop = h.box_id("shop");
        let bank = h.box_id("bank");
        let at = h.screen("shop");
        h.move_to(at);
        assert!(h.s.draw_list().marks.contains(&Mark {
            item: ItemKey::Box(shop),
            kind: MarkKind::Enterable
        }));
        let at = h.screen("bank");
        h.move_to(at);
        assert!(h.s.draw_list().marks.contains(&Mark {
            item: ItemKey::Box(bank),
            kind: MarkKind::Hover
        }));

        h.click("bank", Button::Left, NONE);
        assert_eq!(h.s.view, Some(declared("context")));
        assert_eq!(h.selection(), vec![el("bank")]);

        h.click("shop", Button::Left, NONE);
        assert_eq!(h.s.view, Some(declared("containers")));
        assert_eq!(h.scene().key, "containers");
    }

    /// EDIT-15 (plan editor-03): with no view scoped to the box, its
    /// default view opens; the first write in it starts with
    /// `AddDefaultView`; undoing that write removes it again.
    #[test]
    fn edit_15_default_view_when_none() {
        let mut h = H::with_file(SHOP);
        h.cmd(Command::ShowView(declared("containers")));
        h.click("web", Button::Left, NONE);
        let default = ViewRef::Default {
            scope: "web".to_owned(),
            kind: ViewKind::Component,
        };
        assert_eq!(h.s.view, Some(default.clone()));
        assert_eq!(h.scene().key, "web-component");
        let (cx, cy) = h.centre_px("cart");
        h.centre_px("pay");

        let parsed = h.s.doc.as_ref().and_then(|d| d.parsed.clone()).expect("parse");
        let batch = h.s.edits_for(
            &Action::Move {
                view: default.clone(),
                boxes: BTreeMap::from([("cart".to_owned(), (cx + 100, cy))]),
                layout: BTreeMap::new(),
            },
            &parsed,
        );
        assert_eq!(
            batch.first(),
            Some(&c4rizer_core::edit::Edit::AddDefaultView {
                scope: "web".to_owned(),
                kind: ViewKind::Component
            })
        );

        h.drag_box("cart", 100.0, 0.0);
        assert_eq!(h.writes.len(), 1, "one write");
        assert_eq!(h.at_lines("web-component").get("cart"), Some(&(cx + 100, cy)));
        assert_eq!(h.s.view, Some(declared("web-component")));

        h.cmd(Command::Undo);
        assert!(h.on_disk().model.view("web-component").is_none(), "undo removes the view again");
        assert_eq!(h.s.view, Some(default));
        assert_eq!(h.scene().key, "web-component");
    }

    /// EDIT-15: with several container views scoped to a system, a click
    /// opens the first declared one and the view list shows the others;
    /// a childless system with only a system context view scoped to it
    /// is not enterable (EDIT-15).
    #[test]
    fn edit_15_several_views_first_declared_opens() {
        let text = SHOP
            .replace(
                "        styles {",
                "        container shop \"again\" \"Again\" {\n            include *\n        }\n        systemContext lone \"lonely\" \"Alone\" {\n            include *\n        }\n        styles {",
            )
            .replace("        customer -> shop", "        lone = softwareSystem \"Lone\" \"Stands alone.\"\n        customer -> shop");
        let mut h = H::with_file(&text);
        h.click("shop", Button::Left, NONE);
        assert_eq!(h.s.view, Some(declared("containers")));
        let listed: Vec<ViewRef> = h.s.views().into_iter().map(|v| v.view).collect();
        assert_eq!(listed, [declared("context"), declared("containers"), declared("again"), declared("lonely")]);
        let lone = h.model().find("lone").expect("lone");
        assert!(!enterable(h.model(), lone));
        let shop = h.model().find("shop").expect("shop");
        assert!(enterable(h.model(), shop));
    }

    /// EDIT-15 + EDIT-24: a move in a default view, then a switch to
    /// another view before `ChangedOnDisk` arrives: the re-applied batch
    /// still starts with `AddDefaultView` for the view the move was made
    /// in and moves its boxes, not the new view's.
    #[test]
    fn edit_15_reapply_after_view_switch_targets_action_view() {
        let mut h = H::with_file(SHOP);
        h.cmd(Command::ShowView(declared("containers")));
        h.click("web", Button::Left, NONE);
        let (cx, cy) = h.centre_px("cart");
        h.hold_writes = true;
        h.drag_box("cart", 100.0, 0.0);
        assert_eq!(h.held_writes.len(), 1);

        std::fs::write(&h.path, SHOP.replace("Buys things.", "Buys lots.")).expect("external");
        h.cmd(Command::ShowView(declared("context")));
        h.release_writes();

        assert_eq!(h.writes.len(), 2, "refused, then re-applied");
        let parsed = h.on_disk();
        assert!(h.text().contains("Buys lots."), "the external change is kept");
        assert_eq!(h.at_lines("web-component").get("cart"), Some(&(cx + 100, cy)));
        assert!(parsed.model.view("context").expect("context").positions.is_empty(), "nothing moved in the new view");
        assert_eq!(h.s.view, Some(declared("context")));
    }

    /// EDIT-15: confirming a title or subtitle edit in a default view not
    /// yet written is a write: its batch starts with `AddDefaultView`,
    /// then sets the text; before it the drawn title is the scope's name.
    #[test]
    fn edit_15_title_edit_writes_default_view() {
        let mut h = H::with_file(SHOP);
        h.cmd(Command::ShowView(declared("containers")));
        h.click("web", Button::Left, NONE);
        let title = h.scene().title.clone().expect("a drawn title");
        assert_eq!(title.lines.join(" "), "Web app");

        let parsed = h.s.doc.as_ref().and_then(|d| d.parsed.clone()).expect("parse");
        let target = TextTarget::View {
            view: "web-component".to_owned(),
            field: ViewField::Title,
        };
        let batch = h.s.edits_for(
            &Action::SetText {
                fields: vec![(target, Some("Inside".to_owned()), None)],
            },
            &parsed,
        );
        assert!(matches!(batch.first(), Some(c4rizer_core::edit::Edit::AddDefaultView { scope, .. }) if scope == "web"));
        assert!(matches!(batch.get(1), Some(c4rizer_core::edit::Edit::SetText { .. })));

        let bounds = h.scene().item_bounds(ItemKey::Title).expect("title bounds");
        let at = h.s.camera.to_screen(bounds.centre());
        h.click_at(at, Button::Left, CTRL);
        assert_eq!(h.selection(), vec![Selected::Title]);
        let d = h.s.details().expect("details");
        assert_eq!((d.name.as_str(), d.kind.as_str()), ("Web app", "View"));
        h.key(Key::Enter, NONE);
        let labels: Vec<&str> = h.s.text_edit.as_ref().expect("editor").fields.iter().map(|f| f.label).collect();
        assert_eq!(labels, ["title", "subtitle"]);
        h.event(Event::Text("Web internals".to_owned()));
        h.key(Key::Enter, NONE);
        let parsed = h.on_disk();
        let spec = parsed.model.view("web-component").expect("default view written");
        assert_eq!(spec.title.as_ref().map(|t| t.value.as_str()), Some("Web internals"));
        assert_eq!(h.s.view, Some(declared("web-component")));
    }

    /// EDIT-15: a model-only file shows the system context default view
    /// of its first software system.
    #[test]
    fn edit_15_model_only_file_shows_default_context_view() {
        let text = "workspace \"Shop\" {\n    model {\n        customer = person \"Customer\" \"Buys things.\"\n        shop = softwareSystem \"Shop\" \"Sells things.\"\n        customer -> shop \"Visits\"\n    }\n}\n";
        let h = H::with_file(text);
        assert_eq!(
            h.s.view,
            Some(ViewRef::Default {
                scope: "shop".to_owned(),
                kind: ViewKind::SystemContext
            })
        );
        assert_eq!(h.scene().boxes.len(), 2);
        assert_eq!(h.s.views().len(), 1, "the default view is listed");
    }

    /// EDIT-16 (plan editor-03): the breadcrumb follows the scope in the
    /// model; Alt+Left and Backspace open the parent crumb; Backspace is
    /// ignored while a text field has focus.
    #[test]
    fn edit_16_back_goes_up() {
        let mut h = H::with_file(SHOP);
        let crumbs = |h: &H| -> Vec<Option<String>> { h.s.breadcrumb().into_iter().map(|c| c.scope).collect() };
        assert_eq!(crumbs(&h), [None, Some("shop".to_owned())]);
        assert_eq!(h.s.breadcrumb()[0].label, "Shop");
        h.click("shop", Button::Left, NONE);
        h.click("web", Button::Left, NONE);
        assert_eq!(crumbs(&h), [None, Some("shop".to_owned()), Some("web".to_owned())]);
        assert_eq!(h.s.breadcrumb()[2].label, "Web app");

        h.input(Input::KeyDown {
            key: Key::Backspace,
            mods: NONE,
            text_focus: true,
        });
        assert_eq!(h.scene().key, "web-component", "Backspace in a text field is ignored");
        h.key(Key::Backspace, NONE);
        assert_eq!(h.s.view, Some(declared("containers")));
        h.key(Key::ArrowLeft, ALT);
        assert_eq!(h.s.view, Some(declared("context")));
        h.key(Key::ArrowLeft, ALT);
        assert_eq!(h.s.view, Some(declared("context")), "the top stays");
        h.key(Key::ArrowLeft, NONE);
        assert_eq!(h.s.view, Some(declared("context")));

        h.cmd(Command::Crumb(1));
        assert_eq!(h.s.view, Some(declared("containers")));
        h.cmd(Command::Crumb(0));
        assert_eq!(h.s.view, Some(declared("context")));
    }

    /// EDIT-17 (plan editor-05): the style panel writes colour, text
    /// colour and shape for every selected box as one undo step, with the
    /// DSL-13 target.
    #[test]
    fn edit_17_style_panel_writes_style() {
        let mut h = H::with_file(SHOP);
        h.cmd(Command::ShowView(declared("containers")));
        h.click("web", Button::Left, CTRL);
        h.click("db", Button::Left, SHIFT);
        let panel = h.s.style_panel().expect("panel");
        assert_eq!(panel.boxes, 2);
        assert_eq!(panel.shape, None, "the shapes differ");
        assert!(panel.tags.contains(&"Container".to_owned()));
        assert!(!panel.tags.contains(&"Database".to_owned()));

        let fill = colour(0x12, 0x34, 0x56);
        h.cmd(Command::SetStyle {
            target: StyleTarget::ThisBox,
            property: StyleProperty::Background(Some(fill)),
        });
        assert_eq!(h.writes.len(), 1);
        assert_eq!(h.s.history.undo.len(), 1, "one step for both boxes");
        let style = |h: &H, key: &str| {
            let parsed = h.on_disk();
            let id = parsed.model.find(key).expect("element");
            parsed.model.style_of(id)
        };
        assert_eq!(style(&h, "web").background, fill);
        assert_eq!(style(&h, "db").background, fill);
        assert_eq!(h.s.style_panel().and_then(|p| p.background), Some(fill));

        let ink = colour(0xfe, 0xdc, 0xba);
        h.cmd(Command::SetStyle {
            target: StyleTarget::Tag("Container".to_owned()),
            property: StyleProperty::Color(Some(ink)),
        });
        assert_eq!(style(&h, "web").color, ink);
        assert_eq!(style(&h, "db").color, ink);

        h.cmd(Command::SetStyle {
            target: StyleTarget::ThisBox,
            property: StyleProperty::Shape(Some(Shape::Box)),
        });
        assert_eq!(style(&h, "db").shape, Shape::Box);
        assert_eq!(h.s.style_panel().and_then(|p| p.shape), Some(Shape::Box));
        assert_eq!(h.s.history.undo.len(), 3);

        h.cmd(Command::Undo);
        assert_eq!(style(&h, "db").shape, Shape::Cylinder);
        h.cmd(Command::ResetStyle);
        assert_ne!(style(&h, "web").background, fill, "reset to tag style");
    }

    /// EDIT-18: resizing by a handle snaps to 20px steps about the centre,
    /// never below `min_size`, shows `DrawList::resize_outline` while
    /// dragging, and writes only the `size` line.
    #[test]
    fn edit_18_resize_on_grid_never_below_text() {
        let mut h = H::with_file(&shop_at());
        h.click("shop", Button::Left, CTRL);
        let id = h.box_id("shop");
        assert_eq!(
            h.s.draw_list().marks,
            vec![Mark {
                item: ItemKey::Box(id),
                kind: MarkKind::SelectedWithHandles
            }]
        );
        let start = h.rect("shop");
        let before = h.text();
        let corner = h.s.camera.to_screen(start.max);
        h.press(corner, Button::Left, NONE);
        h.move_to(pt(corner.x + 43.0, corner.y + 27.0));
        let size = match &h.s.gesture {
            Gesture::Resizing { size, .. } => *size,
            g => panic!("not resizing: {g:?}"),
        };
        assert_eq!(size.0 % 20, 0);
        assert_eq!(size.1 % 20, 0);
        assert!(size.0 > start.width() && size.1 > start.height(), "{size:?} vs {start:?}");
        let (low, high) = h.s.draw_list().resize_outline.expect("outline while resizing");
        let mid = h.s.camera.to_screen(start.centre());
        assert!(
            (f32::midpoint(low.x, high.x) - mid.x).abs() <= 1.0 && (f32::midpoint(low.y, high.y) - mid.y).abs() <= 1.0,
            "about the centre"
        );
        assert_eq!(h.scene().boxes[id.0 as usize].rect, start, "the box keeps its size until release");
        h.up(pt(corner.x + 43.0, corner.y + 27.0), Button::Left);

        let (added, removed) = line_diff(&before, &h.text());
        assert_eq!(added, [format!("            size shop {} {}", size.0, size.1)]);
        assert!(removed.is_empty(), "{removed:?}");
        assert_eq!(h.rect("shop").centre(), start.centre());
        assert_eq!((h.rect("shop").width(), h.rect("shop").height()), size);

        // Shrinking stops where the texts stop fitting.
        let r = h.rect("shop");
        let corner = h.s.camera.to_screen(r.max);
        h.press(corner, Button::Left, NONE);
        h.move_to(pt(corner.x - 2000.0, corner.y - 2000.0));
        let small = match &h.s.gesture {
            Gesture::Resizing { size, .. } => *size,
            g => panic!("not resizing: {g:?}"),
        };
        let plan_box = h.s.shown.as_ref().expect("built").1.plan.boxes[id.0 as usize].clone();
        assert!(texts_fit(&plan_box, small.0 / 2 / GRID_PX, small.1 / 2 / GRID_PX), "{small:?}");
        let floor = min_size(&plan_box);
        assert!(texts_fit(&plan_box, floor.0 / 2 / GRID_PX, floor.1 / 2 / GRID_PX));
        assert!(small.0 * small.1 >= floor.0 * floor.1, "{small:?} below {floor:?}");
        assert!(
            small.0 <= r.width() && small.1 <= r.height() && small != (r.width(), r.height()),
            "the box shrinks down to the floor: {small:?} from {r:?}, floor {floor:?}"
        );
        assert_ne!(small, (20, 20));
    }

    /// EDIT-19 (plan editor-05): 100 writes undo and redo in order through
    /// external reloads; a step whose target is gone is skipped with a
    /// status notice; a multi-box move is one step.
    #[test]
    fn edit_19_undo_redo_100_steps() {
        let mut h = H::with_file(&shop_at());

        // A multi-box move is one step.
        h.click("customer", Button::Left, CTRL);
        h.click("bank", Button::Left, SHIFT);
        h.drag_box("customer", 100.0, 0.0);
        assert_eq!(h.s.history.undo.len(), 1);
        let at = h.at_lines("context");
        assert_eq!((at["customer"], at["bank"]), ((400, 200), (1100, 700)));
        h.cmd(Command::Undo);
        let at = h.at_lines("context");
        assert_eq!((at["customer"], at["bank"]), ((300, 200), (1000, 700)));
        h.cmd(Command::Redo);
        assert_eq!(h.at_lines("context")["bank"], (1100, 700));

        // 100 steps through an external reload.
        h.click("customer", Button::Left, CTRL);
        let fill = |i: u8| colour(i, 0x40, 0x80);
        let bg = |h: &H| h.model().style_of(h.model().find("customer").expect("customer")).background;
        let original = bg(&h);
        h.s.history = crate::history::History::default();
        for i in 0..100_u8 {
            h.cmd(Command::SetStyle {
                target: StyleTarget::ThisBox,
                property: StyleProperty::Background(Some(fill(i))),
            });
            if i == 50 {
                let text = h.text().replace("Moves money.", "Moves lots of money.");
                h.external(&text);
            }
        }
        assert_eq!(h.s.history.undo.len(), 100);
        for i in (0..100_u8).rev() {
            assert_eq!(bg(&h), fill(i));
            h.key(Key::Z, CTRL);
        }
        assert_eq!(bg(&h), original);
        assert!(!h.s.history.can_undo() && h.s.history.redo.len() == 100);
        for i in 0..100_u8 {
            if i % 2 == 0 {
                h.key(Key::Y, CTRL);
            } else {
                h.key(Key::Z, CTRL_SHIFT);
            }
            assert_eq!(bg(&h), fill(i));
        }
        assert!(h.text().contains("Moves lots of money."), "the external change survives");

        // A step whose target changed under it is skipped with a notice.
        h.key(Key::Enter, NONE);
        h.key(Key::Tab, NONE);
        h.event(Event::Text("Buys gadgets.".to_owned()));
        h.key(Key::Enter, NONE);
        assert!(h.text().contains("Buys gadgets."));
        let text = h.text().replace("Buys gadgets.", "Buys gizmos.");
        h.external(&text);
        let steps = h.s.history.undo.len();
        let writes = h.writes.len();
        h.cmd(Command::Undo);
        assert_eq!(h.writes.len(), writes, "nothing written");
        assert!(h.s.status().notice.is_some_and(|n| n.contains("skipped")), "{:?}", h.s.status());
        assert_eq!(h.s.history.undo.len(), steps - 1, "the step is dropped");
        assert!(h.text().contains("Buys gizmos."));
        h.cmd(Command::Undo);
        assert_eq!(bg(&h), fill(98), "the next step still undoes");
    }

    /// EDIT-20 (plan editor-04): Shift-click and rubber band select
    /// several boxes, which move together and align and distribute as one
    /// step.
    #[test]
    fn edit_20_align_and_distribute() {
        let mut h = H::with_file(&shop_at());
        h.click("customer", Button::Left, NONE);
        h.click("shop", Button::Left, SHIFT);
        assert_eq!(h.s.view, Some(declared("context")), "Shift-click never enters");
        assert_eq!(h.selection(), vec![el("customer"), el("shop")]);

        let b = h.scene().bounds;
        let (from, to) = (h.s.camera.to_screen(b.min), h.s.camera.to_screen(b.max));
        assert!(!matches!(h.scene().hit(b.min, HIT_TOLERANCE_PX), Some(ItemKey::Box(_))));
        h.press(from, Button::Left, NONE);
        h.move_to(pt(f32::midpoint(from.x, to.x), f32::midpoint(from.y, to.y)));
        assert!(h.s.draw_list().rubber_band.is_some());
        h.move_to(to);
        h.up(to, Button::Left);
        assert_eq!(h.selection(), vec![el("bank"), el("customer"), el("shop")]);

        h.drag_box("shop", 0.0, 50.0);
        assert_eq!(h.writes.len(), 1, "one write");
        let at = h.at_lines("context");
        assert_eq!((at["customer"], at["shop"], at["bank"]), ((300, 250), (300, 750), (1000, 750)));

        h.cmd(Command::Align(Alignment::Top));
        assert_eq!(h.writes.len(), 2);
        let tops: Vec<i32> = ["customer", "shop", "bank"].iter().map(|k| h.rect(k).min.y).collect();
        assert!(tops.iter().all(|&t| t == tops[0]), "{tops:?}");

        h.cmd(Command::Distribute(Axis::Horizontal));
        assert_eq!(h.writes.len(), 3);
        let mut xs: Vec<i32> = ["customer", "shop", "bank"].iter().map(|k| h.centre_px(k).0).collect();
        xs.sort_unstable();
        assert_eq!(xs, [300, 650, 1000]);
        assert_eq!(h.s.history.undo.len(), 3, "each one step");
    }

    /// EDIT-21: no `Input` or `Command` yields an edit that creates or
    /// deletes an element or relationship; `RemoveDefaultView` only
    /// appears as an undo of `AddDefaultView`.
    #[test]
    #[expect(clippy::too_many_lines, reason = "one walk over every input and command")]
    fn edit_21_no_gesture_creates_or_deletes() {
        type Shape21 = (Vec<String>, Vec<(String, String)>, usize);
        fn shape(text: &str) -> Shape21 {
            let model = parse(text).expect("every write parses").model;
            let key = |id: c4rizer_core::model::ElementId| model.element(id).key.as_ref().map_or_else(String::new, |k| k.value.clone());
            let elements = model.elements.iter().map(|e| key(e.id)).collect();
            let rels = model
                .relationships
                .iter()
                .map(|r| (key(r.source.value), key(r.destination.value)))
                .collect();
            (elements, rels, model.views.len())
        }
        let mut h = H::with_file(SHOP);
        let (elements, rels, _) = shape(SHOP);
        let mut checked = 0;
        let mut removed_views = 0;
        let mut step = |h: &mut H, event: Event| {
            let undo = matches!(
                event,
                Event::Command(Command::Undo)
                    | Event::Input(Input::KeyDown {
                        key: Key::Z,
                        mods: Modifiers {
                            ctrl: true,
                            shift: false,
                            ..
                        },
                        ..
                    })
            );
            let before = shape(&h.text());
            let writes = h.writes.len();
            h.event(event);
            for w in &h.writes[writes..] {
                let after = shape(&w.text);
                assert_eq!(after.0, elements, "elements unchanged");
                assert_eq!(after.1, rels, "relationships unchanged");
                if after.2 < before.2 {
                    assert!(undo, "a view is removed only by undo");
                    removed_views += 1;
                }
                checked += 1;
            }
        };
        let buttons = [Button::Left, Button::Right, Button::Middle];
        let mods = [NONE, CTRL, SHIFT, ALT, CTRL_SHIFT];
        let keys = [
            Key::Enter,
            Key::F2,
            Key::Tab,
            Key::Escape,
            Key::Backspace,
            Key::ArrowLeft,
            Key::Space,
            Key::Z,
            Key::Y,
        ];
        for view in [declared("context"), declared("containers")] {
            step(&mut h, Event::Command(Command::ShowView(view.clone())));
            let boxes: Vec<String> = h
                .scene()
                .boxes
                .iter()
                .filter_map(|b| h.model().element(b.element).key.as_ref().map(|k| k.value.clone()))
                .collect();
            for key in &boxes {
                for &button in &buttons {
                    for &m in &mods {
                        step(&mut h, Event::Command(Command::ShowView(view.clone())));
                        let at = h.screen(key);
                        step(&mut h, Event::Input(Input::PointerDown { at, button, mods: m }));
                        step(&mut h, Event::Input(Input::PointerMove { at: pt(at.x + 30.0, at.y + 10.0) }));
                        step(&mut h, Event::Input(Input::PointerUp { at: pt(at.x + 30.0, at.y + 10.0), button }));
                    }
                }
                step(&mut h, Event::Command(Command::ShowView(view.clone())));
                let at = h.screen(key);
                step(&mut h, Event::Input(Input::PointerDown { at, button: Button::Left, mods: CTRL }));
                step(&mut h, Event::Input(Input::PointerUp { at, button: Button::Left }));
                for &k in &keys {
                    for &m in &mods {
                        step(&mut h, Event::Input(Input::KeyDown { key: k, mods: m, text_focus: false }));
                        step(&mut h, Event::Text("Renamed".to_owned()));
                        step(&mut h, Event::Input(Input::KeyUp { key: k }));
                    }
                }
                step(&mut h, Event::Input(Input::KeyDown { key: Key::Escape, mods: NONE, text_focus: false }));
            }
            let all: Vec<Selected> = boxes.iter().map(|k| el(k)).collect();
            h.s.selection = all.into_iter().collect();
            let commands = [
                Command::SetStyle {
                    target: StyleTarget::ThisBox,
                    property: StyleProperty::Background(Some(colour(1, 2, 3))),
                },
                Command::SetStyle {
                    target: StyleTarget::Tag("Element".to_owned()),
                    property: StyleProperty::Shape(Some(Shape::Box)),
                },
                Command::ResetStyle,
                Command::Align(Alignment::Left),
                Command::Align(Alignment::Middle),
                Command::Distribute(Axis::Vertical),
                Command::Undo,
                Command::Redo,
                Command::Undo,
                Command::PickFinding(0),
                Command::ExportView(ExportFormat::Svg),
                Command::ExportAll(ExportFormat::Svg),
                Command::Crumb(0),
                Command::Crumb(1),
                Command::Crumb(2),
            ];
            for c in commands {
                step(&mut h, Event::Command(c));
            }
            step(&mut h, Event::Input(Input::KeyUp { key: Key::Space }));
        }
        // A default view is added by a move and removed only by its undo.
        step(&mut h, Event::Command(Command::ShowView(declared("containers"))));
        step(&mut h, Event::Command(Command::ShowView(ViewRef::Default { scope: "web".to_owned(), kind: ViewKind::Component })));
        let at = h.screen("cart");
        step(&mut h, Event::Input(Input::PointerDown { at, button: Button::Left, mods: NONE }));
        step(&mut h, Event::Input(Input::PointerMove { at: pt(at.x + 60.0, at.y) }));
        step(&mut h, Event::Input(Input::PointerUp { at: pt(at.x + 60.0, at.y), button: Button::Left }));
        assert!(h.on_disk().model.view("web-component").is_some());
        while h.on_disk().model.view("web-component").is_some() {
            assert!(h.s.history.can_undo());
            step(&mut h, Event::Command(Command::Undo));
        }
        assert!(checked > 10, "the walk wrote ({checked})");
        assert!(removed_views >= 1);
    }

    /// EDIT-22: the watcher's echo of the app's own write (same hash) does
    /// not reload.
    #[test]
    fn edit_22_own_write_not_reloaded() {
        let mut h = H::with_file(SHOP);
        h.click("customer", Button::Left, CTRL);
        h.cmd(Command::SetStyle {
            target: StyleTarget::ThisBox,
            property: StyleProperty::Background(Some(colour(9, 9, 9))),
        });
        let text = h.text();
        let hash = files::hash(text.as_bytes());
        assert_eq!(h.s.doc.as_ref().map(|d| d.hash), Some(hash));
        let shown = h.s.shown.clone().map(|(_, b)| b);
        let effects = h.s.handle(Event::File(FileEvent::Changed { text, hash }));
        assert!(effects.is_empty(), "no reload, no rebuild: {effects:?}");
        assert_eq!(h.s.shown.clone().map(|(_, b)| b), shown);
    }

    /// EDIT-23: a `Changed` after a rename save reloads like any save.
    #[test]
    fn edit_23_rename_save_reloads() {
        let mut h = H::with_file(SHOP);
        h.click("customer", Button::Left, CTRL);
        let text = SHOP.replace("Buys things.", "Buys by rename.");
        let tmp = h.dir.join(".shop.dsl.swp");
        std::fs::write(&tmp, &text).expect("temp");
        std::fs::rename(&tmp, &h.path).expect("rename over");
        let effects = h.s.handle(Event::File(FileEvent::Changed {
            hash: files::hash(text.as_bytes()),
            text,
        }));
        assert!(effects
            .iter()
            .any(|e| matches!(e, Effect::Work(Request { job: Job::Build { shown: true, .. }, .. }))));
        h.run(effects);
        assert_eq!(h.s.details().and_then(|d| d.description).as_deref(), Some("Buys by rename."));
    }

    /// EDIT-24 (plan editor-06): a write refused by the on-disk check
    /// reloads and re-applies the action to the new text; an action whose
    /// target is gone is dropped with a status notice.
    #[test]
    fn edit_24_changed_on_disk_reapplies() {
        let mut h = H::with_file(&shop_at());
        h.hold_writes = true;
        h.drag_box("customer", 100.0, 0.0);
        assert_eq!(h.held_writes.len(), 1);
        let external = h.text().replace("Moves money.", "Moves lots of money.");
        std::fs::write(&h.path, &external).expect("external");
        h.release_writes();
        assert_eq!(h.writes.len(), 2, "refused, then re-applied");
        assert!(h.text().contains("Moves lots of money."));
        assert_eq!(h.at_lines("context")["customer"], (400, 200));
        assert!(h.s.pending.is_none());
        assert_eq!(h.s.history.undo.len(), 1);

        h.click("bank", Button::Left, CTRL);
        h.hold_writes = true;
        h.cmd(Command::SetStyle {
            target: StyleTarget::ThisBox,
            property: StyleProperty::Background(Some(colour(5, 5, 5))),
        });
        assert_eq!(h.held_writes.len(), 1);
        let without_bank: String = h.text().lines().filter(|l| !l.contains("bank")).flat_map(|l| [l, "\n"]).collect();
        std::fs::write(&h.path, &without_bank).expect("external");
        h.release_writes();
        assert_eq!(h.writes.len(), 3, "nothing re-written");
        assert_eq!(h.text(), without_bank);
        assert!(h.s.status().notice.is_some(), "the drop is reported");
        assert_eq!(h.s.history.undo.len(), 1);
        assert!(h.model().find("bank").is_none(), "the new text is shown");
    }

    /// EDIT-3, EDIT-24: an external save during a drag does not drop the
    /// drag; the release writes the move against the new text, by element
    /// key, whether or not the new text's build has landed.
    #[test]
    fn edit_24_external_save_mid_drag_keeps_the_drag() {
        for stall in [false, true] {
            let mut h = H::with_file(&shop_at());
            let from = h.screen("customer");
            h.press(from, Button::Left, NONE);
            h.move_to(pt(from.x + 50.0, from.y));
            h.move_to(pt(from.x + 100.0, from.y));
            h.stall = stall;
            let external = h.text().replace("Moves money.", "Moves lots of money.");
            h.external(&external);
            h.move_to(pt(from.x + 100.0, from.y));
            h.up(pt(from.x + 100.0, from.y), Button::Left);
            h.stall = false;
            assert_eq!(h.writes.len(), 1, "stall {stall}: the move is written");
            assert!(h.text().contains("Moves lots of money."), "stall {stall}");
            let at = h.at_lines("context");
            assert_eq!(at["customer"], (400, 200), "stall {stall}");
            assert_eq!(at["shop"], (300, 700), "stall {stall}");
            assert_eq!(at["bank"], (1000, 700), "stall {stall}");
            assert_eq!(h.s.history.undo.len(), 1, "stall {stall}");
        }
    }

    /// EDIT-24: when the external save removes the dragged element, the
    /// drag is dropped and the status bar says so.
    #[test]
    fn edit_24_external_save_removing_dragged_box_drops_drag() {
        for stall in [false, true] {
            let mut h = H::with_file(&shop_at());
            let from = h.screen("customer");
            h.press(from, Button::Left, NONE);
            h.move_to(pt(from.x + 50.0, from.y));
            h.move_to(pt(from.x + 100.0, from.y));
            h.stall = stall;
            let without: String = h.text().lines().filter(|l| !l.contains("customer")).flat_map(|l| [l, "\n"]).collect();
            h.external(&without);
            h.up(pt(from.x + 100.0, from.y), Button::Left);
            h.stall = false;
            assert!(h.writes.is_empty(), "stall {stall}: nothing written");
            assert_eq!(h.text(), without, "stall {stall}");
            assert!(h.s.status().notice.is_some(), "stall {stall}: the drop is reported");
            assert_eq!(h.s.gesture, Gesture::Idle, "stall {stall}");
        }
    }

    /// EDIT-25: 100 drag events with a stalled worker (no reply) each move
    /// the box at once and return without waiting; stale `Dragged`
    /// replies are dropped; after release the shown scene equals the
    /// build of the saved file.
    #[test]
    fn edit_25_drag_never_waits_on_worker() {
        let mut h = H::with_file(&shop_at());
        let (cx, cy) = h.centre_px("customer");
        let from = h.screen("customer");
        h.stall = true;
        h.press(from, Button::Left, NONE);
        let began = Instant::now();
        for i in 1..=100_i32 {
            #[expect(clippy::cast_precision_loss, reason = "small")]
            let at = pt(from.x + (3 * i) as f32, from.y);
            h.move_to(at);
            let want = (snap_px_to_grid(cx + 3 * i) * GRID_PX, cy);
            assert_eq!(h.centre_px("customer"), want, "moved at once, step {i}");
        }
        assert!(began.elapsed() < Duration::from_secs(5), "{:?}", began.elapsed());
        let drags: Vec<Request> = h.held_work.iter().filter(|r| matches!(r.job, Job::Drag { .. })).cloned().collect();
        assert!(drags.len() > 2);

        let stale = h.serve(drags[0].clone());
        assert!(stale.is_empty());
        assert!(h.s.dragged.is_none(), "a stale reply is dropped");
        let fresh = h.serve(drags[drags.len() - 1].clone());
        assert!(fresh.is_empty());
        assert!(h.s.dragged.is_some(), "the newest reply is shown");

        h.up(pt(from.x + 300.0, from.y), Button::Left);
        assert_eq!(h.writes.len(), 1, "written on release");
        h.stall = false;
        let build = h
            .held_work
            .iter()
            .rev()
            .find(|r| matches!(r.job, Job::Build { shown: true, .. }))
            .cloned()
            .expect("the saved file's build was requested");
        let effects = h.serve(build);
        h.run(effects);
        assert!(h.s.preview.is_none());
        let parsed = h.on_disk();
        let expected = build_view(&parsed.model, "context", &Keep::default(), &AtomicBool::new(false)).expect("builds");
        assert_eq!(h.scene(), &expected.scene);
        assert_eq!(h.centre_px("customer"), (cx + 300, cy));
    }

    /// EDIT-26: the first move in a view writes an `at` line for every box
    /// as one step; the next move changes only the moved box's line.
    #[test]
    fn edit_26_first_write_writes_every_box() {
        let mut h = H::with_file(SHOP);
        let shop = h.centre_px("shop");
        let bank = h.centre_px("bank");
        let (cx, cy) = h.centre_px("customer");
        h.drag_box("customer", 100.0, 0.0);
        let at = h.at_lines("context");
        assert_eq!(at.len(), 3);
        assert_eq!((at["customer"], at["shop"], at["bank"]), ((cx + 100, cy), shop, bank));
        assert_eq!(h.s.history.undo.len(), 1, "one step");
        assert!(h.at_lines("containers").is_empty(), "other views untouched");

        let before = h.text();
        h.drag_box("bank", 0.0, 100.0);
        let (added, removed) = line_diff(&before, &h.text());
        assert_eq!(added, [format!("            at bank {} {}", bank.0, bank.1 + 100)]);
        assert_eq!(removed, [format!("            at bank {} {}", bank.0, bank.1)]);
        assert_eq!(h.s.history.undo.len(), 2);
    }

    /// EDIT-26 with EDIT-25's last sentence: in a view with some
    /// hand-written `at` lines, the first write adds one for every box
    /// still without, leaves the written ones alone, and is one step.
    #[test]
    fn edit_26_first_write_places_boxes_left_unplaced() {
        let original = shop_with(&[("customer", 300, 200)]);
        let mut h = H::with_file(&original);
        let (sx, sy) = h.centre_px("shop");
        let bank = h.centre_px("bank");
        h.drag_box("shop", 100.0, 0.0);
        let at = h.at_lines("context");
        assert_eq!(at.len(), 3, "every box has an `at` line: {at:?}");
        assert_eq!(
            (at["customer"], at["shop"], at["bank"]),
            ((300, 200), (sx + 100, sy), bank)
        );
        assert_eq!(h.s.history.undo.len(), 1, "one step");
        let parsed = h.on_disk();
        let expected = build_view(&parsed.model, "context", &Keep::default(), &AtomicBool::new(false)).expect("builds");
        assert_eq!(h.scene(), &expected.scene, "the file alone gives the shown scene");

        h.cmd(Command::Undo);
        assert_eq!(h.text(), original, "undo removes the lines it added");
    }

    /// EDIT-26 through undo (EDIT-19, EDIT-24): a recorded move re-applied
    /// to a view where a box lost its `at` line is a first write again,
    /// so that box gets its line back in the same step.
    #[test]
    fn edit_26_undo_rebuilds_when_a_box_lost_its_line() {
        let mut h = H::with_file(&shop_at());
        h.drag_box("customer", 100.0, 0.0);
        let without_bank: String = h
            .text()
            .lines()
            .filter(|l| !l.contains("at bank"))
            .flat_map(|l| [l, "\n"])
            .collect();
        h.external(&without_bank);
        assert!(!h.at_lines("context").contains_key("bank"));
        h.cmd(Command::Undo);
        let at = h.at_lines("context");
        assert_eq!(at.len(), 3, "every box has an `at` line: {at:?}");
        assert_eq!((at["customer"], at["shop"]), ((300, 200), (300, 700)));
    }

    /// EDIT-27: the file disappearing keeps the last diagram read-only
    /// until it reappears; a failed write reverts the canvas and shows the
    /// error; a vanished open view falls back to the first declared one.
    #[test]
    fn edit_27_file_removed_keeps_last_read_only() {
        let mut h = H::with_file(SHOP);
        let scene = h.scene().clone();
        std::fs::remove_file(&h.path).expect("remove");
        h.event(Event::File(FileEvent::Removed));
        assert_eq!(h.s.status().problem, Some(Problem::Missing));
        assert!(h.s.read_only());
        assert_eq!(h.scene(), &scene);
        h.drag_box("customer", 100.0, 0.0);
        assert!(h.writes.is_empty());

        h.external(SHOP);
        assert_eq!(h.s.status().problem, None);
        assert!(!h.s.read_only());

        let (cx, cy) = h.centre_px("customer");
        h.hold_writes = true;
        h.drag_box("customer", 100.0, 0.0);
        assert_eq!(h.centre_px("customer"), (cx + 100, cy), "preview until written");
        h.hold_writes = false;
        h.held_writes.clear();
        h.event(Event::File(FileEvent::WriteFailed {
            error: "cannot write shop.dsl: permission denied".to_owned(),
        }));
        assert_eq!(
            h.s.status().problem,
            Some(Problem::Io("cannot write shop.dsl: permission denied".to_owned()))
        );
        assert_eq!(h.centre_px("customer"), (cx, cy), "the canvas shows the file again");
        assert!(h.s.pending.is_none());
        assert!(!h.s.history.can_undo());

        h.cmd(Command::ShowView(declared("containers")));
        h.external(&SHOP.replace(CONTAINERS, ""));
        assert_eq!(h.s.view, Some(declared("context")));
        assert_eq!(h.scene().key, "context");
        assert_eq!(h.s.status().problem, None);
    }

    /// EDIT-28 (plan Review Focus): a press and release moving under 4 px
    /// is a click (enters or selects), 4 px or more is a drag; Shift-click
    /// toggles and never enters; empty-canvas click and Escape deselect;
    /// Space+drag pans.
    #[test]
    fn edit_28_click_under_4px_enters_not_drags() {
        let mut h = H::with_file(SHOP);
        let p = h.screen("customer");
        h.press(p, Button::Left, NONE);
        h.move_to(pt(p.x + 2.0, p.y + 1.0));
        h.up(pt(p.x + 2.0, p.y + 1.0), Button::Left);
        assert_eq!(h.selection(), vec![el("customer")], "still a click");
        assert!(h.writes.is_empty());

        let p = h.screen("shop");
        h.press(p, Button::Left, NONE);
        h.move_to(pt(p.x + 3.0, p.y));
        h.up(pt(p.x + 3.0, p.y), Button::Left);
        assert_eq!(h.s.view, Some(declared("containers")), "a click enters");
        h.cmd(Command::ShowView(declared("context")));

        h.drag_box("shop", 5.0, 0.0);
        assert_eq!(h.s.view, Some(declared("context")), "a drag never enters");
        assert_eq!(h.writes.len(), 1, "4 px or more is a drag");

        h.click("customer", Button::Left, NONE);
        h.click("shop", Button::Left, SHIFT);
        assert_eq!(h.s.view, Some(declared("context")));
        assert_eq!(h.selection(), vec![el("customer"), el("shop")]);
        h.click("shop", Button::Left, SHIFT);
        assert_eq!(h.selection(), vec![el("customer")]);

        let empty = h.empty_point();
        h.click_at(empty, Button::Left, NONE);
        assert!(h.selection().is_empty(), "empty canvas deselects");
        h.click("customer", Button::Left, NONE);
        h.key(Key::Escape, NONE);
        assert!(h.selection().is_empty(), "Escape deselects");

        let pan = h.s.camera.pan;
        h.key(Key::Space, NONE);
        let p = h.screen("customer");
        h.press(p, Button::Left, NONE);
        h.move_to(pt(p.x + 40.0, p.y));
        h.up(pt(p.x + 40.0, p.y), Button::Left);
        h.input(Input::KeyUp { key: Key::Space });
        assert!((h.s.camera.pan.x - (pan.x - 40.0)).abs() < 1e-3, "Space+drag pans");
        assert_eq!(h.writes.len(), 1, "and moves nothing");
        assert!(!h.s.space);
    }

    /// EDIT-29: on `example/`, a `Changed` event to the scene handed to
    /// the canvas takes under 200 ms (release perf oracle).
    #[test]
    #[ignore = "perf: release perf oracle only"]
    fn perf_edit_29_reload_under_200ms() {
        let example = include_str!("../../../tests/data/example.dsl");
        let mut h = H::with_file(example);
        let before = h.scene().clone();
        let mut worst = Duration::ZERO;
        for i in 0..5 {
            let text = format!("{example}\n// save {i}\n");
            std::fs::write(&h.path, &text).expect("save");
            let began = Instant::now();
            let effects = h.s.handle(Event::File(FileEvent::Changed {
                hash: files::hash(text.as_bytes()),
                text,
            }));
            let shown = effects.into_iter().find_map(|e| match e {
                Effect::Work(r @ Request { job: Job::Build { shown: true, .. }, .. }) => Some(r),
                _ => None,
            });
            let reply = worker::serve(shown.expect("a build"), &AtomicBool::new(false)).expect("reply");
            let _ = h.s.handle(Event::Worker(reply));
            let scene = h.s.scene().expect("scene");
            worst = worst.max(began.elapsed());
            assert_eq!(scene.key, before.key);
        }
        assert!(worst < Duration::from_millis(200), "worst reload {worst:?}");
    }

    /// The narrow tiled window of the EDIT-30 report, canvas px.
    const CANVAS: ScreenPoint = ScreenPoint {
        x: 848.0,
        y: 1300.0,
    };

    /// The camera "Fit view" gives for the scene on screen.
    fn fitted(h: &H) -> Camera {
        Camera::fit(h.scene().bounds, CANVAS)
    }

    /// EDIT-30: the first show of a view fits the whole scene, whether
    /// the canvas size is known before the build lands or after.
    #[test]
    fn edit_30_first_show_fits_scene() {
        let mut h = H::with_file(SHOP);
        assert_eq!(
            h.s.camera,
            Camera::default(),
            "no canvas size yet: nothing to fit"
        );
        h.s.set_canvas_size(CANVAS);
        assert_eq!(h.s.camera, fitted(&h));
        assert!(h.s.camera.zoom <= 1.0);

        let mut h = H::empty();
        std::fs::write(&h.path, SHOP).expect("write fixture");
        h.s.set_canvas_size(CANVAS);
        let path = h.path.clone();
        h.cmd(Command::Open(path));
        assert_eq!(
            h.s.camera,
            fitted(&h),
            "size known first, fit when the build lands"
        );
    }

    /// EDIT-30: the fit covers the title and the key, not only the boxes.
    #[test]
    fn edit_30_fit_covers_title_and_key() {
        // A canvas smaller than the scene, so only a fit shows it all.
        let small = pt(360.0, 260.0);
        let mut h = H::with_file(SHOP);
        h.s.set_canvas_size(small);
        let scene = h.scene().clone();
        let inside = |p: Point| {
            let q = h.s.camera.to_screen(p);
            q.x >= 19.9 && q.y >= 19.9 && q.x <= small.x - 19.9 && q.y <= small.y - 19.9
        };
        let title = scene.title.as_ref().expect("a title");
        assert!(inside(title.at), "title on screen");
        assert!(
            inside(scene.legend.rect.min) && inside(scene.legend.rect.max),
            "key on screen"
        );
    }

    /// EDIT-30, EDIT-1: a reload keeps zoom and pan; it does not fit
    /// again.
    #[test]
    fn edit_30_reload_keeps_zoom_and_pan() {
        let mut h = H::with_file(SHOP);
        h.s.set_canvas_size(CANVAS);
        h.input(Input::Wheel {
            at: pt(100.0, 100.0),
            notches: 2.0,
        });
        let camera = h.s.camera;
        let text = format!("{SHOP}\n// saved elsewhere\n");
        std::fs::write(&h.path, &text).expect("save");
        h.event(Event::File(FileEvent::Changed {
            hash: files::hash(text.as_bytes()),
            text,
        }));
        assert_eq!(h.scene().key, "context");
        assert_eq!(h.s.camera, camera);
        h.s.set_canvas_size(CANVAS);
        assert_eq!(h.s.camera, camera, "a resize does not fit again");
    }

    /// EDIT-30: entering a view by a click fits it the first time; going
    /// back up restores the camera the view had; entering it again
    /// restores its own camera too.
    #[test]
    fn edit_30_return_to_view_restores_its_camera() {
        let mut h = H::with_file(SHOP);
        h.s.set_canvas_size(CANVAS);
        h.input(Input::Wheel {
            at: pt(100.0, 100.0),
            notches: -1.0,
        });
        h.s.camera.pan_by(pt(7.0, 3.0));
        let context = h.s.camera;

        h.click("shop", Button::Left, NONE);
        assert_eq!(h.s.view, Some(declared("containers")));
        assert_eq!(h.s.camera, fitted(&h), "first show of containers fits");
        h.s.camera.pan_by(pt(-12.0, 5.0));
        let containers = h.s.camera;

        h.key(Key::Backspace, NONE);
        assert_eq!(h.s.view, Some(declared("context")));
        assert_eq!(h.s.camera, context, "back up: the context camera as it was");

        h.cmd(Command::ShowView(declared("containers")));
        assert_eq!(h.s.camera, containers);
    }

    /// EDIT-30: a tiling window manager resizes the window right after
    /// the first frame; a fitted camera the user has not moved follows
    /// the canvas size, a moved one stays.
    #[test]
    fn edit_30_untouched_fit_follows_canvas_resize() {
        let mut h = H::with_file(SHOP);
        h.s.set_canvas_size(pt(1280.0, 720.0));
        let wide = h.s.camera;
        h.s.set_canvas_size(CANVAS);
        assert_ne!(h.s.camera, wide);
        assert_eq!(h.s.camera, fitted(&h), "still fitted after the tiling resize");

        h.s.camera.pan_by(pt(5.0, 5.0));
        let moved = h.s.camera;
        h.s.set_canvas_size(pt(1280.0, 720.0));
        assert_eq!(h.s.camera, moved, "a moved camera is the user's");
    }

    /// EDIT-30: "Fit view" fits the scene on screen on demand.
    #[test]
    fn edit_30_fit_view_on_demand() {
        let mut h = H::with_file(SHOP);
        h.s.set_canvas_size(CANVAS);
        h.s.camera.pan_by(pt(300.0, -200.0));
        h.input(Input::Wheel {
            at: pt(10.0, 10.0),
            notches: 4.0,
        });
        assert_ne!(h.s.camera, fitted(&h));
        h.s.fit_view();
        assert_eq!(h.s.camera, fitted(&h));
    }
}

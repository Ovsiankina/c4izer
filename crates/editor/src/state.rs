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
// EDIT-16: the workspace crumb, and Backspace on a system context view
// (Alt+Left is Back since P4-114), open the first declared system
// context view, else the first declared view (nothing when the file
// declares no view).
// EDIT-15: a title or subtitle edit is also a write that adds the default
// view, like a move, resize or style; a default view has no title line
// and its drawn title is the scope's name.
// EDIT-15: a box is enterable when its element has at least one child in
// the model (opening the default view when no view is scoped to it), or
// a container view scoped to it (for a system) or a component view
// scoped to it (for a container); a system context view does not count
// (`enterable`, `view_into`).

use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use c4rizer_core::edit::{
    Edit, ElementField, RelationshipField, RelationshipRef, StyleProperty, StyleTarget, TextTarget,
    ViewField, apply,
};
use c4rizer_core::geom::{GRID_PX, Point, Rect, snap_px_to_grid};
use c4rizer_core::lint::Finding;
use c4rizer_core::model::{
    Colour, ElementId, ElementKind, Mode, Model, ModelError, PaperSize, Parsed, RelationshipId, Routing, Shape,
    ViewKind, parse_with,
};
use c4rizer_core::pipeline::{ViewBuild, default_view, keep_across_reparse};
use c4rizer_core::place::Keep;
use c4rizer_core::scene::{ItemKey, ModelId, Scene, texts_fit};
use c4rizer_core::view::{BoundaryOf, BoxId, PlanBox};

use crate::camera::{Camera, Fit, ScenePoint, ScreenPoint, ZOOM_STEP};
use crate::files::{self, ContentHash, FileEvent, FileMemory, UiState, WriteRequest, export_dir, export_name};
use crate::history::{History, Step};
use crate::worker::{ExportFormat, ExportTarget, Generation, Job, Reply, Request, ViewRef};

mod anchors;
mod steps;
mod vertices;
pub use anchors::SnapMark;
pub use steps::StepDetails;

/// Pointer travel below which a press and release is a click, in screen
/// px (EDIT-28).
pub const CLICK_SLOP_PX: f32 = 4.0;

/// Hit tolerance for lines and boundary outlines, in scene px
/// (`Scene::hit`) (EDIT-28).
pub const HIT_TOLERANCE_PX: i32 = 4;

/// Most views kept in the session's history of views shown (P2-714).
const VISITED_MAX: usize = 100;

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
    /// With Alt: back (P4-114); alone: next view (P2-713).
    ArrowLeft,
    /// Held: drag pans (EDIT-28).
    Space,
    /// With Ctrl: undo; with Ctrl+Shift: redo (EDIT-19).
    Z,
    /// With Ctrl: redo (EDIT-19).
    Y,
    /// Previous view (P2-713); with a hovered line, nothing.
    ArrowUp,
    /// Previous view (P2-713); with Alt: forward (P4-115).
    ArrowRight,
    /// Next view (P2-713); with a hovered line, nothing.
    ArrowDown,
    /// Zoom in one `ZOOM_STEP` at the canvas centre (P2-711); `+` and `=`.
    Plus,
    /// Zoom out one `ZOOM_STEP` at the canvas centre (P2-711).
    Minus,
    /// Align bottom (P2-707), else back to the last viewed view (P2-714).
    B,
    /// Align centre (P2-707), else fit content (P2-705).
    C,
    /// Toggle descriptions (P2-721).
    D,
    /// Open the tag filter (P2-725).
    G,
    /// Distribute horizontally (P2-707), else fit height (P2-705).
    H,
    /// Show or hide the key (P2-728).
    I,
    /// Toggle jumps of the hovered line (P2-710).
    J,
    /// Align left (P2-707), else open the navigator (P2-729).
    L,
    /// Align middle (P2-707), else toggle metadata (P2-722).
    M,
    /// Show or hide the view list (P2-727).
    N,
    /// Open the perspectives list (P2-726).
    P,
    /// Align right (P2-707), or cycle the hovered line's routing (P2-709).
    R,
    /// Align top (P2-707), else toggle tooltips (P2-723).
    T,
    /// Distribute vertically (P2-707).
    V,
    /// Fit width (P2-705).
    W,
    /// Fullscreen (P2-706).
    F11,
    /// Alt went down: makes a shown tooltip sticky (P2-724). The shell
    /// reports it when the Alt modifier turns on.
    Alt,
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
    /// Alt: Alt+Left goes back, Alt+Right forward (P4-114, P4-115).
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
    /// The canvas widget's size, screen px, reported each frame: the
    /// anchor of the `+`/`-` zoom and of the fits (P2-705, P2-711).
    CanvasSize {
        /// Width and height.
        size: ScreenPoint,
    },
    /// A double click (the second release of two quick clicks) over the
    /// canvas: with Alt, opens the URL of the item (P2-719, P2-720).
    DoubleClick {
        /// Where.
        at: ScreenPoint,
        /// Modifiers held.
        mods: Modifiers,
    },
}

/// One step of the canvas size list (P2-701).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum CanvasStep {
    /// The previous size of the list.
    Decrease,
    /// The smallest size that holds the scene.
    Auto,
    /// The next size of the list.
    Increase,
}

/// One per-view line setting (P2-708..P2-710).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum LineChange {
    /// Label position, 0..100, `None` to drop it.
    Position(Option<u8>),
    /// Routing, `None` to drop it.
    Routing(Option<Routing>),
    /// Jumps on or off, `None` to drop it.
    Jump(Option<bool>),
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
    /// Pick an entry of the context menu (editor-ux-02).
    ContextAction(MenuAction),
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
    /// "Auto-place view": every box placed afresh and written, one undo
    /// step (P2-700).
    AutoPlace,
    /// Decrease, auto or increase the canvas size, one undo step; the
    /// drawing stays centred unless `keep_positions` (Alt) (P2-701,
    /// P2-702).
    Canvas {
        /// Which step.
        step: CanvasStep,
        /// Alt held: every box keeps its position.
        keep_positions: bool,
    },
    /// "Bring boxes onto the canvas" (the magnet), one undo step (P2-703).
    BringOntoCanvas,
    /// "Centre": the selection, else every box, centred on the canvas,
    /// one undo step (P2-704).
    Centre,
    /// Fit the camera to the scene's width, height or both (P2-705).
    Fit(Fit),
    /// Toggle fullscreen (P2-706).
    Fullscreen,
    /// Open the previous view of the view list (P2-713).
    PreviousView,
    /// Open the next view of the view list (P2-713).
    NextView,
    /// Open the last viewed view (P2-714).
    Back,
    /// Open quick navigation (P2-715).
    QuickNavigation,
    /// Toggle descriptions (P2-721).
    ToggleDescriptions,
    /// Toggle metadata (P2-722).
    ToggleMetadata,
    /// Toggle tooltips (P2-723).
    ToggleTooltips,
    /// Open or close the tag filter (P2-725).
    TagFilterPanel,
    /// The tags chosen in the tag filter, empty to clear it (P2-725).
    TagFilter(Vec<String>),
    /// Open or close the perspectives list (P2-726).
    PerspectivePanel,
    /// The chosen perspective, `None` for the normal view (P2-726).
    Perspective(Option<String>),
    /// Show or hide the view list panel (P2-727).
    ToggleViewList,
    /// Show or hide the drawn key (P2-728).
    ToggleKey,
    /// Open or close the diagram navigator (P2-729).
    Navigator,
    /// "Remove all vertices": delete every `via` line of the open view,
    /// one undo step (P3-313).
    // APPROVER: new pub item
    RemoveAllVertices,
    /// Select step `n` (index into `ViewSpec::steps`) of the open dynamic
    /// view, from the Steps list (P3-219).
    // APPROVER: new pub item
    SelectStep(u32),
    /// Move step `step` of the open dynamic view one place up or down
    /// within its block (P3-220, P3-221).
    // APPROVER: new pub item
    MoveStep {
        /// Index into `ViewSpec::steps`.
        step: u32,
        /// Up (towards the first step) or down.
        up: bool,
    },
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
    /// Enter (`true`) or leave fullscreen (P2-706).
    Fullscreen(bool),
    /// Open this `http`/`https` URL in the system browser (P2-719,
    /// P2-720).
    OpenUrl(String),
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
    /// A step line of the open dynamic view, by its index into
    /// `ViewSpec::steps` (P3-218, P3-219).
    // APPROVER: new pub item
    Step(u32),
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
    /// Left drag on a vertex handle of the selected line (P3-311).
    // APPROVER: new pub item
    DraggingVertex {
        /// The line, by its plan edge in the scene shown.
        edge: c4rizer_core::view::EdgeId,
        /// Index of the vertex in the line's vertices.
        index: usize,
        /// Every vertex of the line, grid units, the dragged one at its
        /// current snapped position (the last sent to the worker).
        vertices: Vec<Point>,
    },
    /// Left drag on an end handle of the selected line (P4-214).
    // APPROVER: new pub item
    DraggingEnd {
        /// The line, by its plan edge in the scene shown.
        edge: c4rizer_core::view::EdgeId,
        /// True for the destination end, false for the source end.
        destination: bool,
        /// The snap point the end sits on, `None` away from every one.
        snapped: Option<c4rizer_core::anchor::Anchor>,
        /// The pointer, screen px.
        at: ScreenPoint,
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
    /// Set the canvas size, moving boxes to keep the drawing centred, one
    /// batch (P2-701, P2-702).
    SetCanvasSize {
        /// The view.
        view: ViewRef,
        /// The size, `None` for auto.
        size: Option<PaperSize>,
        /// Boxes moved, canonical key to centre px.
        boxes: BTreeMap<String, (i32, i32)>,
        /// Every box of `view` at its centre, px (EDIT-26 first write).
        layout: BTreeMap<String, (i32, i32)>,
    },
    /// Set one per-view line setting (P2-708..P2-710).
    SetLine {
        /// The view.
        view: ViewRef,
        /// The line's winning relationship.
        relationship: RelationshipRef,
        /// The setting.
        change: LineChange,
    },
    /// Swap two neighbouring steps of a dynamic view (P3-220).
    // APPROVER: new pub item
    SwapSteps {
        /// The view.
        view: ViewRef,
        /// Index into `ViewSpec::steps` of the upper step.
        first: u32,
    },
    /// Set, move or remove line vertices, with the boxes moved by the
    /// same gesture, one batch (P3-310..P3-315).
    // APPROVER: new pub item
    Vertices {
        /// The view.
        view: ViewRef,
        /// Boxes moved by the gesture, canonical key to centre px; empty
        /// for a vertex-only edit.
        boxes: BTreeMap<String, (i32, i32)>,
        /// The `via` lines to write, each one line's whole vertex list.
        vias: Vec<ViaChange>,
        /// Every box of `view` at its centre, px (EDIT-26 first write).
        layout: BTreeMap<String, (i32, i32)>,
    },
    /// Pin or release the ends of one line (P4-215..P4-218).
    // APPROVER: new pub item
    Anchor {
        /// The view.
        view: ViewRef,
        /// The line's whole `anchor` line.
        change: AnchorChange,
        /// Every box of `view` at its centre, px (EDIT-26 first write).
        layout: BTreeMap<String, (i32, i32)>,
    },
    /// Apply an undo step (EDIT-19).
    Undo(Step),
    /// Apply a redo step (EDIT-19).
    Redo(Step),
}

/// One line's pinned ends, as `edit::Edit::SetAnchor` names them
/// (P4-215..P4-218).
// APPROVER: new pub item
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AnchorChange {
    /// The line's source, canonical element key.
    pub source: String,
    /// The line's destination, canonical element key.
    pub destination: String,
    /// The dynamic step number, `None` in other views.
    pub order: Option<String>,
    /// The source end's point, `None` for the router's.
    pub from: Option<c4rizer_core::anchor::Anchor>,
    /// The destination end's point, `None` for the router's.
    pub to: Option<c4rizer_core::anchor::Anchor>,
}

/// One line's new vertex list, as `edit::Edit::SetVia` names it
/// (P3-310..P3-315).
// APPROVER: new pub item
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ViaChange {
    /// The line's source, canonical element key.
    pub source: String,
    /// The line's destination, canonical element key.
    pub destination: String,
    /// The dynamic step number, `None` in other views.
    pub order: Option<String>,
    /// The vertices, px, on the 10 px grid; `None` removes the `via`
    /// line.
    pub points: Option<Vec<(i32, i32)>>,
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
#[expect(clippy::struct_excessive_bools, reason = "independent view-only flags (EDIT-11, P2-721, P2-722, P2-728)")]
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
    /// Descriptions off (P2-721): no description text is painted; every
    /// rect, line and label keeps its geometry.
    pub hide_descriptions: bool,
    /// Metadata off (P2-722): no type line and no label technology is
    /// painted; geometry kept.
    pub hide_metadata: bool,
    /// The key is hidden (P2-728).
    pub hide_key: bool,
    /// Items faded by the tag filter or the chosen perspective (P2-725,
    /// P2-726): marks, painted at reduced opacity.
    pub faded: Vec<ItemKey>,
    /// The tooltip overlay (P2-723, P2-724).
    pub tooltip: Option<Tooltip>,
}

/// The tooltip overlay (P2-723): the hovered item's details, at the
/// pointer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Tooltip {
    /// The item it describes.
    pub item: ItemKey,
    /// Its details (EDIT-10 fields, URL, properties).
    pub details: Details,
    /// Stays after the pointer leaves the item (P2-724).
    pub sticky: bool,
    /// The item's `url`, if any.
    // APPROVER: new pub item beyond A.2 (tooltip content, B
    // editor-breadth 9: URL and properties beyond the EDIT-10 details).
    pub url: Option<String>,
    /// The item's properties, name and value, in file order.
    // APPROVER: new pub item beyond A.2 (as `url`).
    pub properties: Vec<(String, String)>,
}

/// The view-only viewing aids (P2-721..P2-729): none writes the file
/// (P2-730), all survive a reload (P2-731).
#[derive(Debug, Clone, PartialEq, Eq)]
#[expect(clippy::struct_excessive_bools, reason = "independent view-only toggles (P2-706, P2-721..P2-728)")]
pub struct ViewAids {
    /// Descriptions shown (P2-721).
    pub descriptions: bool,
    /// Metadata shown (P2-722).
    pub metadata: bool,
    /// Tooltips on; `None` until toggled: the view's or view set's
    /// `structurizr.tooltips` decides (P2-723).
    pub tooltips: Option<bool>,
    /// Tags chosen in the tag filter, empty for none (P2-725).
    pub tags: Vec<String>,
    /// The chosen perspective (P2-726).
    pub perspective: Option<String>,
    /// The key drawn (P2-728).
    pub key: bool,
    /// The view list panel shown (P2-727).
    pub view_list: bool,
    /// Fullscreen (P2-706).
    pub fullscreen: bool,
}

impl Default for ViewAids {
    fn default() -> Self {
        ViewAids {
            descriptions: true,
            metadata: true,
            tooltips: None,
            tags: Vec::new(),
            perspective: None,
            key: true,
            view_list: true,
            fullscreen: false,
        }
    }
}

/// A list open over the canvas; while one is open it takes the keys
/// (P2-712).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Overlay {
    /// Quick navigation (P2-715): the typed text and the highlighted
    /// entry of `State::quick_nav_entries`.
    QuickNavigation {
        /// The typed text.
        query: String,
        /// Index of the highlighted entry.
        highlighted: usize,
    },
    /// The tag filter (P2-725).
    TagFilter,
    /// The perspectives list (P2-726).
    Perspectives,
    /// The diagram navigator (P2-729).
    Navigator,
    /// The right-click menu of a box, line or label (editor-ux-02):
    /// its entries; `State::menu_at` is where it opens.
    ContextMenu {
        /// The entries, in the order shown.
        entries: Vec<MenuEntry>,
    },
}

/// What a context menu entry does (editor-ux-02).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MenuAction {
    /// Open a deeper view of the box's element (was EDIT-15's double
    /// click).
    Show(ViewRef),
    /// Open the `url` of the item named `name` (was P2-719, P2-720's
    /// Alt+double-click), through the same http/https check.
    OpenUrl {
        /// The item's name, for the status notice.
        name: String,
        /// The URL as written.
        url: String,
    },
    /// Remove the pin of one end of a line (P4-217).
    // APPROVER: new pub item
    ReleaseAnchor {
        /// The line, by its plan edge in the scene shown.
        edge: c4rizer_core::view::EdgeId,
        /// True for the destination end.
        destination: bool,
    },
}

/// One entry of the context menu (editor-ux-02).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MenuEntry {
    /// The text shown.
    pub label: String,
    /// What picking it does.
    pub action: MenuAction,
}

/// Where a navigator entry sits relative to the open view (P2-729).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum NavLevel {
    /// Above: views of a higher C4 level that contain the open view's
    /// scope.
    Above,
    /// Next to: other views of the same level and kind.
    NextTo,
    /// Below: the views the open view's boxes enter.
    Below,
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
    /// For a dynamic step line: its number, ends and relationship
    /// (P3-218); `None` otherwise.
    // APPROVER: new pub item
    pub step: Option<StepDetails>,
    /// A deployment node's `instances` value as written (P3-131).
    pub instances: Option<String>,
    /// The deployment environment's name of a deployment element (P3-131).
    pub environment: Option<String>,
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

/// The shapes the style panel offers (EDIT-17, P2-417): all 19, in the
/// notation page's order, shown in the reference spelling
/// (`Shape::keyword`).
pub(crate) const PANEL_SHAPES: [Shape; 19] = [
    Shape::Box,
    Shape::RoundedBox,
    Shape::Circle,
    Shape::Ellipse,
    Shape::Hexagon,
    Shape::Diamond,
    Shape::Cylinder,
    Shape::Bucket,
    Shape::Pipe,
    Shape::Person,
    Shape::Robot,
    Shape::Folder,
    Shape::WebBrowser,
    Shape::Window,
    Shape::Terminal,
    Shape::Shell,
    Shape::MobileDevicePortrait,
    Shape::MobileDeviceLandscape,
    Shape::Component,
];

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
    /// Light or dark: the mode every build and export uses (P2-433).
    mode: Mode,
    // DECISION: the dsl-breadth bookkeeping (included files, P2-136,
    // P2-139, P2-141) is private fields and `pub(crate)` methods, like the
    // EDIT-30 fields above, so the step-4 contract of `State`, `Doc`,
    // `Event`, `Effect` and `WriteRequest` stays as it is.
    /// The root file's text as last read or written: what an included
    /// file's change re-parses with (P2-141).
    root_text: Option<String>,
    /// Every file the last parse read besides the root, by the path the
    /// parse read it at, with the hash of its bytes as read or last
    /// written by the app: the per-file own-write hash (EDIT-22, P2-141).
    included: BTreeMap<PathBuf, ContentHash>,
    /// The included file holding `Doc::parse_error`, `None` when the
    /// error is in the root file (P2-136).
    error_file: Option<PathBuf>,
    /// The file of the write in flight (EDIT-22), so its result event is
    /// told apart: the root file's path as opened, or an included file's.
    in_flight: Option<PathBuf>,
    /// The writes of the pending batch still to go after `in_flight`, in
    /// order: included files first, the root file last (P2-139).
    queued: VecDeque<WriteRequest>,
    /// The viewing aids (P2-721..P2-729).
    pub aids: ViewAids,
    /// The list open over the canvas (P2-715, P2-725, P2-726, P2-729).
    pub overlay: Option<Overlay>,
    /// Where the context menu opens, canvas px (editor-ux-02).
    pub menu_at: ScreenPoint,
    /// The views shown this session, oldest first, the open one last
    /// (P2-714).
    visited: Vec<ViewRef>,
    /// The views Back left, the next Forward target last (P4-115).
    ahead: Vec<ViewRef>,
    /// The selection in the order it was made (P2-707: align to the first
    /// selected box).
    selection_order: Vec<Selected>,
    /// The sticky tooltip's item (P2-724).
    sticky: Option<ItemKey>,
    /// Space went down and no drag happened since (P2-715).
    space_tap: bool,
    /// The last pointer position over the canvas, screen px.
    pointer: Option<ScreenPoint>,
    /// The navigator's search text (P2-729), cleared when it opens.
    nav_query: String,
    /// Auto-place wrote; fit the camera when its build is shown (P2-700).
    after_place: AfterPlace,
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
            mode: Mode::Light,
            root_text: None,
            included: BTreeMap::new(),
            error_file: None,
            in_flight: None,
            queued: VecDeque::new(),
            aids: ViewAids::default(),
            overlay: None,
            menu_at: ScreenPoint::default(),
            visited: Vec::new(),
            ahead: Vec::new(),
            selection_order: Vec::new(),
            sticky: None,
            space_tap: false,
            pointer: None,
            nav_query: String::new(),
            after_place: AfterPlace::Nothing,
        }
    }

    /// The views quick navigation offers for its typed text, in view-list
    /// order (P2-715).
    // TODO(srs): P2-715, the matching rule. DECISION: reading taken (B
    // editor-breadth 9): a case-insensitive substring of the key or the
    // drawn title.
    #[must_use]
    pub fn quick_nav_entries(&self) -> Vec<ViewEntry> {
        let query = match &self.overlay {
            Some(Overlay::QuickNavigation { query, .. }) => query.to_lowercase(),
            _ => String::new(),
        };
        self.views()
            .into_iter()
            .filter(|e| e.view.key().to_lowercase().contains(&query) || e.label.to_lowercase().contains(&query))
            .collect()
    }

    /// True when tooltips are on (P2-723): toggled this session, else the
    /// open view's `structurizr.tooltips`, else the view set's, else off.
    // TODO(srs): P2-723, the default. DECISION: reading taken (B
    // editor-breadth 9; Structurizr 6.2.4 diagrams.jsp disables the
    // tooltip at start): off.
    fn tooltips_on(&self) -> bool {
        if let Some(on) = self.aids.tooltips {
            return on;
        }
        let Some(model) = self.model() else { return false };
        let read = |props: &[c4rizer_core::model::Property]| {
            props
                .iter()
                .rev()
                .find(|p| p.name.value == "structurizr.tooltips")
                .map(|p| p.value.value.eq_ignore_ascii_case("true"))
        };
        let view = self.view.as_ref().and_then(|v| model.view(&v.key()));
        view.and_then(|v| read(&v.properties))
            .or_else(|| read(&model.views_properties))
            .unwrap_or(false)
    }

    /// The user-defined tags of the open view's boxes and lines, sorted:
    /// the tag filter's choices (P2-725).
    // P2-725 as written: the built-ins are the six the sentence names, so
    // `External` is listed (as Structurizr 6.2.4 lists it; B
    // editor-breadth 9 hid it). TODO(srs): P2-725, the full built-in list
    // once groups and deployment tags land. DECISION: a line carries the
    // tags of every relationship it stands for.
    #[must_use]
    pub fn tag_choices(&self) -> Vec<String> {
        const BUILT_IN: [&str; 6] = ["Element", "Person", "Software System", "Container", "Component", "Relationship"];
        let mut out = BTreeSet::new();
        for (tags, _) in self.view_items() {
            out.extend(tags.into_iter().filter(|t| !BUILT_IN.contains(&t.as_str())));
        }
        out.into_iter().collect()
    }

    /// The perspective names of the open view's elements and
    /// relationships, sorted (P2-726).
    #[must_use]
    pub fn perspective_choices(&self) -> Vec<String> {
        let mut out = BTreeSet::new();
        for (_, perspectives) in self.view_items() {
            out.extend(perspectives);
        }
        out.into_iter().collect()
    }

    /// The tags and perspective names of every box and line of the scene
    /// on screen, in scene order, with their items (P2-725, P2-726).
    fn view_items(&self) -> Vec<(Vec<String>, Vec<String>)> {
        self.view_items_keyed().into_iter().map(|(_, tags, p)| (tags, p)).collect()
    }

    /// `view_items` with each item's keys: a box, or a line and its label.
    fn view_items_keyed(&self) -> Vec<(Vec<ItemKey>, Vec<String>, Vec<String>)> {
        let (Some(scene), Some(model)) = (self.scene(), self.scene_model()) else {
            return Vec::new();
        };
        let mut out = Vec::new();
        for b in &scene.boxes {
            let e = model.element(b.element);
            let perspectives = e.perspectives.iter().map(|p| p.name.value.clone()).collect();
            out.push((vec![ItemKey::Box(b.id)], e.tags.clone(), perspectives));
        }
        for l in &scene.lines {
            let mut tags: Vec<String> = Vec::new();
            let mut perspectives: Vec<String> = Vec::new();
            for &r in &l.relationships {
                let rel = model.relationship(r);
                for t in &rel.tags {
                    if !tags.contains(t) {
                        tags.push(t.clone());
                    }
                }
                for p in &rel.perspectives {
                    if !perspectives.contains(&p.name.value) {
                        perspectives.push(p.name.value.clone());
                    }
                }
            }
            out.push((vec![ItemKey::Line(l.edge), ItemKey::Label(l.edge)], tags, perspectives));
        }
        out
    }

    /// The navigator's entries: the views above, next to and below the
    /// open view by C4 level, kept to those whose key or title contains
    /// the search text, ignoring case (P2-729).
    // TODO(srs): P2-729, the form. DECISION: reading taken (B
    // editor-breadth 7 asked for a searchable tree grouped by kind; the
    // fix-round ruling keeps three groups by C4 level). Above: views of a
    // higher level whose scope contains the open view's scope (a
    // landscape contains every scope, a context or container view of a
    // system its containers). Next to: views of the same level and kind.
    // Below: per enterable box of the open view, the views it enters — a
    // landscape's system box its context views, else the system's
    // container views (a container's component views), else EDIT-15
    // `view_into`. A filtered view takes its base view's level; custom
    // and image views have no level, so only same-kind views sit next to
    // them. Above and next to keep declaration order, below scene order;
    // an entry sits in the first group that takes it.
    /// The navigator's search text (P2-729).
    pub(crate) fn nav_query(&self) -> &str {
        &self.nav_query
    }

    #[must_use]
    pub fn navigator(&self) -> Vec<(NavLevel, ViewEntry)> {
        let (Some(model), Some(view)) = (self.model(), self.view.as_ref()) else {
            return Vec::new();
        };
        let place = c4_place(model, view);
        let kind = view_kind(model, view);
        let drawn = || model.views.iter().filter(|v| v.kind.drawable());
        let mut out: Vec<(NavLevel, ViewEntry)> = Vec::new();
        let push = |out: &mut Vec<(NavLevel, ViewEntry)>, level, r: ViewRef| {
            if r != *view && !out.iter().any(|(_, e)| e.view == r) {
                out.push((level, self.entry_of(model, r)));
            }
        };
        if let Some((rank, scope)) = place {
            for v in drawn() {
                let r = ViewRef::Declared(v.key.clone());
                if let Some((up, up_scope)) = c4_place(model, &r)
                    && up < rank
                    && scope_contains(model, up, up_scope, scope)
                {
                    push(&mut out, NavLevel::Above, r);
                }
            }
        }
        for v in drawn().filter(|v| Some(v.kind) == kind) {
            let r = ViewRef::Declared(v.key.clone());
            if c4_place(model, &r).map(|p| p.0) == place.map(|p| p.0) {
                push(&mut out, NavLevel::NextTo, r);
            }
        }
        if let Some(scene) = self.scene() {
            for b in &scene.boxes {
                if !enterable(model, b.element) {
                    continue;
                }
                let scoped = |k: Option<ViewKind>| -> Vec<ViewRef> {
                    drawn()
                        .filter(|v| Some(v.kind) == k && v.scope.element() == Some(b.element))
                        .map(|v| ViewRef::Declared(v.key.clone()))
                        .collect()
                };
                let e = model.element(b.element);
                let mut into = Vec::new();
                if place.map(|p| p.0) == Some(0) && e.kind == ElementKind::SoftwareSystem {
                    into = scoped(Some(ViewKind::SystemContext));
                }
                if into.is_empty() {
                    into = scoped(counting_kind(e.kind));
                }
                if into.is_empty() {
                    into.extend(view_into(model, b.element));
                }
                for r in into {
                    push(&mut out, NavLevel::Below, r);
                }
            }
        }
        let query = self.nav_query.to_lowercase();
        out.retain(|(_, e)| e.view.key().to_lowercase().contains(&query) || e.label.to_lowercase().contains(&query));
        out
    }

    /// The view list entry of `view` (EDIT-8): its drawn title, else its
    /// key; a default view's label is its scope's name.
    fn entry_of(&self, model: &Model, view: ViewRef) -> ViewEntry {
        let (label, kind) = match (&view, model.view(&view.key())) {
            (_, Some(v)) => (v.drawn_title().map_or_else(|| v.key.clone(), |t| t.value.clone()), v.kind),
            (ViewRef::Default { scope, kind }, None) => (
                model.find(scope).map_or_else(|| view.key(), |id| model.element(id).name.value.clone()),
                *kind,
            ),
            (ViewRef::Declared(key), None) => (key.clone(), ViewKind::SystemContext),
        };
        ViewEntry {
            current: self.view.as_ref() == Some(&view),
            view,
            label,
            kind,
        }
    }

    /// Opens `view` with the box of `element` (its canonical key)
    /// selected: a usage click or a graph or tree double click on the
    /// explore pages (P3-504, P3-512).
    // DECISION: an element with no key (nothing to select by) opens the
    // view with nothing selected; the selection is replaced, as a click
    // on the box would replace it.
    pub(crate) fn reveal(&mut self, view: ViewRef, element: String) -> Vec<Effect> {
        self.overlay = None;
        let effects = self.show(view);
        self.selection.clear();
        self.selection_order.clear();
        if !element.is_empty() {
            let picked = Selected::Element(element);
            self.selection.insert(picked.clone());
            self.selection_order.push(picked);
        }
        effects
    }

    /// True when Back has a view to return to (P4-114).
    pub(crate) fn can_go_back(&self) -> bool {
        self.visited.len() >= 2
    }

    /// True when Forward has a view to return to (P4-115).
    pub(crate) fn can_go_forward(&self) -> bool {
        !self.ahead.is_empty()
    }

    /// Forward: the view the last Back left (Alt+Right, the toolbar)
    /// (P4-115); nothing when Back was not used since the last view
    /// change.
    // DECISION: a crate-private method like `set_mode`, not a `Command`
    // variant, so the pub `Command` enum is unchanged; the toolbar calls
    // it and Alt+Right reaches it through `Key::ArrowRight`.
    pub(crate) fn forward(&mut self) -> Vec<Effect> {
        let Some(view) = self.ahead.pop() else { return Vec::new() };
        self.overlay = None;
        self.visited.push(view.clone());
        self.show(view)
    }

    /// Keeps the view history to views the new parse still has (P4-116):
    /// a default view written since is its declared view now, a view gone
    /// is dropped, and repeats left side by side are merged.
    fn prune_history(&mut self) {
        let Some(model) = self.model() else { return };
        let keep = |list: &[ViewRef]| -> Vec<ViewRef> {
            let mut out: Vec<ViewRef> = Vec::new();
            for v in list.iter().filter(|v| view_exists(model, v)) {
                let v = match v {
                    ViewRef::Default { .. } if model.view(&v.key()).is_some() => ViewRef::Declared(v.key()),
                    _ => v.clone(),
                };
                if out.last() != Some(&v) {
                    out.push(v);
                }
            }
            out
        };
        let (visited, ahead) = (keep(&self.visited), keep(&self.ahead));
        self.visited = visited;
        self.ahead = ahead;
    }

    /// The light/dark mode the canvas shows and exports in (P2-433).
    pub(crate) fn mode(&self) -> Mode {
        self.mode
    }

    /// The light/dark switch (P2-433): rebuilds the shown view in `mode`
    /// and drops the other views' builds (they were in the old mode);
    /// writes nothing and adds no undo step. Switching to the current mode
    /// does nothing.
    // DECISION: a crate-private method like `set_canvas_size`, not a
    // `Command` variant, so the pub `Command` enum is unchanged; the shell
    // calls it from the View menu and runs the effects.
    pub(crate) fn set_mode(&mut self, mode: Mode) -> Vec<Effect> {
        if self.mode == mode {
            return Vec::new();
        }
        self.mode = mode;
        self.cache.clear();
        self.request_shown()
    }

    /// Feeds one event and returns the effects to execute, in order.
    /// Covers the whole input table of EDIT-28, the commands, the file
    /// events (EDIT-1, 11, 22..24, 27) and worker replies (EDIT-25).
    /// Never blocks and never routes (EDIT-25). Serves EDIT-1..29.
    pub fn handle(&mut self, event: Event) -> Vec<Effect> {
        let before = self.camera;
        let effects = self.handle_event(event);
        self.settle_camera(before);
        // P2-707: the selection's order is the order its items were added
        // in; one event adds at most one item by a click, so syncing after
        // each event keeps it.
        let selection = &self.selection;
        self.selection_order.retain(|s| selection.contains(s));
        for s in selection {
            if !self.selection_order.contains(s) {
                self.selection_order.push(s.clone());
            }
        }
        // P2-714: the session's history of views shown.
        // DECISION: kept by `ViewRef` (keys, so it survives a re-parse),
        // consecutive repeats are one entry, bounded to `VISITED_MAX`
        // entries, the oldest dropped first.
        // P4-113: every view change but Back and Forward (which move
        // between the two stacks themselves) pushes and drops Forward's
        // views, as a browser does.
        if let Some(view) = &self.view
            && self.visited.last() != Some(view)
        {
            self.visited.push(view.clone());
            self.ahead.clear();
            if self.visited.len() > VISITED_MAX {
                self.visited.remove(0);
            }
        }
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
                if let Some(Overlay::QuickNavigation { query, highlighted }) = self.overlay.as_mut() {
                    *query = text;
                    *highlighted = 0;
                    return Vec::new();
                }
                if self.overlay == Some(Overlay::Navigator) {
                    self.nav_query = text;
                    return Vec::new();
                }
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
            hide_descriptions: !self.aids.descriptions,
            hide_metadata: !self.aids.metadata,
            hide_key: !self.aids.key,
            faded: self.faded(),
            tooltip: self.tooltip(),
        }
    }

    /// Every view of the file in declaration order, or as the view set's
    /// `structurizr.sort` orders them (P2-716), plus the open default view
    /// when one is shown (EDIT-8, EDIT-15). Empty with no parse.
    #[must_use]
    pub fn views(&self) -> Vec<ViewEntry> {
        let Some(model) = self.model() else {
            return Vec::new();
        };
        let mut out = listed_views(model, self.view.as_ref());
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

    /// The view crumb `n` of `breadcrumb` opens (EDIT-16, P2-305): the
    /// workspace crumb a landscape or context view, an element's crumb
    /// the view into it (`view_into`); `None` when there is none.
    pub(crate) fn crumb_target(&self, n: usize) -> Option<ViewRef> {
        let model = self.model()?;
        if n == 0 {
            // TODO(srs): P2-305, the workspace crumb. DECISION: reading
            // taken (B landscape-filtered 8): the first system landscape
            // view, else the first system context view (EDIT-16), else
            // the first view.
            return model
                .views
                .iter()
                .find(|v| v.kind == ViewKind::SystemLandscape)
                .or_else(|| model.views.iter().find(|v| v.kind == ViewKind::SystemContext))
                .or_else(|| model.views.first())
                .map(|v| ViewRef::Declared(v.key.clone()));
        }
        self.breadcrumb()
            .get(n)
            .and_then(|c| c.scope.as_deref())
            .and_then(|k| model.find(k))
            .and_then(|id| view_into(model, id))
    }

    /// The kind of `view` in the open model, `None` without one.
    pub(crate) fn kind_of(&self, view: &ViewRef) -> Option<ViewKind> {
        view_kind(self.model()?, view)
    }

    /// The details of the single selected item, `None` for no selection
    /// or several items (EDIT-10, EDIT-5).
    #[must_use]
    pub fn details(&self) -> Option<Details> {
        if self.selection.len() != 1 {
            return None;
        }
        let subject = self.selection.iter().next()?.clone();
        self.details_of(subject)
    }

    /// The details of `subject` (EDIT-10), the selection or a tooltip's
    /// item (P2-723).
    fn details_of(&self, subject: Selected) -> Option<Details> {
        let model = self.model()?;
        match &subject {
            Selected::Element(key) => {
                let e = model.element(model.find(key)?);
                // P3-116, P3-131: an instance shows its element's texts.
                let of = e.instance_of.map(|x| model.element(x));
                Some(Details {
                    name: e.name.value.clone(),
                    kind: kind_label(e.kind, e.is_external()).to_owned(),
                    description: e
                        .description
                        .as_ref()
                        .or_else(|| of.and_then(|o| o.description.as_ref()))
                        .map(|d| d.value.clone()),
                    technology: of.map_or(e, |o| o).technology.as_ref().map(|t| t.value.clone()),
                    tags: e.tags.clone(),
                    lifted_from: None,
                    merged: Vec::new(),
                    step: None,
                    instances: (e.kind == ElementKind::DeploymentNode)
                        .then(|| e.instances.as_ref().map_or_else(|| "1".to_owned(), |i| i.value.clone())),
                    environment: e
                        .environment
                        .and_then(|env| model.environments.get(env.0 as usize))
                        .map(|env| env.name.value.clone()),
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
                    step: None,
                    instances: None,
                    environment: None,
                    subject,
                })
            }
            Selected::Step(step) => self.step_details(*step, subject.clone()),
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
                    step: None,
                    instances: None,
                    environment: None,
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

    /// Every notice of the open file's last good parse (P2-126), in file
    /// order: `<path>:<line>:<column>: <construct> is accepted and has no
    /// effect`, `<path>` being the file it is in. The findings panel lists
    /// them.
    #[must_use]
    pub(crate) fn notices(&self) -> Vec<String> {
        let (Some(doc), Some(parsed)) = (&self.doc, self.parsed()) else {
            return Vec::new();
        };
        parsed
            .model
            .notices
            .iter()
            .map(|n| match n.span {
                Some(span) => {
                    let path = match span.file.0 {
                        0 => doc.path.as_path(),
                        i => parsed.paths.get(i as usize).map_or(doc.path.as_path(), PathBuf::as_path),
                    };
                    format!("{}:{n}", path.display())
                }
                None => n.to_string(),
            })
            .collect()
    }

    /// The files the open diagram includes, as the last parse read them
    /// (also a failed one), in path order: what the shell watches
    /// besides the root file (P2-141).
    #[must_use]
    pub(crate) fn included_files(&self) -> Vec<PathBuf> {
        self.included.keys().cloned().collect()
    }

    /// The included file the current parse error is in; `None` with no
    /// parse error or an error in the root file (P2-136, EDIT-11).
    #[must_use]
    pub(crate) fn parse_error_file(&self) -> Option<&Path> {
        self.doc.as_ref()?.parse_error.as_ref()?;
        self.error_file.as_deref()
    }

    /// A watcher event for an included file (P2-141): a change whose hash
    /// is the one last read or written by the app is ignored (EDIT-22);
    /// any other change, or the file going away, re-parses the root text
    /// with the files on disk, as an EDIT-1 reload.
    pub(crate) fn included_file(&mut self, path: &Path, event: &FileEvent) -> Vec<Effect> {
        self.notice = None;
        let Some(&known) = self.included.get(path) else {
            return Vec::new();
        };
        match event {
            FileEvent::Changed { hash, .. } if *hash == known => return Vec::new(),
            FileEvent::Changed { .. } | FileEvent::Removed => {}
            _ => return Vec::new(),
        }
        let (Some(doc), Some(text)) = (&self.doc, self.root_text.clone()) else {
            return Vec::new();
        };
        if doc.missing {
            return Vec::new();
        }
        let hash = doc.hash;
        self.reload(&text, hash)
    }

    /// True when `view` is a declared view whose kind is not drawn yet
    /// (P2-2, P2-123).
    pub(crate) fn unsupported(&self, view: &ViewRef) -> bool {
        self.model().is_some_and(|m| is_unsupported(m, view))
    }

    /// Parses `text`, the root file at `path`, reading its included files
    /// through `files::Reads` (P2-5), and records the root text, the files
    /// read with their hashes (P2-141) and the included file an error is
    /// in (P2-136).
    ///
    /// A root file whose first non-whitespace byte is `{` is a
    /// Structurizr workspace JSON (P3-416, P3-420): it is parsed as the
    /// one-line DSL `workspace extends "<its file name>" {}` at its own
    /// path, so the JSON becomes the base (P3-419) and its model the
    /// file's model; the document is read-only (`json_doc`).
    // DECISION: the editor reads JSON through `workspace extends` because
    // it needs a `Parsed`, which `model::parse_json` does not give; the
    // model is the same (P3-419 seeds the builder from the JSON).
    fn parse_doc(&mut self, path: &Path, text: &str) -> Result<Parsed, ModelError> {
        let mut reads = files::Reads::default();
        let result = if is_json_text(text) {
            let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
            // The stub is parsed at a sibling path that is never read or
            // written, so `extends` of the file itself is no cycle. The
            // name is a DSL string: `"` and `\` escaped, so it names this
            // file and no other.
            let stub = path.with_file_name(format!("{name}{}", crate::worker::JSON_STUB_SUFFIX));
            let quoted = name.replace('\\', "\\\\").replace('"', "\\\"");
            parse_with(&format!("workspace extends \"{quoted}\" {{\n}}\n"), Some(&stub), &mut reads)
        } else {
            parse_with(text, Some(path), &mut reads)
        };
        self.root_text = Some(text.to_owned());
        self.error_file = match &result {
            Ok(_) => None,
            Err(e) => (e.span.file.0 as usize)
                .checked_sub(1)
                .and_then(|i| reads.read.get(i))
                .map(|(p, _)| p.clone()),
        };
        let read: BTreeMap<PathBuf, ContentHash> = reads.read.into_iter().collect();
        if result.is_err() {
            // DECISION: after a failed parse the files of the last parse
            // stay watched too (one that failed to read was not read), with
            // a hash no content has, so any change to them re-parses.
            for path in std::mem::take(&mut self.included).into_keys() {
                if !read.contains_key(&path) {
                    self.included.insert(path, ContentHash::default());
                }
            }
            self.included.extend(read);
        } else {
            self.included = read;
        }
        result
    }

    /// True when edits are refused: no parse, a parse error, the file
    /// missing (EDIT-11, EDIT-27), or a workspace JSON (P3-420). Undo and
    /// redo are refused too.
    #[must_use]
    pub fn read_only(&self) -> bool {
        match &self.doc {
            Some(doc) => doc.parsed.is_none() || doc.parse_error.is_some() || doc.missing || self.json_doc(),
            None => true,
        }
    }

    /// True when the open file is a Structurizr workspace JSON, which
    /// the editor shows and exports but never writes (P3-420).
    pub(crate) fn json_doc(&self) -> bool {
        self.doc.is_some() && self.root_text.as_deref().is_some_and(is_json_text)
    }

    /// Sets the P3-420 notice and returns true when the open file is a
    /// workspace JSON, so the caller refuses the edit.
    fn refuse_json(&mut self) -> bool {
        let json = self.json_doc();
        if json {
            self.notice = Some(JSON_REFUSED.to_owned());
        }
        json
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
    #[expect(clippy::too_many_lines, reason = "one flat arm per action reads best")]
    pub fn edits_for(&self, action: &Action, parsed: &Parsed) -> Vec<Edit> {
        let model = &parsed.model;
        match action {
            Action::Move { view, boxes, layout } => {
                let view = &layout_view(model, view);
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
                let view = &layout_view(model, view);
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
            Action::Vertices {
                view,
                boxes,
                vias,
                layout,
            } => {
                let view = &layout_view(model, view);
                let mut own: Vec<Edit> = boxes
                    .iter()
                    .map(|(element, &centre)| Edit::Move {
                        view: view.key(),
                        element: element.clone(),
                        centre: Some(centre),
                    })
                    .collect();
                own.extend(vias.iter().map(|v| Edit::SetVia {
                    view: view.key(),
                    source: v.source.clone(),
                    destination: v.destination.clone(),
                    order: v.order.clone(),
                    points: v.points.clone(),
                }));
                layout_batch(model, view, layout, Some(boxes), own)
            }
            Action::Anchor { view, change, layout } => {
                let view = &layout_view(model, view);
                let own = vec![Edit::SetAnchor {
                    view: view.key(),
                    source: change.source.clone(),
                    destination: change.destination.clone(),
                    order: change.order.clone(),
                    from: change.from,
                    to: change.to,
                }];
                layout_batch(model, view, layout, None, own)
            }
            Action::Undo(step) | Action::Redo(step) => rebuild_first_write(parsed, &step.edits),
            Action::SwapSteps { view, first } => vec![Edit::SwapSteps {
                view: view.key(),
                first: *first,
            }],
            Action::SetLine {
                view,
                relationship,
                change,
            } => {
                let view = &layout_view(model, view);
                let mut out = add_default(model, view);
                let (view, relationship) = (view.key(), relationship.clone());
                out.push(match *change {
                    LineChange::Position(position) => Edit::SetLinePosition {
                        view,
                        relationship,
                        position,
                    },
                    LineChange::Routing(routing) => Edit::SetLineRouting {
                        view,
                        relationship,
                        routing,
                    },
                    LineChange::Jump(jump) => Edit::SetLineJump {
                        view,
                        relationship,
                        jump,
                    },
                });
                out
            }
            Action::SetCanvasSize {
                view,
                size,
                boxes,
                layout,
            } => {
                let view = &layout_view(model, view);
                let mut own: Vec<Edit> = boxes
                    .iter()
                    .map(|(element, &centre)| Edit::Move {
                        view: view.key(),
                        element: element.clone(),
                        centre: Some(centre),
                    })
                    .collect();
                own.push(Edit::SetCanvasSize {
                    view: view.key(),
                    size: *size,
                });
                layout_batch(model, view, layout, Some(boxes), own)
            }
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
/// `camera`, within `HANDLE_PX`, with every handle on the box edge (no
/// line ends to keep clear); corners win over sides (EDIT-18). The
/// editor's own hit test keeps line ends clear (`handle_spots`, P2-735).
#[must_use]
#[allow(dead_code, reason = "the EDIT-18 contract reader; the editor hits handle_spots with line ends (P2-735)")]
pub fn handle_at(rect: Rect, camera: &Camera, at: ScreenPoint) -> Option<Handle> {
    handle_spots(rect, camera, &[])
        .into_iter()
        .find(|(_, c)| (at.x - c.x).abs() <= HANDLE_PX && (at.y - c.y).abs() <= HANDLE_PX)
        .map(|(h, _)| h)
}

/// Length of a line's end kept clear of handles, scene px: the arrowhead
/// (P2-735).
// DECISION: 10 scene px, the arrowhead length `place` reserves
// (`ARROW_LEN_PX`), plus the 2px gap a tip may stand off the edge.
const LINE_END_CLEAR_PX: f32 = 12.0;

/// Each line end of `scene` with the stretch of line behind it that its
/// arrowhead covers, screen px through `camera`: `(end, toward)`, both
/// ends of every line (P2-735).
pub(crate) fn line_ends(scene: &Scene, camera: &Camera) -> Vec<(ScreenPoint, ScreenPoint)> {
    let mut out = Vec::new();
    for l in &scene.lines {
        let n = l.points.len();
        if n < 2 {
            continue;
        }
        for (end, next) in [(l.points[0], l.points[1]), (l.points[n - 1], l.points[n - 2])] {
            let e = camera.to_screen(end);
            let t = camera.to_screen(next);
            let (dx, dy) = (t.x - e.x, t.y - e.y);
            let len = dx.hypot(dy);
            let reach = (LINE_END_CLEAR_PX * camera.zoom).min(len);
            let toward = if len > 0.0 {
                ScreenPoint {
                    x: e.x + dx / len * reach,
                    y: e.y + dy / len * reach,
                }
            } else {
                e
            };
            out.push((e, toward));
        }
    }
    out
}

/// The eight resize handles of `rect` (scene px), centres in screen px
/// through `camera`, corners first. A handle whose square would cover a
/// line end or its arrowhead (`ends`, from `line_ends`) is drawn outside
/// the box edge, slid along it when straight out still covers (P2-735,
/// EDIT-18).
// DECISION: candidates in order: on the edge (EDIT-18), straight out by
// two handle sides, then out and slid either way along the edge by three
// and six half sides; the first clear one wins, the last if none is (a
// crowded edge). The hit test (`handle_under`) uses the same centres.
pub(crate) fn handle_spots(rect: Rect, camera: &Camera, ends: &[(ScreenPoint, ScreenPoint)]) -> [(Handle, ScreenPoint); 8] {
    let a = camera.to_screen(rect.min);
    let b = camera.to_screen(rect.max);
    let (mx, my) = (f32::midpoint(a.x, b.x), f32::midpoint(a.y, b.y));
    let covers = |c: ScreenPoint| {
        let reach = HANDLE_PX + 1.0;
        ends.iter().any(|&(end, toward)| {
            (0..=6_u8).any(|step| {
                let f = f32::from(step) / 6.0;
                let px = end.x + (toward.x - end.x) * f;
                let py = end.y + (toward.y - end.y) * f;
                (px - c.x).abs() <= reach && (py - c.y).abs() <= reach
            })
        })
    };
    let out_by = 4.0 * HANDLE_PX;
    let slide = 3.0 * HANDLE_PX;
    let spot = |h: Handle, x: f32, y: f32, ox: f32, oy: f32| {
        // Along the edge: x for top and bottom, y for left and right; a
        // corner slides across its diagonal.
        let (tx, ty) = if oy == 0.0 {
            (0.0, 1.0)
        } else if ox == 0.0 {
            (1.0, 0.0)
        } else {
            (ox, -oy)
        };
        let at = |o: f32, s: f32| ScreenPoint {
            x: x + ox * o + tx * s,
            y: y + oy * o + ty * s,
        };
        let pick = [
            at(0.0, 0.0),
            at(out_by, 0.0),
            at(out_by, slide),
            at(out_by, -slide),
            at(out_by, 2.0 * slide),
            at(out_by, -2.0 * slide),
        ];
        (h, pick.iter().copied().find(|&p| !covers(p)).unwrap_or(pick[5]))
    };
    [
        spot(Handle::NW, a.x, a.y, -1.0, -1.0),
        spot(Handle::NE, b.x, a.y, 1.0, -1.0),
        spot(Handle::SE, b.x, b.y, 1.0, 1.0),
        spot(Handle::SW, a.x, b.y, -1.0, 1.0),
        spot(Handle::N, mx, a.y, 0.0, -1.0),
        spot(Handle::E, b.x, my, 1.0, 0.0),
        spot(Handle::S, mx, b.y, 0.0, 1.0),
        spot(Handle::W, a.x, my, -1.0, 0.0),
    ]
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

/// `align` with the first selected box as the reference (P2-707): its
/// edge, centre or middle is the line every box moves to; without a
/// reference in `boxes`, `align`'s outermost edges.
// TODO(srs): P2-707, the alignment reference. DECISION: reading taken (B
// editor-breadth 4, Structurizr 6.2.4 `alignSelectedElementsLeft` uses
// `selectedElements[0]`): the first box selected. This also changes the
// EDIT-20 commands, which P2-707 makes the same as the keys.
fn align_to(boxes: &BTreeMap<BoxId, Rect>, how: Alignment, reference: Option<BoxId>) -> BTreeMap<BoxId, Point> {
    let Some(r) = reference.and_then(|id| boxes.get(&id)) else {
        return align(boxes, how);
    };
    if boxes.len() < 2 {
        return BTreeMap::new();
    }
    let c = r.centre();
    boxes
        .iter()
        .map(|(&id, b)| {
            let bc = b.centre();
            let (hw, hh) = (b.width() / 2, b.height() / 2);
            let (x, y) = match how {
                Alignment::Left => (r.min.x + hw, bc.y),
                Alignment::Right => (r.max.x - hw, bc.y),
                Alignment::Centre => (c.x, bc.y),
                Alignment::Top => (bc.x, r.min.y + hh),
                Alignment::Bottom => (bc.x, r.max.y - hh),
                Alignment::Middle => (bc.x, c.y),
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
    if image_view_of(model, element).is_some() {
        return true;
    }
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

/// The views a box of `element` drawn in a view of kind `from` opens:
/// the context menu's "Enter <view>" entries (editor-ux-02) and the view
/// map's links (P4-105). The first is the one EDIT-15 opened by double
/// click (`view_into`), then every other declared view scoped to it (a
/// landscape's system also its context views). Empty when `element` is
/// not enterable.
pub(crate) fn views_entered(model: &Model, from: Option<ViewKind>, element: ElementId) -> Vec<ViewRef> {
    let mut out: Vec<ViewRef> = Vec::new();
    if !enterable(model, element) {
        return out;
    }
    out.extend(view_into(model, element));
    let e = model.element(element);
    let landscape = from == Some(ViewKind::SystemLandscape);
    let kinds = [counting_kind(e.kind), (landscape && e.kind == ElementKind::SoftwareSystem).then_some(ViewKind::SystemContext)];
    for v in model.views.iter().filter(|v| v.kind.drawable() && v.scope.element() == Some(element)) {
        let r = ViewRef::Declared(v.key.clone());
        if kinds.contains(&Some(v.kind)) && !out.contains(&r) {
            out.push(r);
        }
    }
    out
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
    let counting = counting_kind(e.kind);
    if let Some(v) = model
        .views
        .iter()
        .find(|v| Some(v.kind) == counting && v.scope.element() == Some(element))
    {
        return Some(ViewRef::Declared(v.key.clone()));
    }
    if let Some(key) = image_view_of(model, element) {
        return Some(ViewRef::Declared(key.to_owned()));
    }
    let kind = counting?;
    Some(ViewRef::Default {
        scope: e.key.as_ref()?.value.clone(),
        kind,
    })
}

/// The declared views of the view list (EDIT-8) in its order (P2-716),
/// `current` marking the view on screen; the view map's nodes with
/// `current` `None` (P4-101).
pub(crate) fn listed_views(model: &Model, current: Option<&ViewRef>) -> Vec<ViewEntry> {
    // P2-314: a filtered view's base is left out of the list, unless
    // it is on screen (opened by EDIT-15 or EDIT-16).
    // TODO(srs): P2-314, EDIT-15/16 opening a base view. DECISION:
    // reading taken (B landscape-filtered 5): they may open it, and it
    // shows in the list until another view is opened.
    let hidden = model.filtered_bases();
    let mut out: Vec<ViewEntry> = model
        .views
        .iter()
        .filter(|v| !hidden.contains(&v.key.as_str()) || current.is_some_and(|s| s.key() == v.key))
        .map(|v| {
            let view = ViewRef::Declared(v.key.clone());
            ViewEntry {
                current: current == Some(&view),
                label: v.drawn_title().map_or_else(|| v.key.clone(), |t| t.value.clone()),
                kind: v.kind,
                view,
            }
        })
        .collect();
    sort_views(model, &mut out);
    out
}

/// Orders the declared views of the view list by the view set's
/// `structurizr.sort` (P2-716): `key` by key, `type` by kind then key,
/// `created` or none in declaration order (stable).
// TODO(srs): P2-716, the type order, the tie-breaks and the default.
// DECISION: reading taken (B editor-breadth 8): no property keeps
// declaration order (EDIT-8); `type` is landscape, context, container,
// component, filtered, custom, image, dynamic, deployment, then key;
// keys compare byte by byte (deterministic; Structurizr uses the
// browser's `localeCompare`). Structurizr 6.2.4 sorts `type` by the base
// view's kind for a filtered view and by scope before key, and with no
// property by software system; B's [P] rulings are kept.
fn sort_views(model: &Model, list: &mut [ViewEntry]) {
    let sort = model
        .views_properties
        .iter()
        .rev()
        .find(|p| p.name.value == "structurizr.sort")
        .map(|p| p.value.value.to_ascii_lowercase());
    let rank = |kind: ViewKind| match kind {
        ViewKind::SystemLandscape => 0,
        ViewKind::SystemContext => 1,
        ViewKind::Container => 2,
        ViewKind::Component => 3,
        ViewKind::Filtered => 4,
        ViewKind::Custom => 5,
        ViewKind::Image => 6,
        ViewKind::Dynamic => 7,
        _ => 8,
    };
    match sort.as_deref() {
        Some("key") => list.sort_by_key(|e| e.view.key()),
        Some("type") => list.sort_by_key(|e| (rank(e.kind), e.view.key())),
        _ => {}
    }
}

/// True for an `http://` or `https://` URL, ignoring case (P2-719).
fn is_web_url(url: &str) -> bool {
    let lower = url.trim().to_ascii_lowercase();
    lower.starts_with("https://") || lower.starts_with("http://")
}

/// The canvas size list (P2-701): every `PaperSize`, ordered by area,
/// ties in Structurizr's enum order (portrait before landscape).
// TODO(srs): P2-701 (a). DECISION: reading taken (B editor-breadth 5a):
// ordered by area. Structurizr 6.2.4 lists landscape sizes then portrait
// ones (`PaperSize.getOrderedPaperSizes`) and its editor now steps free
// dimensions instead; B's [P] ruling is kept.
fn paper_sizes() -> Vec<PaperSize> {
    use PaperSize as P;
    let mut list = vec![
        P::A6Portrait,
        P::A6Landscape,
        P::A5Portrait,
        P::A5Landscape,
        P::A4Portrait,
        P::A4Landscape,
        P::A3Portrait,
        P::A3Landscape,
        P::A2Portrait,
        P::A2Landscape,
        P::A1Portrait,
        P::A1Landscape,
        P::A0Portrait,
        P::A0Landscape,
        P::LetterPortrait,
        P::LetterLandscape,
        P::LegalPortrait,
        P::LegalLandscape,
        P::Slide4x3,
        P::Slide16x9,
        P::Slide16x10,
    ];
    list.sort_by_key(|p| {
        let (w, h) = p.size_px();
        i64::from(w) * i64::from(h)
    });
    list
}

/// The first image view scoped to `element` (P2-322 f).
// TODO(srs): P2-322 (f). DECISION: reading taken (B landscape-filtered
// 10f): an element-scoped image view makes its element enterable, and a
// right-click menu offers it when no container or component view of the element is
// declared (the cookbook's "level 4" zoom-in).
fn image_view_of(model: &Model, element: ElementId) -> Option<&str> {
    model
        .views
        .iter()
        .find(|v| v.kind == ViewKind::Image && v.scope.element() == Some(element))
        .map(|v| v.key.as_str())
}

/// The view shown on opening `model` (EDIT-13, EDIT-15, EDIT-27):
/// `remembered` if it still exists (declared, or a default view that
/// still opens) and is drawn, else the first declared view that is drawn
/// (P2-123: never an unsupported one), else (a model-only file, or only
/// unsupported views) the system context default view of the first
/// software system; `None` when the model has no software system.
#[must_use]
pub fn initial_view(model: &Model, remembered: Option<&ViewRef>) -> Option<ViewRef> {
    if let Some(view) = remembered
        && view_exists(model, view)
        && !is_unsupported(model, view)
    {
        return Some(match view {
            ViewRef::Default { .. } if model.view(&view.key()).is_some() => ViewRef::Declared(view.key()),
            _ => view.clone(),
        });
    }
    // P2-338, P2-717: the view carrying `default`, when it is drawn.
    // TODO(srs): P2-717. DECISION: reading taken (B editor-breadth 10): a
    // remembered view beats `default`, and the EDIT-27 fallback (this
    // function with no remembered view) prefers it.
    if let Some(v) = model.default_view_key().and_then(|k| model.view(k))
        && v.kind.drawable()
    {
        return Some(ViewRef::Declared(v.key.clone()));
    }
    // P2-123: the editor never opens an unsupported view by itself.
    if let Some(v) = model.views.iter().find(|v| v.kind.drawable()) {
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
        Selected::Step(step) => steps::step_text_fields(model, view, *step),
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
        ElementKind::DeploymentNode => "Deployment Node",
        ElementKind::InfrastructureNode => "Infrastructure Node",
        ElementKind::SoftwareSystemInstance => "Software System Instance",
        ElementKind::ContainerInstance => "Container Instance",
        _ => "Element",
    }
}

// ------------------------------------------------------------ internals

impl State {
    /// The model of the last good parse.
    pub(crate) fn model(&self) -> Option<&Model> {
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
            Gesture::Dragging { .. }
                | Gesture::Resizing { .. }
                | Gesture::RubberBand { .. }
                | Gesture::Pressed { .. }
                | Gesture::DraggingVertex { .. }
                | Gesture::DraggingEnd { .. }
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
        if self.unsupported(&view) {
            // P2-123: an unsupported view draws nothing; a build in flight
            // for the previous view is stale.
            self.shown = None;
            self.awaited_build = Some(self.next_generation());
            return Vec::new();
        }
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
                mode: self.mode,
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
            .filter(|v| v.kind.drawable())
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
                        mode: self.mode,
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
        self.sticky = None;
        self.cancel_gesture();
        self.gesture = Gesture::Idle;
        self.hover = None;
        // DECISION: a view switch clears the selection rather than keeping
        // the items also drawn in the new view: EDIT-28 makes the selection
        // what was clicked on the shown view, `Title` and `Step(n)` name
        // items of one view only, and the new scene may not be built yet
        // to check against. `reveal` sets its own selection after this.
        self.selection.clear();
        self.selection_order.clear();
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
        self.prune_history();
    }

    // ---------------------------------------------------------- writes

    /// Writes `action`: its batch applied to the last good parse and sent
    /// as a checked write (EDIT-3, EDIT-24); refused while read-only or a
    /// write is in flight.
    fn write(&mut self, action: Action) -> Vec<Effect> {
        if self.refuse_json() {
            self.give_back(action);
            return Vec::new();
        }
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
            // P3-419, P3-420: a batch that would rewrite a `workspace
            // extends <file>.json` base is refused: that text was written
            // from the JSON, and the editor never writes JSON.
            Ok(applied)
                if applied.files.iter().any(|(file, _)| {
                    parsed.paths.get(file.0 as usize).is_some_and(|p| {
                        p.extension().is_some_and(|e| e.eq_ignore_ascii_case("json"))
                    })
                }) =>
            {
                self.notice = Some(JSON_BASE_REFUSED.to_owned());
                self.preview = None;
                Vec::new()
            }
            Ok(applied) => {
                // P2-139: the patched included files first, each checked
                // against its own hash (EDIT-24), then the root file when
                // its text changed; one at a time, each after the last
                // landed, so a refused write stops the batch.
                // DECISION: the root file is not rewritten when only
                // included files changed (its bytes stay, P2-139); a batch
                // with no included file always writes the root, as before.
                let mut queue: VecDeque<WriteRequest> = applied
                    .files
                    .iter()
                    .filter_map(|(file, text)| {
                        let p = parsed.paths.get(file.0 as usize)?.clone();
                        let expected = self.included.get(&p).copied().unwrap_or_default();
                        Some(WriteRequest {
                            path: p,
                            text: text.clone(),
                            expected,
                        })
                    })
                    .collect();
                if queue.is_empty() || parsed.trees.first().is_none_or(|t| t.print() != applied.text) {
                    queue.push_back(WriteRequest {
                        path,
                        text: applied.text.clone(),
                        expected,
                    });
                }
                self.pending = Some(Pending {
                    action,
                    inverse: applied.inverse,
                    text: applied.text,
                });
                let first = queue.pop_front();
                self.queued = queue;
                self.in_flight = first.as_ref().map(|r| r.path.clone());
                first.map(Effect::Write).into_iter().collect()
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

    /// Moves boxes to `to` (grid centres) as one step (EDIT-3, EDIT-20);
    /// with `from` (a drag's start centres), the vertices of every line
    /// whose two ends moved by the same delta move too, in the same write
    /// (P3-314).
    fn write_moves(&mut self, to: &BTreeMap<BoxId, Point>, from: Option<&BTreeMap<BoxId, Point>>) -> Vec<Effect> {
        let carried = from.map(|f| self.carried_vias(f, to)).unwrap_or_default();
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
        if !carried.is_empty() {
            return self.write_vias(carried, boxes);
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
            Input::PointerMove { at } => {
                // DECISION: a move outside the reported canvas with no
                // gesture under way is the pointer leaving (P2-712): a
                // line beyond the canvas edge is not hovered, and the
                // re-hit after a write has no stale point to find it by.
                if matches!(self.gesture, Gesture::Idle) && !self.on_canvas(at) {
                    self.pointer = None;
                    self.hover = None;
                    return Vec::new();
                }
                self.pointer = Some(at);
                self.pointer_move(at)
            }
            Input::PointerUp { at, button } => self.pointer_up(at, button),
            Input::PointerLeft => {
                self.pointer = None;
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
                    // P2-715: a Space press released with no drag in
                    // between opens quick navigation.
                    if std::mem::take(&mut self.space_tap) && self.text_edit.is_none() && self.overlay.is_none() {
                        return self.command(Command::QuickNavigation);
                    }
                }
                Vec::new()
            }
            Input::CanvasSize { size } => {
                self.set_canvas_size(size);
                Vec::new()
            }
            Input::DoubleClick { at, mods } => self.double_click(at, mods),
        }
    }

    /// Whether `at` lies on the canvas last reported (true while no size
    /// is known).
    fn on_canvas(&self, at: ScreenPoint) -> bool {
        self.canvas
            .is_none_or(|size| (0.0..=size.x).contains(&at.x) && (0.0..=size.y).contains(&at.y))
    }

    fn pointer_down(&mut self, at: ScreenPoint, button: Button, mods: Modifiers) -> Vec<Effect> {
        // P2-715: a press while Space is held starts a pan, not a tap.
        self.space_tap = false;
        // editor-ux-02: a press on the canvas closes the context menu.
        if matches!(self.overlay, Some(Overlay::ContextMenu { .. })) {
            self.overlay = None;
        }
        if button == Button::Middle || (button == Button::Left && self.space) {
            self.gesture = Gesture::Panning { last: at };
            return Vec::new();
        }
        let hit = self.hit(at);
        // P3-420: on a workspace JSON the handle is found too, so the press
        // says why a resize is refused instead of starting a rubber band.
        let handle = if button == Button::Left && (!self.read_only() || self.json_doc()) {
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
        // P2-735: the handles as drawn, moved off line ends.
        let ends = line_ends(scene, &self.camera);
        handle_spots(rect, &self.camera, &ends)
            .into_iter()
            .find(|(_, c)| (at.x - c.x).abs() <= HANDLE_PX && (at.y - c.y).abs() <= HANDLE_PX)
            .map(|(h, _)| (id, h))
    }

    fn pointer_move(&mut self, at: ScreenPoint) -> Vec<Effect> {
        match self.gesture.clone() {
            Gesture::DraggingVertex { .. } => self.vertex_drag_to(at),
            Gesture::DraggingEnd { .. } => self.end_drag_to(at),
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
                if distance(from, at) < CLICK_SLOP_PX {
                    return Vec::new();
                }
                // editor-ux-03: a right drag pans like a middle drag,
                // from where the button went down, and never opens the
                // context menu.
                if button == Button::Right {
                    self.gesture = Gesture::Panning { last: from };
                    return self.pointer_move(at);
                }
                if button != Button::Left {
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
        // P3-420: a move or resize on a workspace JSON says why it is
        // refused, and starts nothing else (no rubber band).
        if (matches!(hit, Some(ItemKey::Box(_))) || handle.is_some() || self.vertex_under(from).is_some() || self.end_under(from).is_some())
            && self.refuse_json()
        {
            self.gesture = Gesture::Idle;
            return Vec::new();
        }
        let read_only = self.read_only();
        if !read_only && let Some((edge, index)) = self.vertex_under(from) {
            return self.start_vertex_drag(edge, index, at);
        }
        // P4-214: an end handle of the selected line wins over the box
        // under it and its resize handles.
        if !read_only && let Some((edge, destination)) = self.end_under(from) {
            return self.start_end_drag(edge, destination, at);
        }
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
            Gesture::DraggingVertex { edge, vertices, .. } => self.finish_vertex_drag(edge, &vertices),
            Gesture::DraggingEnd {
                edge,
                destination,
                snapped,
                ..
            } => self.finish_end_drag(edge, destination, snapped),
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
                if b == Button::Left
                    && let Some(effects) = self.vertex_click(at, mods, hit)
                {
                    return effects;
                }
                let effects = self.click(b, mods, hit);
                if b == Button::Right {
                    self.open_context_menu(at, hit);
                }
                effects
            }
            Gesture::Dragging { boxes, to, .. } => {
                self.dragged = None;
                self.awaited_drag = None;
                if to == boxes {
                    self.preview = None;
                    return Vec::new();
                }
                // The preview stays until the saved file's build arrives.
                self.write_moves(&to, Some(&boxes))
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
    fn click(&mut self, _button: Button, mods: Modifiers, hit: Option<ItemKey>) -> Vec<Effect> {
        self.text_edit = None;
        // TODO(srs): P2-724, the release. DECISION: reading taken (B
        // editor-breadth 9): Escape, or a click anywhere but the item.
        if self.sticky.is_some() && self.sticky != hit {
            self.sticky = None;
        }
        let (Some(scene), Some(model)) = (self.scene(), self.model()) else {
            self.selection.clear();
            return Vec::new();
        };
        let selected = hit.and_then(|h| selected_of(scene, model, h));
        // P2-718: Alt+click toggles like Shift+click and never enters (B
        // editor-breadth 3, amending EDIT-28).
        if mods.shift || mods.alt {
            if let Some(s) = selected
                && !self.selection.remove(&s)
            {
                self.selection.insert(s);
            }
            return Vec::new();
        }
        // EDIT-15 as amended by the owner: a single click selects an
        // enterable box like any other; the right-click menu enters it
        // (`open_context_menu`). Every button selects.
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

    #[expect(clippy::too_many_lines, reason = "the whole P2-712 key table in one place")]
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
        // P2-712 (1): an open quick-navigation list takes every key.
        if let Some(Overlay::QuickNavigation { .. }) = self.overlay {
            return self.quick_nav_key(key);
        }
        match key {
            Key::Enter | Key::F2 => {
                self.open_text_edit();
                return Vec::new();
            }
            Key::Escape => {
                // DECISION: Escape closes an open list first; otherwise it
                // releases a sticky tooltip (P2-724, B editor-breadth 9)
                // and deselects (EDIT-28) in the same press.
                if self.overlay.take().is_none() {
                    self.sticky = None;
                    self.selection.clear();
                }
                return Vec::new();
            }
            Key::Backspace if !text_focus => return self.go_up(),
            // P4-114 (amends EDIT-16): Alt+Left is Back, Alt+Right
            // Forward; Backspace alone still opens the parent crumb.
            Key::ArrowLeft if mods.alt => return self.command(Command::Back),
            Key::ArrowRight if mods.alt => return self.forward(),
            Key::Space => {
                self.space = true;
                self.space_tap = !text_focus;
                return Vec::new();
            }
            Key::Z if mods.ctrl && mods.shift => return self.command(Command::Redo),
            Key::Z if mods.ctrl => return self.command(Command::Undo),
            Key::Y if mods.ctrl => return self.command(Command::Redo),
            _ => {}
        }
        // P2-712: the single-key bindings act only with no text focus and
        // no chord modifier.
        if text_focus || mods.ctrl {
            return Vec::new();
        }
        // P2-712 (2): a line or label under the pointer.
        if let Some(effects) = self.line_key(key) {
            return effects;
        }
        // P2-712 (3): two or more selected boxes.
        if self.selected_rects().len() >= 2 {
            let command = match key {
                Key::L => Some(Command::Align(Alignment::Left)),
                Key::C => Some(Command::Align(Alignment::Centre)),
                Key::R => Some(Command::Align(Alignment::Right)),
                Key::T => Some(Command::Align(Alignment::Top)),
                Key::M => Some(Command::Align(Alignment::Middle)),
                Key::B => Some(Command::Align(Alignment::Bottom)),
                Key::H => Some(Command::Distribute(Axis::Horizontal)),
                Key::V => Some(Command::Distribute(Axis::Vertical)),
                _ => None,
            };
            if let Some(command) = command {
                return self.command(command);
            }
        }
        // P2-712 (4): the viewer bindings.
        let command = match key {
            // P2-713 as written: Up or Right opens the previous view, Down
            // or Left the next (Structurizr 6.2.4's diagrams.jsp maps
            // Left/Right the other way; the written sentence wins, see
            // docs/deferred.md). TODO(srs): P2-713, the ends; DECISION:
            // the list stops at its ends, as Structurizr's code does.
            Key::ArrowUp | Key::ArrowRight => Command::PreviousView,
            Key::ArrowDown | Key::ArrowLeft => Command::NextView,
            Key::Plus | Key::Minus => {
                let canvas = self.canvas.unwrap_or_default();
                let centre = ScreenPoint {
                    x: canvas.x / 2.0,
                    y: canvas.y / 2.0,
                };
                let factor = if key == Key::Plus { ZOOM_STEP } else { 1.0 / ZOOM_STEP };
                self.camera.zoom_at(centre, factor);
                self.remember();
                return Vec::new();
            }
            Key::W => Command::Fit(Fit::Width),
            Key::H => Command::Fit(Fit::Height),
            Key::C => Command::Fit(Fit::Content),
            Key::B => Command::Back,
            Key::D => Command::ToggleDescriptions,
            Key::M => Command::ToggleMetadata,
            Key::T => Command::ToggleTooltips,
            Key::G => Command::TagFilterPanel,
            Key::P => Command::PerspectivePanel,
            Key::I => Command::ToggleKey,
            Key::L => Command::Navigator,
            Key::N => Command::ToggleViewList,
            Key::F11 => Command::Fullscreen,
            Key::Alt => {
                // P2-724: Alt while a tooltip shows makes it sticky.
                if self.sticky.is_none()
                    && let Some(tip) = self.tooltip()
                {
                    self.sticky = Some(tip.item);
                }
                return Vec::new();
            }
            _ => return Vec::new(),
        };
        self.command(command)
    }

    /// A key while quick navigation is open (P2-712 (1), P2-715): Enter
    /// opens the highlighted view, Escape closes, Up and Down move the
    /// highlight; every other key is swallowed.
    fn quick_nav_key(&mut self, key: Key) -> Vec<Effect> {
        let count = self.quick_nav_entries().len();
        let Some(Overlay::QuickNavigation { highlighted, .. }) = self.overlay.as_mut() else {
            return Vec::new();
        };
        match key {
            Key::Escape => self.overlay = None,
            Key::ArrowDown if count > 0 => *highlighted = (*highlighted + 1).min(count - 1),
            Key::ArrowUp => *highlighted = highlighted.saturating_sub(1),
            Key::Enter => {
                let pick = *highlighted;
                let entry = self.quick_nav_entries().into_iter().nth(pick);
                self.overlay = None;
                if let Some(entry) = entry {
                    return self.show(entry.view);
                }
            }
            _ => {}
        }
        Vec::new()
    }

    /// The line or label under the pointer, as its winning relationship,
    /// when it can be written (P2-712 (2)).
    fn hovered_line(&self) -> Option<RelationshipRef> {
        let hover = self.hover?;
        if !matches!(hover, ItemKey::Line(_) | ItemKey::Label(_)) {
            return None;
        }
        let (scene, model) = (self.scene()?, self.scene_model()?);
        match scene.model_id(hover)? {
            ModelId::Relationship(r) => Some(RelationshipRef::of(model, r)),
            _ => None,
        }
    }

    /// Up, Down, `r` and `j` over a line (P2-708..P2-710); `None` when no
    /// line is under the pointer or the key is not one of these.
    // TODO(srs): P2-708, direction and step. DECISION: reading taken (B
    // editor-breadth 6): Up moves the label 5 position units toward the
    // source, Down 5 toward the destination, clamped to 0..100; an unset
    // position counts as 50 (Structurizr's default label position).
    // DECISION: an unset routing counts as Direct (Structurizr's default),
    // so the first `r` writes Orthogonal; an unset jump counts as off.
    fn line_key(&mut self, key: Key) -> Option<Vec<Effect>> {
        if !matches!(key, Key::ArrowUp | Key::ArrowDown | Key::R | Key::J) {
            return None;
        }
        let relationship = self.hovered_line()?;
        let style = self.hovered_style()?;
        let change = match key {
            Key::ArrowUp => LineChange::Position(Some(style.position.unwrap_or(50).saturating_sub(5))),
            Key::ArrowDown => LineChange::Position(Some(style.position.unwrap_or(50).saturating_add(5).min(100))),
            Key::R => LineChange::Routing(Some(match style.routing {
                Some(Routing::Orthogonal) => Routing::Curved,
                Some(Routing::Curved) => Routing::Direct,
                _ => Routing::Orthogonal,
            })),
            _ => LineChange::Jump(Some(!style.jump)),
        };
        let view = self.view.clone()?;
        Some(self.write(Action::SetLine {
            view,
            relationship,
            change,
        }))
    }

    /// The resolved style of the hovered line in the shown plan.
    fn hovered_style(&self) -> Option<c4rizer_core::model::ResolvedRelationshipStyle> {
        let (ItemKey::Line(edge) | ItemKey::Label(edge)) = self.hover? else {
            return None;
        };
        let (_, build) = self.shown.as_ref()?;
        build.plan.edges.get(edge.0 as usize).map(|e| e.style)
    }

    /// The items the tag filter or the perspective fades (P2-725,
    /// P2-726).
    // TODO(srs): P2-725, fade or hide, any or all; P2-726, what is drawn.
    // DECISION: reading taken (B editor-breadth 9): an item carrying none
    // of the chosen tags (any-of), or without the chosen perspective, is
    // faded (painted at 20% opacity), never hidden; the scene is not
    // rebuilt.
    fn faded(&self) -> Vec<ItemKey> {
        if self.aids.tags.is_empty() && self.aids.perspective.is_none() {
            return Vec::new();
        }
        let mut out = Vec::new();
        for (keys, tags, perspectives) in self.view_items_keyed() {
            let by_tag = !self.aids.tags.is_empty() && !tags.iter().any(|t| self.aids.tags.contains(t));
            let by_perspective = self.aids.perspective.as_ref().is_some_and(|p| !perspectives.contains(p));
            if by_tag || by_perspective {
                out.extend(keys);
            }
        }
        out
    }

    /// After a reload, drops a chosen tag or perspective the open view no
    /// longer has (P2-731).
    fn drop_stale_aids(&mut self) {
        if !self.aids.tags.is_empty() {
            let choices = self.tag_choices();
            self.aids.tags.retain(|t| choices.contains(t));
        }
        if let Some(p) = &self.aids.perspective
            && !self.perspective_choices().contains(p)
        {
            self.aids.perspective = None;
        }
    }

    /// The tooltip to show now (P2-723, P2-724): the sticky one, else,
    /// with tooltips on, the hovered box or line's.
    // TODO(srs): P2-723, the content. DECISION: reading taken (B
    // editor-breadth 9): name, type, description, technology, tags (the
    // EDIT-10 details), URL and properties.
    fn tooltip(&self) -> Option<Tooltip> {
        let scene = self.scene()?;
        let (item, sticky) = match self.sticky {
            Some(item) => (item, true),
            None if self.tooltips_on() => (self.hover?, false),
            None => return None,
        };
        if !matches!(item, ItemKey::Box(_) | ItemKey::Line(_) | ItemKey::Label(_)) || scene.item_bounds(item).is_none() {
            return None;
        }
        let model = self.scene_model()?;
        let subject = selected_of(scene, model, item)?;
        let details = self.details_of(subject)?;
        let (url, properties) = match scene.model_id(item)? {
            ModelId::Element(e) => {
                let e = model.element(e);
                (e.url.as_ref().map(|u| u.value.clone()), &e.properties)
            }
            ModelId::Relationship(r) => {
                let r = model.relationship(r);
                (r.url.as_ref().map(|u| u.value.clone()), &r.properties)
            }
            _ => return None,
        };
        Some(Tooltip {
            item,
            details,
            sticky,
            url,
            properties: properties.iter().map(|p| (p.name.value.clone(), p.value.value.clone())).collect(),
        })
    }

    /// A double click (P2-719, P2-720): with Alt, opens the URL of the
    /// box's element or the line's winning relationship.
    // TODO(srs): P2-719, the schemes. DECISION: reading taken (B
    // editor-breadth 10): only `http` and `https` URLs are opened; any
    // other URL, or none, leaves a status notice naming the item. The
    // URLs in `properties` and docs are not offered (no parity row).
    fn double_click(&mut self, at: ScreenPoint, mods: Modifiers) -> Vec<Effect> {
        // P3-312: a double click on a handle of the selected line removes
        // that vertex.
        if !mods.alt
            && !mods.shift
            && !mods.ctrl
            && let Some((edge, index)) = self.vertex_under(at)
        {
            // P3-420: refused with the JSON message on a workspace JSON.
            if self.refuse_json() {
                return Vec::new();
            }
            if !self.read_only() {
                return self.remove_vertex(edge, index);
            }
        }
        if !mods.alt {
            return self.edit_at(at, mods);
        }
        let (Some(scene), Some(model)) = (self.scene(), self.scene_model()) else { return Vec::new() };
        let Some(hit) = self.hit(at) else { return Vec::new() };
        let (name, url) = match (hit, scene.model_id(hit)) {
            (ItemKey::Box(_), Some(ModelId::Element(e))) => {
                let e = model.element(e);
                (e.name.value.clone(), e.url.as_ref().map(|u| u.value.clone()))
            }
            (ItemKey::Line(_) | ItemKey::Label(_), Some(ModelId::Relationship(r))) => {
                let rel = model.relationship(r);
                let name = format!(
                    "{} -> {}",
                    model.element(rel.source.value).name.value,
                    model.element(rel.destination.value).name.value
                );
                (name, rel.url.as_ref().map(|u| u.value.clone()))
            }
            _ => return Vec::new(),
        };
        self.open_url(&name, url)
    }

    /// Opens `url` of the item `name` when it is a web URL, else says
    /// why not in the status notice (P2-719, P2-720).
    fn open_url(&mut self, name: &str, url: Option<String>) -> Vec<Effect> {
        match url {
            Some(url) if is_web_url(&url) => vec![Effect::OpenUrl(url)],
            Some(url) => {
                self.notice = Some(format!("not opened: {url} ({name}) is not an http or https URL"));
                Vec::new()
            }
            None => {
                self.notice = Some(format!("nothing to open: {name} has no URL"));
                Vec::new()
            }
        }
    }

    /// A plain double click on an item with editable text edits that
    /// text in place, as Enter does (EDIT-5, EDIT-15 as amended by the
    /// owner in editor-ux-02: it no longer enters a level). The double
    /// click arrives after both clicks have selected the item.
    // DECISION: the item under the pointer becomes the selection first,
    // so the edit never depends on the two clicks having landed; Shift
    // or Ctrl held keeps the double click a selection gesture, as
    // EDIT-28 says for those modifiers.
    fn edit_at(&mut self, at: ScreenPoint, mods: Modifiers) -> Vec<Effect> {
        if mods.shift || mods.ctrl || self.text_edit.is_some() {
            return Vec::new();
        }
        let (Some(scene), Some(model)) = (self.scene(), self.model()) else { return Vec::new() };
        let Some(item) = self.hit(at).and_then(|h| selected_of(scene, model, h)) else { return Vec::new() };
        self.selection.clear();
        self.selection.insert(item);
        self.open_text_edit();
        Vec::new()
    }

    /// The views a box of `element` opens from the open view
    /// (`views_entered`).
    fn views_into(&self, model: &Model, element: ElementId) -> Vec<ViewRef> {
        views_entered(model, self.view.as_ref().and_then(|v| view_kind(model, v)), element)
    }

    /// Opens the context menu at `at` over `hit` (editor-ux-02): "Enter
    /// <view>" for each view a box opens, and "Open URL" for an item
    /// with a `url`. Nothing opens when the item has no entry.
    fn open_context_menu(&mut self, at: ScreenPoint, hit: Option<ItemKey>) {
        let (Some(scene), Some(model)) = (self.scene(), self.scene_model()) else { return };
        let Some(hit) = hit else { return };
        let mut entries = Vec::new();
        let name_url = match (hit, scene.model_id(hit)) {
            (ItemKey::Box(_), Some(ModelId::Element(e))) => {
                for view in self.views_into(model, e) {
                    let label = format!("Enter {}", self.entry_of(model, view.clone()).label);
                    entries.push(MenuEntry { label, action: MenuAction::Show(view) });
                }
                let el = model.element(e);
                el.url.as_ref().map(|u| (el.name.value.clone(), u.value.clone()))
            }
            (ItemKey::Line(_) | ItemKey::Label(_), Some(ModelId::Relationship(r))) => {
                // P4-217: "Release start" / "Release end" of a pinned line.
                if let ItemKey::Line(edge) = hit {
                    entries.extend(self.anchor_entries(edge));
                }
                let rel = model.relationship(r);
                let name = format!(
                    "{} -> {}",
                    model.element(rel.source.value).name.value,
                    model.element(rel.destination.value).name.value
                );
                rel.url.as_ref().map(|u| (name, u.value.clone()))
            }
            _ => None,
        };
        if let Some((name, url)) = name_url {
            entries.push(MenuEntry {
                label: format!("Open URL {url}"),
                action: MenuAction::OpenUrl { name, url },
            });
        }
        if !entries.is_empty() {
            self.menu_at = at;
            self.overlay = Some(Overlay::ContextMenu { entries });
        }
    }

    /// Opens the in-place editor on the single selected item (EDIT-5).
    fn open_text_edit(&mut self) {
        if self.selection.len() == 1 && self.refuse_json() {
            return;
        }
        if self.read_only() || self.selection.len() != 1 {
            return;
        }
        // P2-123: an unsupported view changes no file.
        if self.view.as_ref().is_some_and(|v| self.unsupported(v)) {
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

    /// Backspace: the parent crumb (EDIT-16 as amended by P4-114).
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
            Command::RemoveAllVertices => self.remove_all_vertices(),
            Command::SelectStep(step) => self.select_step(step),
            Command::MoveStep { step, up } => self.move_step(step, up),
            Command::Open(path) => {
                self.remember();
                vec![Effect::Read(path)]
            }
            Command::ShowOpenDialog => vec![Effect::Dialog(Dialog::Open {
                dir: self.doc.as_ref().map(|d| export_dir(&d.path)),
            })],
            Command::ContextAction(action) => {
                self.overlay = None;
                match action {
                    MenuAction::Show(view) => self.show(view),
                    MenuAction::OpenUrl { name, url } => self.open_url(&name, Some(url)),
                    MenuAction::ReleaseAnchor { edge, destination } => self.release_anchor(edge, destination),
                }
            }
            Command::ShowView(view) => {
                // P2-715, P2-729: picking a view in quick navigation or the
                // navigator closes it.
                if matches!(self.overlay, Some(Overlay::QuickNavigation { .. } | Overlay::Navigator)) {
                    self.overlay = None;
                }
                self.show(view)
            }
            Command::Crumb(n) => self.crumb_target(n).map_or_else(Vec::new, |v| self.show(v)),
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
                let to = align_to(&rects, how, self.first_selected_box());
                self.write_moves(&to, None)
            }
            Command::Distribute(axis) => {
                let rects = self.selected_rects();
                let to = distribute(&rects, axis);
                self.write_moves(&to, None)
            }
            Command::Undo => {
                if self.refuse_json() {
                    return Vec::new();
                }
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
                if self.refuse_json() {
                    return Vec::new();
                }
                if self.read_only() || self.pending.is_some() {
                    self.notice = Some("redo refused: the file is read-only or a change is being written".to_owned());
                    return Vec::new();
                }
                match self.history.pop_redo() {
                    Some(step) => self.write(Action::Redo(step)),
                    None => Vec::new(),
                }
            }
            // P3-431: a whole-workspace format (Ilograph, JSON, theme,
            // static site) writes a folder, so "Export view" asks for one.
            Command::ExportView(format) if format.whole_workspace() => {
                let Some(doc) = &self.doc else { return Vec::new() };
                vec![Effect::Dialog(Dialog::ExportAll {
                    dir: export_dir(&doc.path),
                    format,
                })]
            }
            Command::ExportView(format) => {
                let (Some(doc), Some(view)) = (&self.doc, &self.view) else { return Vec::new() };
                if let Some(kind) = self.model().and_then(|m| unsupported_kind(m, view)) {
                    self.notice = Some(format!(
                        "nothing to export: {} views are not drawn by this version",
                        kind.keyword()
                    ));
                    return Vec::new();
                }
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
                // P2-180: unsupported views are never exported.
                let targets: Vec<(ViewRef, PathBuf)> =
                    targets.into_iter().filter(|(v, _)| !is_unsupported(&parsed.model, v)).collect();
                if targets.is_empty() {
                    self.notice = Some("nothing to export: those views are not drawn by this version".to_owned());
                    return Vec::new();
                }
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
                        mode: self.mode,
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
            Command::Fit(how) => {
                if let (Some(scene), Some(canvas)) = (self.scene(), self.canvas)
                    && let Some(camera) = crate::camera::fit(scene.bounds, canvas, how)
                {
                    self.camera = camera;
                    self.fit_pending = false;
                    self.remember();
                }
                Vec::new()
            }
            Command::Navigator => {
                self.overlay = match self.overlay {
                    Some(Overlay::Navigator) => None,
                    _ => Some(Overlay::Navigator),
                };
                self.nav_query.clear();
                Vec::new()
            }
            Command::Fullscreen => {
                // P2-706: the view, camera and selection are untouched.
                self.aids.fullscreen = !self.aids.fullscreen;
                vec![Effect::Fullscreen(self.aids.fullscreen)]
            }
            Command::PreviousView | Command::NextView => {
                // TODO(srs): P2-713, the ends. DECISION: reading taken from
                // Structurizr 6.2.4 diagrams.jsp (`navigateToPreviousDiagram`
                // and `navigateToNextDiagram` stop at the ends); B
                // editor-breadth 9 proposed wrapping, Structurizr wins.
                let list = self.views();
                let Some(at) = list.iter().position(|e| e.current) else {
                    return list.first().map_or_else(Vec::new, |e| self.show(e.view.clone()));
                };
                let next = if command == Command::PreviousView {
                    at.checked_sub(1)
                } else {
                    Some(at + 1)
                };
                match next.and_then(|i| list.get(i)) {
                    Some(e) => {
                        let view = e.view.clone();
                        self.show(view)
                    }
                    None => Vec::new(),
                }
            }
            Command::Back => {
                // P2-714: the view shown before the current one, Structurizr's
                // `viewsVisited.pop()` then `peek()`; P4-114: the view left
                // is Forward's next target.
                if self.visited.len() < 2 {
                    return Vec::new();
                }
                self.ahead.extend(self.visited.pop());
                match self.visited.last().cloned() {
                    Some(view) => self.show(view),
                    None => Vec::new(),
                }
            }
            Command::QuickNavigation => {
                self.overlay = Some(Overlay::QuickNavigation {
                    query: String::new(),
                    highlighted: 0,
                });
                Vec::new()
            }
            // TODO(srs): P2-721, P2-722 with a style's `description false` or
            // `metadata false` (P2-10). DECISION: the toggles only hide; a
            // text the style already left out of the scene is never shown.
            // TODO(srs): P2-722, what metadata is. DECISION: reading taken
            // (B editor-breadth 9): the box type line and the label's
            // technology line.
            Command::ToggleDescriptions => {
                self.aids.descriptions = !self.aids.descriptions;
                Vec::new()
            }
            Command::ToggleMetadata => {
                self.aids.metadata = !self.aids.metadata;
                Vec::new()
            }
            Command::ToggleTooltips => {
                self.aids.tooltips = Some(!self.tooltips_on());
                self.sticky = None;
                Vec::new()
            }
            Command::TagFilterPanel => {
                self.overlay = match self.overlay {
                    Some(Overlay::TagFilter) => None,
                    _ => Some(Overlay::TagFilter),
                };
                Vec::new()
            }
            Command::TagFilter(mut tags) => {
                tags.sort();
                tags.dedup();
                self.aids.tags = tags;
                Vec::new()
            }
            Command::PerspectivePanel => {
                self.overlay = match self.overlay {
                    Some(Overlay::Perspectives) => None,
                    _ => Some(Overlay::Perspectives),
                };
                Vec::new()
            }
            Command::Perspective(p) => {
                self.aids.perspective = p;
                Vec::new()
            }
            Command::ToggleViewList => {
                self.aids.view_list = !self.aids.view_list;
                Vec::new()
            }
            // TODO(srs): P2-728, what "show" means. DECISION: reading taken
            // (B editor-breadth 7): `i` toggles the drawn key's visibility on
            // the canvas. P2-732's list of canvas-only differences does not
            // name a hidden key yet; the export always draws it.
            Command::ToggleKey => {
                self.aids.key = !self.aids.key;
                Vec::new()
            }
            Command::AutoPlace => self.auto_place(),
            Command::Canvas { step, keep_positions } => self.canvas_step(step, keep_positions),
            Command::BringOntoCanvas => self.bring_onto_canvas(),
            Command::Centre => self.centre(),
        }
    }

    /// The spec whose `at` lines a layout command in the open view
    /// writes: the declared view (a filtered view's base, P2-313), or the
    /// default view as `pipeline::default_view` builds it (EDIT-15).
    fn layout_spec(&self) -> Option<(ViewRef, c4rizer_core::model::ViewSpec)> {
        let (view, model) = (self.view.as_ref()?, self.model()?);
        if self.unsupported(view) {
            return None;
        }
        let layout = layout_view(model, view);
        let spec = match (&layout, model.view(&layout.key())) {
            (_, Some(spec)) => spec.clone(),
            (ViewRef::Default { scope, kind }, None) => default_view(model, model.find(scope)?, *kind)?,
            (ViewRef::Declared(_), None) => return None,
        };
        Some((view.clone(), spec))
    }

    /// "Auto-place view" (P2-700): every box of the open view placed by
    /// CORE-22 as though it had no `at` line and no previous scene
    /// (`size` lines kept), then one `Action::Move` writing every box's
    /// `at` line at its new centre; the default view goes first when it
    /// is not written yet (EDIT-15).
    // DECISION: placement only, on the UI thread, as `rebuild_first_write`
    // already does (EDIT-25: no routing here); the worker's build of the
    // written file routes the lines.
    fn auto_place(&mut self) -> Vec<Effect> {
        let Some((view, spec)) = self.layout_spec() else { return Vec::new() };
        let Some(model) = self.model() else { return Vec::new() };
        let mut plan = c4rizer_core::view::plan_in(model, &spec, self.mode);
        for b in &mut plan.boxes {
            b.centre = None;
        }
        let centres = c4rizer_core::place::place(&plan, &Keep::default());
        let boxes: BTreeMap<String, (i32, i32)> = plan
            .boxes
            .iter()
            .filter_map(|b| {
                let key = model.full_key(b.element)?;
                let c = centres.get(b.id.0 as usize)?;
                Some((key, (c.x * GRID_PX, c.y * GRID_PX)))
            })
            .collect();
        if boxes.is_empty() {
            return Vec::new();
        }
        let effects = self.write(Action::Move {
            view,
            layout: boxes.clone(),
            boxes,
        });
        // DECISION: the camera fits the re-placed drawing once its build is
        // shown (fix round): the new layout can sit anywhere, and a camera
        // kept from the old one may show nothing of it.
        if !effects.is_empty() {
            self.after_place = AfterPlace::Fit;
        }
        effects
    }

    /// The canvas of the open view, scene px: its `paperSize` from (0, 0),
    /// or the scene bounds on auto (P2-703, P2-704).
    // TODO(srs): P2-703, P2-704 on an auto canvas. DECISION: reading taken
    // (B editor-breadth 5d): the scene bounds are the canvas.
    fn canvas_rect(&self) -> Option<Rect> {
        let scene = self.scene()?;
        let (_, spec) = self.layout_spec()?;
        Some(match spec.paper_size {
            Some(p) => {
                let (w, h) = p.value.size_px();
                Rect::from_corners(Point::new(0, 0), Point::new(w, h))
            }
            None => scene.bounds,
        })
    }

    /// The paper of the open view, scene px from (0, 0): its `paperSize`;
    /// `None` on an auto canvas (P2-701). The shell paints it under the
    /// page so the canvas size is visible.
    pub(crate) fn paper_rect(&self) -> Option<Rect> {
        let (_, spec) = self.layout_spec()?;
        let (w, h) = spec.paper_size?.value.size_px();
        Some(Rect::from_corners(Point::new(0, 0), Point::new(w, h)))
    }

    /// Decrease, Auto or Increase canvas size (P2-701, P2-702): the
    /// previous, smallest-holding or next size of `paper_sizes`, written
    /// with every box moved by one grid offset that centres the scene on
    /// the new canvas, unless `keep_positions` (Alt); one undo step.
    // TODO(srs): P2-702, Auto. DECISION: reading taken (B editor-breadth
    // 5d; Structurizr 6.2.4 `autoPageSize` calls `centreDiagram`): Auto
    // always re-centres. From an auto canvas, Decrease and Increase step
    // from the size Auto would pick.
    // DECISION: the drawing centred is the whole scene (drawing, title
    // and key: `Scene::bounds`), the canvas from (0, 0).
    fn canvas_step(&mut self, step: CanvasStep, keep_positions: bool) -> Vec<Effect> {
        let Some((view, spec)) = self.layout_spec() else { return Vec::new() };
        let (Some(scene), Some(model)) = (self.scene(), self.scene_model()) else { return Vec::new() };
        let list = paper_sizes();
        let b = scene.bounds;
        let auto = list
            .iter()
            .position(|p| p.size_px().0 >= b.width() && p.size_px().1 >= b.height())
            .unwrap_or(list.len() - 1);
        let current = spec.paper_size.and_then(|p| list.iter().position(|&q| q == p.value)).unwrap_or(auto);
        let target = match step {
            CanvasStep::Auto => Some(auto),
            CanvasStep::Decrease => current.checked_sub(1),
            CanvasStep::Increase => Some(current + 1).filter(|&i| i < list.len()),
        };
        let Some(size) = target.map(|i| list[i]) else { return Vec::new() };
        let mut boxes = BTreeMap::new();
        let mut shift = (0, 0);
        if step == CanvasStep::Auto || !keep_positions {
            let (w, h) = size.size_px();
            let c = b.centre();
            let (dx, dy) = (snap_px_to_grid(w / 2 - c.x), snap_px_to_grid(h / 2 - c.y));
            if (dx, dy) != (0, 0) {
                shift = (dx * GRID_PX, dy * GRID_PX);
                for bx in &scene.boxes {
                    let Some(key) = model.full_key(bx.element) else { continue };
                    let at = scene.centres[bx.id.0 as usize];
                    boxes.insert(key, ((at.x + dx) * GRID_PX, (at.y + dy) * GRID_PX));
                }
            }
        }
        if spec.paper_size.map(|p| p.value) == Some(size) && boxes.is_empty() {
            return Vec::new();
        }
        let layout = self.layout();
        let effects = self.write(Action::SetCanvasSize {
            view,
            size: Some(size),
            boxes,
            layout,
        });
        // The camera follows the shared offset, so the drawing stays where
        // it was on screen and the page grows or shrinks around it.
        // DECISION: follow rather than refit (the zoom the user chose is
        // kept); found in the real-window check, where the re-centred
        // drawing jumped off screen. Undo moves it back like any write.
        #[expect(clippy::cast_precision_loss, reason = "scene px fit f32 exactly")]
        if shift != (0, 0) && !effects.is_empty() {
            self.camera.pan.x += shift.0 as f32;
            self.camera.pan.y += shift.1 as f32;
            self.remember();
        }
        effects
    }

    /// "Bring boxes onto the canvas" (P2-703): each box lying wholly or
    /// partly outside the canvas moves to the nearest grid centre that
    /// puts it inside; one undo step, nothing written when all are in.
    // TODO(srs): P2-703, "nearest". DECISION: reading taken (B
    // editor-breadth 5d): per axis, the box is clamped into the canvas
    // and its centre rounded inward to the 10px grid; a box larger than
    // the canvas keeps its top-left edge on the canvas's.
    fn bring_onto_canvas(&mut self) -> Vec<Effect> {
        let (Some(canvas), Some(scene)) = (self.canvas_rect(), self.scene()) else { return Vec::new() };
        let inward = |lo: i32, hi: i32, c: i32| -> i32 {
            if c < lo || hi < lo {
                (lo + GRID_PX - 1).div_euclid(GRID_PX)
            } else if c > hi {
                hi.div_euclid(GRID_PX)
            } else {
                c.div_euclid(GRID_PX)
            }
        };
        let mut to = BTreeMap::new();
        for b in &scene.boxes {
            let r = b.rect;
            if canvas.contains(r.min) && canvas.contains(r.max) {
                continue;
            }
            let (hw, hh) = (r.width() / 2, r.height() / 2);
            let c = r.centre();
            let x = inward(canvas.min.x + hw, canvas.max.x - hw, c.x);
            let y = inward(canvas.min.y + hh, canvas.max.y - hh, c.y);
            let at = scene.centres[b.id.0 as usize];
            // Keep an axis already inside exactly where it is.
            let x = if c.x - hw >= canvas.min.x && c.x + hw <= canvas.max.x { at.x } else { x };
            let y = if c.y - hh >= canvas.min.y && c.y + hh <= canvas.max.y { at.y } else { y };
            to.insert(b.id, Point::new(x, y));
        }
        self.write_moves(&to, None)
    }

    /// "Centre" (P2-704): the selected boxes, else every box, moved by
    /// one grid offset that puts the centre of their bounding rectangle
    /// on the canvas centre; one undo step.
    fn centre(&mut self) -> Vec<Effect> {
        let (Some(canvas), Some(scene)) = (self.canvas_rect(), self.scene()) else { return Vec::new() };
        let mut rects = self.selected_rects();
        if rects.is_empty() {
            rects = scene.boxes.iter().map(|b| (b.id, b.rect)).collect();
        }
        let Some(first) = rects.values().next().copied() else { return Vec::new() };
        let union = rects.values().fold(first, |a, r| {
            Rect::from_corners(
                Point::new(a.min.x.min(r.min.x), a.min.y.min(r.min.y)),
                Point::new(a.max.x.max(r.max.x), a.max.y.max(r.max.y)),
            )
        });
        let (c, u) = (canvas.centre(), union.centre());
        let (dx, dy) = (snap_px_to_grid(c.x - u.x), snap_px_to_grid(c.y - u.y));
        if (dx, dy) == (0, 0) {
            return Vec::new();
        }
        let to: BTreeMap<BoxId, Point> = rects
            .keys()
            .map(|&id| {
                let at = scene.centres[id.0 as usize];
                (id, Point::new(at.x + dx, at.y + dy))
            })
            .collect();
        self.write_moves(&to, None)
    }

    /// The box of the first selected element still selected and on
    /// screen (P2-707).
    fn first_selected_box(&self) -> Option<BoxId> {
        let (scene, model) = (self.scene()?, self.model()?);
        self.selection_order
            .iter()
            .filter(|s| self.selection.contains(s))
            .flat_map(|s| items_of(scene, model, s))
            .find_map(|k| match k {
                ItemKey::Box(id) => Some(id),
                _ => None,
            })
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
        if matches!(
            event,
            FileEvent::Written { .. } | FileEvent::ChangedOnDisk { .. } | FileEvent::WriteFailed { .. }
        ) && let Some(path) = self.in_flight.take()
            && self.doc.as_ref().is_some_and(|d| d.path != path)
        {
            return self.included_written(path, event);
        }
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
                self.queued.clear();
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
                self.queued.clear();
                self.after_place = AfterPlace::Nothing;
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
        // P4-116: another file starts a new view history.
        if self.doc.as_ref().is_none_or(|d| d.path != path) {
            self.visited.clear();
            self.ahead.clear();
        }
        self.included.clear();
        self.in_flight = None;
        self.queued.clear();
        let (parsed, parse_error) = match self.parse_doc(&path, text) {
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
        self.prune_history();
        self.ui.opened(&path);
        self.remember();
        let mut effects = vec![Effect::Watch(path), Effect::SaveUiState(self.ui.clone())];
        effects.extend(self.request_shown());
        effects
    }

    /// The file changed on disk (EDIT-1, EDIT-11, EDIT-23, EDIT-27).
    fn reload(&mut self, text: &str, hash: ContentHash) -> Vec<Effect> {
        let Some(path) = self.doc.as_ref().map(|d| d.path.clone()) else { return Vec::new() };
        let result = self.parse_doc(&path, text);
        let Some(doc) = self.doc.as_mut() else { return Vec::new() };
        doc.hash = hash;
        doc.missing = false;
        match result {
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
        let Some(path) = self.doc.as_ref().map(|d| d.path.clone()) else { return Vec::new() };
        let result = self.parse_doc(&path, &pending.text);
        let Some(doc) = self.doc.as_mut() else { return Vec::new() };
        doc.hash = hash;
        match result {
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

    /// The result of the in-flight write to the included file `path`
    /// (P2-139): on `Written` its own-write hash is kept (EDIT-22) and the
    /// next write of the batch goes, or the batch is done; a file changed
    /// on disk re-parses and re-applies the action (EDIT-24); a failure
    /// gives the action back (EDIT-27).
    fn included_written(&mut self, path: PathBuf, event: FileEvent) -> Vec<Effect> {
        match event {
            FileEvent::Written { hash } => {
                self.included.insert(path, hash);
                if let Some(next) = self.queued.pop_front() {
                    self.in_flight = Some(next.path.clone());
                    return vec![Effect::Write(next)];
                }
                let Some(hash) = self.doc.as_ref().map(|d| d.hash) else { return Vec::new() };
                self.written(hash)
            }
            FileEvent::ChangedOnDisk { .. } => {
                // DECISION: the files of the batch written before this one
                // keep their new text; the action is re-applied to the files
                // as they are now, like a root-file EDIT-24 conflict.
                self.queued.clear();
                let pending = self.pending.take();
                let (Some(text), Some(hash)) = (self.root_text.clone(), self.doc.as_ref().map(|d| d.hash)) else {
                    return Vec::new();
                };
                let mut effects = self.reload(&text, hash);
                let Some(pending) = pending else { return effects };
                let (Some(parsed), Some(doc)) = (self.parsed(), self.doc.as_ref()) else {
                    return effects;
                };
                if doc.parse_error.is_some() {
                    self.notice = Some("change dropped: an included file on disk does not parse".to_owned());
                    return effects;
                }
                let (path, expected) = (doc.path.clone(), doc.hash);
                effects.extend(self.write_to(pending.action, &parsed, path, expected));
                effects
            }
            FileEvent::WriteFailed { error } => {
                self.queued.clear();
                self.io_error = Some(error);
                if let Some(pending) = self.pending.take() {
                    self.give_back(pending.action);
                }
                self.preview = None;
                self.dragged = None;
                Vec::new()
            }
            _ => Vec::new(),
        }
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
                    // P2-731: a reload of the same view drops stale aids.
                    // DECISION: only a new parse of the view on screen drops
                    // them; a view switch keeps the filter and perspective
                    // for the session (B editor-breadth 10).
                    if let Some((old_parsed, old_build)) = &old
                        && !Arc::ptr_eq(old_parsed, &parsed)
                        && self.shown.as_ref().is_some_and(|(_, b)| b.scene.key == old_build.scene.key)
                    {
                        self.drop_stale_aids();
                    }
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
                    // DECISION: the item under a still pointer is hit again
                    // in the new scene, so a hovered line keeps taking its
                    // keys after its own write (P2-708..P2-710).
                    if std::mem::replace(&mut self.after_place, AfterPlace::Nothing) == AfterPlace::Fit {
                        self.fit_view();
                    }
                    self.hover = match (self.pointer, &self.gesture) {
                        (Some(p), Gesture::Idle) => self.hit(p),
                        _ => None,
                    };
                    return self.request_background();
                }
                // A background build from before a mode switch is dropped.
                if !shown && self.awaited_build.is_none_or(|g| generation > g) && build.scene.mode == self.mode {
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
                } else if matches!(self.gesture, Gesture::DraggingVertex { .. } | Gesture::DraggingEnd { .. }) {
                    self.preview = Some(build.scene.clone());
                    self.dragged = Some(build);
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
            .map(|id| {
                let mut items: Vec<ItemKey> = scene.boxes_of(id).into_iter().map(ItemKey::Box).collect();
                // P3-131: a selected deployment node is its boundary.
                items.extend(
                    scene
                        .boundaries
                        .iter()
                        .enumerate()
                        .filter(|(_, b)| b.of == BoundaryOf::DeploymentNode(id))
                        .map(|(i, _)| ItemKey::Boundary(u32::try_from(i).unwrap_or(u32::MAX))),
                );
                items
            })
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
        Selected::Step(step) => steps::step_items(scene, model, *step),
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
    if let Some(step) = steps::step_selected(scene, model, item) {
        return Some(step);
    }
    match item {
        ItemKey::Box(_) | ItemKey::Line(_) | ItemKey::Label(_) => match scene.model_id(item)? {
            ModelId::Element(e) => model.element(e).key.as_ref().map(|k| Selected::Element(k.value.clone())),
            ModelId::Relationship(r) => Some(Selected::Relationship(RelationshipRef::of(model, r))),
            _ => None,
        },
        ItemKey::Title | ItemKey::Subtitle => Some(Selected::Title),
        // P3-131: a click on a deployment node's boundary selects the node.
        ItemKey::Boundary(i) => match scene.boundaries.get(i as usize)?.of {
            BoundaryOf::DeploymentNode(e) => model.element(e).key.as_ref().map(|k| Selected::Element(k.value.clone())),
            _ => None,
        },
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
/// The kind of `view` when it is a declared view that is not drawn yet
/// (P2-2, P2-123).
fn unsupported_kind(model: &Model, view: &ViewRef) -> Option<ViewKind> {
    match view {
        ViewRef::Declared(key) => model.view(key).map(|v| v.kind).filter(|k| !k.drawable()),
        ViewRef::Default { .. } => None,
    }
}

/// True when `view` is a declared view that is not drawn yet.
fn is_unsupported(model: &Model, view: &ViewRef) -> bool {
    unsupported_kind(model, view).is_some()
}

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

/// The view whose `at` and `size` lines a move or resize in `view`
/// writes: the base view for a filtered view (P2-9 shares its lines),
/// else `view` itself.
// TODO(srs): P2-313, a move or resize in a filtered view. DECISION:
// reading taken (B landscape-filtered 1): it writes the base view's line,
// in one undo step, as the layout is shared.
fn layout_view(model: &Model, view: &ViewRef) -> ViewRef {
    match view {
        ViewRef::Declared(key) => model
            .view(key)
            .and_then(|v| model.base_view(v))
            .map_or_else(|| view.clone(), |b| ViewRef::Declared(b.key.clone())),
        ViewRef::Default { .. } => view.clone(),
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
/// What the next shown build of the open view does after Auto-place
/// (P2-700).
// DECISION: an enum, not a fifth bool on `State` (clippy's
// struct_excessive_bools).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum AfterPlace {
    /// Nothing pending.
    Nothing,
    /// Fit the camera to the re-placed drawing.
    Fit,
}

/// The C4 level of `view` (0 landscape, 1 context, 2 container, 3
/// component) and its scope; a filtered view takes its base view's; none
/// for custom and image views (P2-729).
fn c4_place(model: &Model, view: &ViewRef) -> Option<(u8, Option<ElementId>)> {
    let kind = view_kind(model, view)?;
    let rank = match kind {
        ViewKind::SystemLandscape => 0,
        ViewKind::SystemContext => 1,
        ViewKind::Container => 2,
        ViewKind::Component => 3,
        ViewKind::Filtered => {
            let spec = model.view(&view.key())?;
            let base = model.base_view(spec).filter(|b| b.kind != ViewKind::Filtered)?;
            return c4_place(model, &ViewRef::Declared(base.key.clone()));
        }
        _ => return None,
    };
    Some((rank, view_scope(model, view)))
}

/// True when a view of level `rank` scoped to `upper` shows the element
/// `inner` (or every element, for a landscape) (P2-729).
fn scope_contains(model: &Model, rank: u8, upper: Option<ElementId>, inner: Option<ElementId>) -> bool {
    if rank == 0 {
        return true;
    }
    let (Some(upper), Some(mut at)) = (upper, inner) else {
        return false;
    };
    loop {
        if at == upper {
            return true;
        }
        match model.element(at).parent {
            Some(p) => at = p,
            None => return false,
        }
    }
}

/// The P3-420 notice for an edit of a workspace JSON.
const JSON_REFUSED: &str =
    "change refused: the file is a workspace JSON, which the editor shows and exports but never writes — edit the DSL file it was exported from";

/// The P3-420 notice for an edit that would rewrite a `workspace extends
/// <file>.json` base.
const JSON_BASE_REFUSED: &str =
    "change refused: it would rewrite the workspace JSON this file extends, which the editor never writes — move the view into the DSL file";

/// True when `text` is a Structurizr workspace JSON: its first
/// non-whitespace byte is `{` (P3-416).
fn is_json_text(text: &str) -> bool {
    text.trim_start().starts_with('{')
}

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
        fullscreen: Vec<bool>,
        urls: Vec<String>,
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
                fullscreen: Vec::new(),
                urls: Vec::new(),
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
                    Effect::Fullscreen(on) => {
                        self.fullscreen.push(on);
                        Vec::new()
                    }
                    Effect::OpenUrl(url) => {
                        self.urls.push(url);
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

        /// Enters `key`'s box the way the owner's ruling says
        /// (editor-ux-02): a right click opens the context menu and its
        /// first entry is picked.
        fn enter(&mut self, key: &str) {
            let at = self.screen(key);
            self.click_at(at, Button::Right, NONE);
            let Some(super::Overlay::ContextMenu { entries }) = self.s.overlay.clone() else {
                panic!("no context menu on {key}");
            };
            self.cmd(Command::ContextAction(entries[0].action.clone()));
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

    /// EDIT-15 (plan editor-03, as amended by editor-ux-02): the context
    /// menu of an enterable system opens its container view; a single click on any
    /// box selects it; hover marks enterable boxes.
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

        // EDIT-15 as amended: a single click selects, the context menu
        // enters.
        h.click("shop", Button::Left, NONE);
        assert_eq!(h.s.view, Some(declared("context")));
        assert_eq!(h.selection(), vec![el("shop")]);
        h.enter("shop");
        assert_eq!(h.s.view, Some(declared("containers")));
        assert_eq!(h.scene().key, "containers");
    }

    /// Polish (merge review): entering a view, by the context menu or by
    /// picking it in the view list, starts it with nothing selected, so
    /// the details panel never shows an item of the previous view.
    #[test]
    fn view_switch_clears_selection() {
        let mut h = H::with_file(SHOP);
        h.enter("shop");
        assert_eq!(h.s.view, Some(declared("containers")));
        assert_eq!(h.selection(), Vec::<Selected>::new(), "context menu into a level");
        assert!(h.s.details().is_none());

        h.cmd(Command::ShowView(declared("context")));
        h.click("bank", Button::Left, NONE);
        assert_eq!(h.selection(), vec![el("bank")]);
        h.cmd(Command::ShowView(declared("containers")));
        assert_eq!(h.selection(), Vec::<Selected>::new(), "picked in the view list");
        assert!(h.s.details().is_none());

        // Re-picking the open view is no switch: the selection stays.
        h.cmd(Command::ShowView(declared("context")));
        h.click("bank", Button::Left, NONE);
        h.cmd(Command::ShowView(declared("context")));
        assert_eq!(h.selection(), vec![el("bank")]);
    }

    /// EDIT-15 (plan editor-03): with no view scoped to the box, its
    /// default view opens; the first write in it starts with
    /// `AddDefaultView`; undoing that write removes it again.
    #[test]
    fn edit_15_default_view_when_none() {
        let mut h = H::with_file(SHOP);
        h.cmd(Command::ShowView(declared("containers")));
        h.enter("web");
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

    /// EDIT-15: with several container views scoped to a system, a double
    /// click opens the first declared one and the view list shows the others;
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
        h.enter("shop");
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
        h.enter("web");
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
        h.enter("web");
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
    /// model; Backspace opens the parent crumb (Alt+Left too until P4-114
    /// made it Back); Backspace is ignored while a text field has focus.
    #[test]
    fn edit_16_back_goes_up() {
        let mut h = H::with_file(SHOP);
        let crumbs = |h: &H| -> Vec<Option<String>> { h.s.breadcrumb().into_iter().map(|c| c.scope).collect() };
        assert_eq!(crumbs(&h), [None, Some("shop".to_owned())]);
        assert_eq!(h.s.breadcrumb()[0].label, "Shop");
        h.enter("shop");
        h.enter("web");
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
        h.key(Key::Backspace, NONE);
        assert_eq!(h.s.view, Some(declared("context")));
        h.key(Key::Backspace, NONE);
        assert_eq!(h.s.view, Some(declared("context")), "the top stays");
        // P2-713 (editor-breadth fix round): plain Right is "previous",
        // which stops at the first view.
        h.key(Key::ArrowRight, NONE);
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
                // editor-breadth: the new keys and inputs (P2-733).
                step(&mut h, Event::Command(Command::ShowView(view.clone())));
                let at = h.screen(key);
                step(&mut h, Event::Input(Input::PointerMove { at }));
                step(&mut h, Event::Input(Input::DoubleClick { at, mods: ALT }));
                step(&mut h, Event::Input(Input::CanvasSize { size: pt(900.0, 700.0) }));
                for k in [
                    Key::ArrowUp, Key::ArrowRight, Key::ArrowDown, Key::Plus, Key::Minus, Key::B, Key::C, Key::D,
                    Key::G, Key::H, Key::I, Key::J, Key::L, Key::M, Key::N, Key::P, Key::R, Key::T, Key::V, Key::W,
                    Key::F11, Key::Alt,
                ] {
                    for m in [NONE, ALT] {
                        step(&mut h, Event::Command(Command::ShowView(view.clone())));
                        step(&mut h, Event::Input(Input::PointerMove { at }));
                        step(&mut h, Event::Input(Input::KeyDown { key: k, mods: m, text_focus: false }));
                        step(&mut h, Event::Input(Input::KeyUp { key: k }));
                        step(&mut h, Event::Input(Input::KeyDown { key: Key::Escape, mods: NONE, text_focus: false }));
                    }
                }
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
                Command::AutoPlace,
                Command::Canvas { step: super::CanvasStep::Auto, keep_positions: false },
                Command::Canvas { step: super::CanvasStep::Increase, keep_positions: false },
                Command::Canvas { step: super::CanvasStep::Decrease, keep_positions: true },
                Command::BringOntoCanvas,
                Command::Centre,
                Command::Fit(crate::camera::Fit::Width),
                Command::Fullscreen,
                Command::PreviousView,
                Command::NextView,
                Command::Back,
                Command::QuickNavigation,
                Command::ToggleDescriptions,
                Command::ToggleMetadata,
                Command::ToggleTooltips,
                Command::TagFilterPanel,
                Command::TagFilter(vec!["Database".to_owned()]),
                Command::PerspectivePanel,
                Command::Perspective(None),
                Command::ToggleViewList,
                Command::ToggleKey,
                Command::Navigator,
                Command::Undo,
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
    /// is a click (selects), 4 px or more is a drag; Shift-click
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
        assert_eq!(h.s.view, Some(declared("context")), "a click selects, never enters (EDIT-15 as amended)");
        assert_eq!(h.selection(), vec![el("shop")], "still a click");

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
        // Amended by P2-738: 90% of the scene plus the margins, so the
        // fit stays above the 9px text floor.
        let mut h = H::with_file(SHOP);
        let b = h.scene().bounds;
        #[expect(clippy::cast_precision_loss, reason = "scene px fit f32 exactly")]
        let small = pt(b.width() as f32 * 0.9 + 40.0, b.height() as f32 * 0.9 + 40.0);
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

        h.enter("shop");
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

//! Pipeline thread: takes open, file-changed, drag and release requests
//! with a generation number, drops stale ones, sends scenes back. The UI
//! thread never routes (EDIT-25).
//!
//! Depends on: `c4rizer-core` (pipeline, export, geom, model, place,
//! view).

// Shape: plain data in, plain data out. `serve` runs one job
// synchronously and is what the thread calls; `latest` is the
// stale-dropping rule, a pure function over the queued requests, so both
// are tested without a thread. `Worker` only owns the channels and the
// thread. `state` builds the requests and checks the generation of every
// reply; the worker never looks at the editor's state.
//
// DECISION: one worker thread, not a pool: one view is routed at a time
// (CORE-28), drag previews are cheap (`pipeline::PREVIEW_ROUTE`), and a
// second thread would only race the first for the same generation.
// DECISION: the parse and the previous build travel as `Arc`, so a drag
// event costs a pointer copy on the UI thread, never a deep clone
// (EDIT-25: no UI update waits).
// DECISION: a waker closure (`Wake`) is called after each reply so the
// eframe shell can `request_repaint`; the worker never names an egui type.
//
// Cooperative stop (EDIT-25, owner's ruling): each job the thread runs
// gets a fresh stop flag (`Arc<AtomicBool>`), published with the job in
// `Worker::running`. `Worker::send` sets that flag when a newer shown
// `Build` or a `Drag` arrives for a running job that `latest` would have
// dropped (`stops`): a running `Build` (shown or background) on a newer
// shown `Build`; a running `Drag` on a newer shown `Build` or `Drag`; a
// running background `Build` on a `Drag`. The pipeline then returns
// `Stopped` within one rip-up round or edge order, `serve` returns `None`
// (no reply: the request that stopped it brings its own), and the thread
// takes the newer request without waiting for the stale build to finish.
// An `Export` is never stopped: it is never dropped by `latest`, so its
// flag is never set.

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::atomic::AtomicBool;
use std::sync::mpsc::{Receiver, Sender};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;

use c4rizer_core::geom::Point;
use c4rizer_core::model::{Parsed, ViewKind};
use c4rizer_core::pipeline::ViewBuild;
use c4rizer_core::place::Keep;
use c4rizer_core::view::BoxId;

/// Monotonic request number. A reply whose generation is older than the
/// newest request of its kind is stale and dropped, by the worker before
/// it runs (`latest`) and by `state` when it arrives (EDIT-25).
// DECISION: repr(transparent) and pinned: it crosses the channel on every
// drag event, the editor's hot path (EDIT-25).
#[repr(transparent)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Default)]
pub struct Generation(pub u64);

impl Generation {
    /// The next generation (`self.0 + 1`, saturating). Serves EDIT-25.
    #[must_use]
    pub fn next(self) -> Generation {
        todo!()
    }
}

/// Which view to build: one declared in the file, by its key, or an
/// EDIT-15 default view not yet written, by its scope's element key and
/// kind (`pipeline::default_view`). Keys, not ids, so the value survives
/// a re-parse (EDIT-1). Serves EDIT-8, EDIT-15.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum ViewRef {
    /// `Model::views` entry with this effective key (`ViewSpec::key`).
    Declared(String),
    /// The default view of `scope` (EDIT-15); once written by
    /// `edit::Edit::AddDefaultView` it is `Declared("<scope>-<kind>")`.
    Default {
        /// The scope's canonical element key (`Element::key`).
        scope: String,
        /// `Container` (of a system), `Component` (of a container) or
        /// `SystemContext` (of a system, model-only files).
        kind: ViewKind,
    },
}

impl ViewRef {
    /// The key the view has, or will have once written: the declared key,
    /// or `<scope>-<kind>` with `ViewKind::keyword` (EDIT-15). Names the
    /// export file (EDIT-7). Serves EDIT-7, EDIT-15.
    #[must_use]
    pub fn key(&self) -> String {
        todo!()
    }
}

/// Export file format (EDIT-7).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum ExportFormat {
    /// `export::svg`.
    Svg,
    /// `export::png` at `Job::Export::png_width` (EXP-2), with the
    /// bundled font (`scene::FONT_REGULAR`, `scene::FONT_BOLD`); the
    /// editor's `c4rizer-core` dependency enables feature `png`.
    Png,
}

impl ExportFormat {
    /// `svg` or `png`, the file extension (EDIT-7).
    #[must_use]
    pub const fn extension(self) -> &'static str {
        todo!()
    }
}

/// One view to export: which view and where to write it (EDIT-7).
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct ExportTarget {
    /// The view.
    pub view: ViewRef,
    /// The keep the view is built with, so the export equals the canvas
    /// (EDIT-12, CORE-29).
    pub keep: Keep,
    /// The file to write, chosen in the save dialog.
    pub path: PathBuf,
}

/// The work of one request. Serves EDIT-1, EDIT-2, EDIT-7, EDIT-14,
/// EDIT-25.
#[derive(Debug, Clone)]
pub enum Job {
    /// A full build (`pipeline::build_view` or, for a default view,
    /// `pipeline::default_view` + `pipeline::build_spec`), routed with
    /// `RouteConfig::DEFAULT`: open, reload, release (EDIT-1, EDIT-3,
    /// EDIT-25). `shown: false` marks a background build of another view
    /// (EDIT-14): it runs only when no shown build or drag is queued.
    Build {
        /// The parse to build from (`Parsed::model`).
        parsed: Arc<Parsed>,
        /// The view.
        view: ViewRef,
        /// Previous centres and dragged boxes (CORE-29, EDIT-4).
        keep: Keep,
        /// True for the view on screen, false for a background build.
        shown: bool,
    },
    /// A drag preview (`pipeline::rebuild_after_move` with
    /// `pipeline::PREVIEW_ROUTE`): only lines touching the moved boxes
    /// are re-routed (EDIT-2, EDIT-20, EDIT-25).
    Drag {
        /// The parse `previous` was built from.
        parsed: Arc<Parsed>,
        /// The last full build of the view.
        previous: Arc<ViewBuild>,
        /// Dragged boxes at their snapped centres, grid units.
        moved: BTreeMap<BoxId, Point>,
    },
    /// Write SVG or PNG files (EDIT-7); never dropped as stale and never
    /// stopped.
    Export {
        /// The parse to build from (`Parsed::model`).
        parsed: Arc<Parsed>,
        /// One entry per view to write ("Export view": one; "Export all
        /// views": every declared view).
        targets: Vec<ExportTarget>,
        /// SVG or PNG.
        format: ExportFormat,
        /// PNG width per target, `None` for twice the scene's width
        /// (EDIT-7 default).
        png_width: Option<u32>,
    },
}

/// A job with its generation. Serves EDIT-25.
#[derive(Debug, Clone)]
pub struct Request {
    /// The request's generation, from `state`.
    pub generation: Generation,
    /// The work.
    pub job: Job,
}

/// What the worker sends back. Serves EDIT-1, EDIT-6, EDIT-7, EDIT-25.
#[derive(Debug, Clone)]
pub enum Reply {
    /// A full build finished (scene, lint report, plan, routes).
    Built {
        /// The request's generation.
        generation: Generation,
        /// The view built.
        view: ViewRef,
        /// The build, shared with `state`'s cache.
        build: Arc<ViewBuild>,
        /// Copied from `Job::Build::shown`.
        shown: bool,
    },
    /// A drag preview finished; `state` shows it if no newer drag event
    /// came in since (the box itself already moved, EDIT-25).
    Dragged {
        /// The request's generation.
        generation: Generation,
        /// The preview build.
        build: Arc<ViewBuild>,
    },
    /// The view could not be built: unknown key, or a default view that
    /// no longer opens (`pipeline::default_view` gave `None`).
    NoView {
        /// The request's generation.
        generation: Generation,
        /// The view asked for.
        view: ViewRef,
    },
    /// An export finished: the files written, or the first I/O or PNG
    /// error as one status-bar sentence (SRS 5.1).
    Exported {
        /// The request's generation.
        generation: Generation,
        /// Paths written, in target order, or the error.
        result: Result<Vec<PathBuf>, String>,
    },
}

/// Called after every reply, from the worker thread; the shell passes a
/// closure that calls `egui::Context::request_repaint`, so a reply is
/// shown without waiting for input (EDIT-25).
pub type Wake = Arc<dyn Fn() + Send + Sync>;

/// The job the worker thread is running and its stop flag (EDIT-25).
#[derive(Debug, Clone)]
pub struct Running {
    /// The running job's generation.
    pub generation: Generation,
    /// The running job (a cheap clone: its parse and builds are `Arc`s).
    pub job: Job,
    /// Its stop flag, passed to the pipeline; set by `Worker::send` when
    /// `stops` says so.
    pub stop: Arc<AtomicBool>,
}

/// The worker thread and its two channels. Dropping it closes the
/// request channel and joins the thread. Serves EDIT-25.
pub struct Worker {
    /// Requests to the thread.
    pub requests: Sender<Request>,
    /// Replies from the thread.
    pub replies: Receiver<Reply>,
    /// The thread, joined on drop.
    pub thread: Option<JoinHandle<()>>,
    /// The job the thread is running, `None` while it idles; shared
    /// between the thread (which sets it around each `serve`) and `send`
    /// (which sets its stop flag, EDIT-25).
    pub running: Arc<Mutex<Option<Running>>>,
}

impl Worker {
    /// Starts the thread: it drains every queued request, keeps
    /// `latest` of them, runs them with `serve` in order, each with a
    /// fresh stop flag published in `running`, sends each reply (none for
    /// a stopped job) and calls `wake`. Serves EDIT-25.
    #[must_use]
    pub fn spawn(wake: Wake) -> Worker {
        todo!()
    }

    /// Queues `request`; never blocks (EDIT-25). When `request` is a
    /// shown `Build` or a `Drag` and `stops(running job, request)` holds,
    /// it first sets the running job's stop flag, so the thread serves
    /// `request` without waiting for the stale job to finish; an `Export`
    /// is never stopped. A closed thread is ignored: the status bar
    /// learns it from the missing replies.
    pub fn send(&self, request: Request) {
        todo!()
    }

    /// Every reply received since the last call, in arrival order; never
    /// blocks. Serves EDIT-25.
    #[must_use]
    pub fn drain(&self) -> Vec<Reply> {
        todo!()
    }
}

impl Drop for Worker {
    /// Closes the request channel and joins the thread.
    fn drop(&mut self) {
        todo!()
    }
}

/// The stale-dropping rule (EDIT-25), over the requests queued since the
/// worker last looked, in arrival order. Kept, in arrival order: every
/// `Export`; the newest shown `Build`; the newest `Drag` unless a shown
/// `Build` came after it (a release supersedes the drag); background
/// `Build`s with the newest generation of any shown `Build` or later,
/// one per view (the newest), and only when no shown `Build` or `Drag`
/// is kept (EDIT-14: they never delay the view on screen; the dropped
/// ones are re-sent by `state` when it next idles).
#[must_use]
pub fn latest(queued: Vec<Request>) -> Vec<Request> {
    todo!()
}

/// True when a newly arrived `incoming` job must stop the `running` one
/// (EDIT-25): `incoming` is a shown `Build` or a `Drag`, and
/// `latest(vec![running, incoming])` would drop `running`. So a running
/// `Build` stops on a shown `Build`; a running `Drag` on a shown `Build`
/// or a `Drag`; a running background `Build` on a `Drag`. A running
/// shown `Build` keeps going on a `Drag`, and a running `Export` is never
/// stopped. Serves EDIT-25, EDIT-14, EDIT-7.
#[must_use]
pub fn stops(running: &Job, incoming: &Job) -> bool {
    todo!()
}

/// Runs one request synchronously and returns its reply: the thread's
/// body, and how tests drive the worker without a thread. `stop` goes to
/// the pipeline's build functions; `None` when they returned `Stopped`
/// (no reply is sent). An `Export` ignores `stop` and always replies. A
/// run whose flag is never set gives the same reply as before the flag
/// existed; deterministic for `Build` and `Drag` (CORE-25). Serves
/// EDIT-1, EDIT-2, EDIT-7, EDIT-14, EDIT-25.
#[must_use]
pub fn serve(request: Request, stop: &AtomicBool) -> Option<Reply> {
    todo!()
}

#[cfg(test)]
mod tests {
    /// Size pin for the hot `Generation` (EDIT-25).
    #[test]
    fn generation_is_8_bytes() {
        assert_eq!(std::mem::size_of::<super::Generation>(), 8);
    }

    /// EDIT-25 (plan editor-02): of three queued drags and a shown build
    /// after them, `latest` keeps only the build; of three drags alone,
    /// only the newest.
    #[test]
    fn edit_25_stale_requests_dropped() {
        todo!()
    }

    /// EDIT-14: background builds are dropped while a shown build or a
    /// drag is queued, and never run before it.
    #[test]
    fn edit_14_background_builds_never_delay_shown_view() {
        todo!()
    }

    /// EDIT-25: `serve` of a drag job re-routes only lines touching the
    /// moved box (pipeline's `rebuild_after_move`) and keeps the
    /// generation.
    #[test]
    fn edit_25_drag_reply_keeps_generation() {
        todo!()
    }

    /// EDIT-7: an export job is never dropped by `latest`, whatever
    /// follows it.
    #[test]
    fn edit_7_export_never_dropped() {
        todo!()
    }

    /// EDIT-25: with a background build of a heavy view running on the
    /// thread, a drag request sent after it is served (its `Dragged`
    /// reply arrives) without waiting for that build to finish: `send`
    /// sets the build's stop flag, the build gives no reply.
    #[test]
    fn edit_25_drag_served_without_waiting_for_background_build() {
        todo!()
    }

    /// EDIT-7, EDIT-25: an `Export` is never stopped: `stops` is false for
    /// a running export whatever arrives, and `serve` of an export with a
    /// set flag still writes its files and replies `Exported`.
    #[test]
    fn edit_7_export_never_stopped() {
        todo!()
    }

    /// EDIT-15: `serve` builds a default view through
    /// `pipeline::default_view`, and replies `NoView` when it no longer
    /// opens.
    #[test]
    fn edit_15_default_view_built_or_no_view() {
        todo!()
    }
}

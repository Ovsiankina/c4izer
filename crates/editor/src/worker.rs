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
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;

use c4rizer_core::geom::Point;
use c4rizer_core::model::{Mode, Model, Parsed, ViewKind};

use crate::export_files;
use c4rizer_core::pipeline::{
    BuildError, PREVIEW_ROUTE, ViewBuild, build_spec_in, build_view_in, default_view, rebuild_after_move,
    rebuild_with_anchors, rebuild_with_vertices,
};
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
        Generation(self.0.saturating_add(1))
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
        match self {
            ViewRef::Declared(key) => key.clone(),
            ViewRef::Default { scope, kind } => format!("{scope}-{}", kind.keyword()),
        }
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
    /// `export::svg_embedded`: SVG with the bundled faces embedded
    /// (P2-663, `--svg-text embed`).
    // APPROVER: new pub item beyond A.2 (P2-663 moved here by O14; the
    // smallest additive form is two more formats, same `.svg` extension).
    SvgEmbedded,
    /// `export::svg_outlined`: SVG with text as paths (P2-663,
    /// `--svg-text outline`).
    // APPROVER: new pub item beyond A.2 (as `SvgEmbedded`).
    SvgOutlined,
    /// `export::text::plantuml` with `Flavour::Structurizr`: one
    /// `structurizr-<key>.puml` and its `-key.puml` legend per view
    /// (P3-406, P3-431).
    // APPROVER: approved 2026-11-18
    PlantUml,
    /// `export::text::plantuml` with `Flavour::C4PlantUml` (P3-407).
    // APPROVER: approved 2026-11-18
    C4PlantUml,
    /// `export::text::mermaid` (P3-408).
    // APPROVER: approved 2026-11-18
    Mermaid,
    /// `export::text::websequencediagrams`, dynamic views only (P3-409).
    // APPROVER: approved 2026-11-18
    WebSequenceDiagrams,
    /// `export::text::dot` without positions (P3-410).
    // APPROVER: approved 2026-11-18
    Dot,
    /// `export::text::ilograph`, one file for the workspace (P3-412).
    // APPROVER: approved 2026-11-18
    Ilograph,
    /// `export::json`, `<file stem>.json` (P3-413).
    // APPROVER: approved 2026-11-18
    Json,
    /// `export::theme`, `workspace-theme.json` (P3-421).
    // APPROVER: approved 2026-11-18
    Theme,
    /// `export::static_site` (P3-423).
    // APPROVER: approved 2026-11-18
    Static,
}

impl ExportFormat {
    /// The file extension (EDIT-7, P3-404): `svg`, `png`, `puml`, `mmd`,
    /// `wsd`, `dot`, `yaml` (Ilograph), `json` (JSON and theme), `html`
    /// (static site).
    #[must_use]
    pub const fn extension(self) -> &'static str {
        match self {
            ExportFormat::Svg | ExportFormat::SvgEmbedded | ExportFormat::SvgOutlined => "svg",
            ExportFormat::Png => "png",
            ExportFormat::PlantUml | ExportFormat::C4PlantUml => "puml",
            ExportFormat::Mermaid => "mmd",
            ExportFormat::WebSequenceDiagrams => "wsd",
            ExportFormat::Dot => "dot",
            ExportFormat::Ilograph => "yaml",
            ExportFormat::Json | ExportFormat::Theme => "json",
            ExportFormat::Static => "html",
        }
    }

    /// True for a Structurizr text format written one file per view,
    /// named `structurizr-<key>.<ext>` (P3-404).
    pub(crate) const fn per_view_text(self) -> bool {
        matches!(
            self,
            ExportFormat::PlantUml
                | ExportFormat::C4PlantUml
                | ExportFormat::Mermaid
                | ExportFormat::WebSequenceDiagrams
                | ExportFormat::Dot
        )
    }

    /// True for a format written for the whole workspace into a folder
    /// (Ilograph, JSON, theme, static site), whatever view is open.
    pub(crate) const fn whole_workspace(self) -> bool {
        matches!(self, ExportFormat::Ilograph | ExportFormat::Json | ExportFormat::Theme | ExportFormat::Static)
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
        /// Light or dark: the styles and defaults it is built with
        /// (P2-433).
        mode: Mode,
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
    /// A vertex drag preview (`pipeline::rebuild_with_vertices` with
    /// `pipeline::PREVIEW_ROUTE`), dropped and stopped like `Drag`
    /// (P3-311, EDIT-25).
    // APPROVER: new pub item
    DragVertex {
        /// The parse `previous` was built from.
        parsed: Arc<Parsed>,
        /// The last full build of the view.
        previous: Arc<ViewBuild>,
        /// The line.
        edge: c4rizer_core::view::EdgeId,
        /// Its vertices, grid units.
        vertices: Vec<Point>,
    },
    /// A line-end drag preview (`pipeline::rebuild_with_anchors` with
    /// `pipeline::PREVIEW_ROUTE`), dropped and stopped like `Drag`
    /// (P4-214, EDIT-25).
    // APPROVER: new pub item
    DragAnchor {
        /// The parse `previous` was built from.
        parsed: Arc<Parsed>,
        /// The last full build of the view.
        previous: Arc<ViewBuild>,
        /// The line.
        edge: c4rizer_core::view::EdgeId,
        /// Its source end's anchor.
        source: Option<c4rizer_core::anchor::Anchor>,
        /// Its destination end's anchor.
        destination: Option<c4rizer_core::anchor::Anchor>,
    },
    /// A label drag preview (`label_preview`): the labels placed again
    /// with the line's `position` overridden, no line re-routed; dropped
    /// and stopped like `Drag` (P2-708 by drag, EDIT-25).
    // APPROVER: new pub item
    DragLabel {
        /// The last full build of the view.
        previous: Arc<ViewBuild>,
        /// The line.
        edge: c4rizer_core::view::EdgeId,
        /// Its label's position, 0..100.
        position: u8,
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
        /// The mode shown on the canvas, so the files equal it (P2-433).
        mode: Mode,
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
        // DECISION: exports are never stale (EDIT-7), so `state` reads
        // only the result; the generation stays for symmetry and tests.
        #[allow(dead_code, reason = "exports are never dropped as stale")]
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
    // DECISION: `stops` decides on the jobs alone; the generation is
    // published for a debugger and kept for later stop rules.
    #[allow(dead_code, reason = "published for inspection; `stops` reads the jobs")]
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
        let (requests, inbox) = mpsc::channel::<Request>();
        let (outbox, replies) = mpsc::channel::<Reply>();
        let running: Arc<Mutex<Option<Running>>> = Arc::new(Mutex::new(None));
        let shared = Arc::clone(&running);
        let thread = std::thread::Builder::new()
            .name("c4rizer-worker".to_owned())
            .spawn(move || run_thread(&inbox, &outbox, &shared, &wake))
            .ok();
        Worker {
            requests,
            replies,
            thread,
            running,
        }
    }

    /// Queues `request`; never blocks (EDIT-25). When `request` is a
    /// shown `Build` or a `Drag` and `stops(running job, request)` holds,
    /// it first sets the running job's stop flag, so the thread serves
    /// `request` without waiting for the stale job to finish; an `Export`
    /// is never stopped. A closed thread is ignored: the status bar
    /// learns it from the missing replies.
    pub fn send(&self, request: Request) {
        if let Ok(running) = self.running.lock()
            && let Some(running) = running.as_ref()
            && stops(&running.job, &request.job)
        {
            running.stop.store(true, Ordering::Relaxed);
        }
        let _ = self.requests.send(request);
    }

    /// Every reply received since the last call, in arrival order; never
    /// blocks. Serves EDIT-25.
    #[must_use]
    pub fn drain(&self) -> Vec<Reply> {
        self.replies.try_iter().collect()
    }
}

impl Drop for Worker {
    /// Closes the request channel and joins the thread.
    fn drop(&mut self) {
        // Closing the channel ends the thread's loop; stopping the running
        // job makes that quick.
        let (closed, _) = mpsc::channel();
        drop(std::mem::replace(&mut self.requests, closed));
        if let Ok(running) = self.running.lock()
            && let Some(running) = running.as_ref()
            && !matches!(running.job, Job::Export { .. })
        {
            running.stop.store(true, Ordering::Relaxed);
        }
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
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
    let last_shown = queued
        .iter()
        .rposition(|r| matches!(r.job, Job::Build { shown: true, .. }));
    // DECISION: a vertex drag counts as a drag (P3-311): one preview at a
    // time, the newest of either kind.
    let last_drag = queued
        .iter()
        .rposition(|r| matches!(r.job, Job::Drag { .. } | Job::DragVertex { .. } | Job::DragAnchor { .. } | Job::DragLabel { .. }));
    let keep_drag = match (last_drag, last_shown) {
        (Some(d), Some(b)) => (d > b).then_some(d),
        (d, _) => d,
    };
    let foreground = last_shown.is_some() || keep_drag.is_some();
    let newest_shown = queued
        .iter()
        .filter(|r| matches!(r.job, Job::Build { shown: true, .. }))
        .map(|r| r.generation)
        .max();
    // The newest background build per view, at or after the newest shown
    // build's generation.
    let mut background: BTreeMap<ViewRef, usize> = BTreeMap::new();
    if !foreground {
        for (i, r) in queued.iter().enumerate() {
            if let Job::Build { view, shown: false, .. } = &r.job
                && newest_shown.is_none_or(|g| r.generation >= g)
            {
                background.insert(view.clone(), i);
            }
        }
    }
    queued
        .into_iter()
        .enumerate()
        .filter(|(i, r)| match &r.job {
            Job::Export { .. } => true,
            Job::Build { shown: true, .. } => Some(*i) == last_shown,
            Job::Drag { .. } | Job::DragVertex { .. } | Job::DragAnchor { .. } | Job::DragLabel { .. } => Some(*i) == keep_drag,
            Job::Build { view, shown: false, .. } => background.get(view) == Some(i),
        })
        .map(|(_, r)| r)
        .collect()
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
    if !matches!(incoming, Job::Build { shown: true, .. } | Job::Drag { .. } | Job::DragVertex { .. } | Job::DragAnchor { .. } | Job::DragLabel { .. }) {
        return false;
    }
    if matches!(running, Job::Export { .. }) {
        return false;
    }
    // DECISION: the rule is `latest` itself on the pair, so the two never
    // disagree; the clones are `Arc` copies.
    let pair = vec![
        Request {
            generation: Generation(0),
            job: running.clone(),
        },
        Request {
            generation: Generation(1),
            job: incoming.clone(),
        },
    ];
    !latest(pair).iter().any(|r| r.generation == Generation(0))
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
    let generation = request.generation;
    match request.job {
        Job::DragLabel { previous, edge, position } => {
            if stop.load(Ordering::Relaxed) {
                return None;
            }
            Some(Reply::Dragged {
                generation,
                build: Arc::new(label_preview(&previous, edge, position)),
            })
        }
        Job::DragAnchor {
            parsed,
            previous,
            edge,
            source,
            destination,
        } => rebuild_with_anchors(&parsed.model, &previous, edge, source, destination, &PREVIEW_ROUTE, stop)
            .ok()
            .map(|build| Reply::Dragged {
                generation,
                build: Arc::new(build),
            }),
        Job::DragVertex {
            parsed,
            previous,
            edge,
            vertices,
        } => rebuild_with_vertices(&parsed.model, &previous, edge, &vertices, &PREVIEW_ROUTE, stop)
            .ok()
            .map(|build| Reply::Dragged {
                generation,
                build: Arc::new(build),
            }),
        Job::Build {
            parsed,
            view,
            keep,
            shown,
            mode,
        } => match build(&parsed.model, &view, &keep, mode, stop) {
            Built::Done(build) => Some(Reply::Built {
                generation,
                view,
                build: Arc::new(*build),
                shown,
            }),
            Built::NoView => Some(Reply::NoView { generation, view }),
            Built::Stopped => None,
        },
        Job::Drag {
            parsed,
            previous,
            moved,
        } => rebuild_after_move(&parsed.model, &previous, &moved, &PREVIEW_ROUTE, stop)
            .ok()
            .map(|build| Reply::Dragged {
                generation,
                build: Arc::new(build),
            }),
        Job::Export {
            parsed,
            targets,
            format,
            png_width,
            mode,
        } => Some(Reply::Exported {
            generation,
            result: if format.per_view_text() || format.whole_workspace() {
                export_files(&parsed, &targets, format)
            } else {
                refuse_inputs(&parsed, &targets.iter().map(|t| t.path.clone()).collect::<Vec<_>>())
                    .and_then(|()| export(&parsed.model, &targets, format, png_width, mode))
            },
        }),
    }
}

/// The label drag preview (P2-708 by drag, EDIT-25): `previous` with the
/// `position` of `edge` set, its labels placed again by
/// `label::place_labels` on its own routes, and the scene's labels and
/// lines (whose chevrons keep clear of labels) taken from a scene built
/// on them. Nothing is re-routed; the lint report stays `previous`'s.
// DECISION: only labels and lines are taken from the rebuilt scene: the
// pub `scene::build` sizes boundary tags with the default terminology,
// and the rest of the scene does not depend on labels, so the shown
// scene's tags and texts stay exactly as built. An `edge` out of range
// gives `previous` unchanged.
fn label_preview(previous: &ViewBuild, edge: c4rizer_core::view::EdgeId, position: u8) -> ViewBuild {
    let mut build = previous.clone();
    let Some(e) = build.plan.edges.get_mut(edge.0 as usize) else {
        return build;
    };
    e.style.position = Some(position);
    build.labels = c4rizer_core::label::place_labels(&build.plan, &build.routes);
    let fresh = c4rizer_core::scene::build(&build.plan, &build.routes, &build.labels);
    build.scene.labels = fresh.labels;
    build.scene.lines = fresh.lines;
    build
}

/// The outcome of one full build.
enum Built {
    /// The build, boxed: the other variants are empty.
    Done(Box<ViewBuild>),
    /// The view does not exist or no longer opens.
    NoView,
    /// The stop flag was set.
    Stopped,
}

/// Builds `view` of `model` with `keep`: a declared view by key, a default
/// view through `pipeline::default_view` (EDIT-15).
fn build(model: &Model, view: &ViewRef, keep: &Keep, mode: Mode, stop: &AtomicBool) -> Built {
    let result = match view {
        ViewRef::Declared(key) => match build_view_in(model, key, keep, mode, stop) {
            Ok(b) => Ok(b),
            Err(BuildError::Stopped(_)) => return Built::Stopped,
            Err(_) => return Built::NoView,
        },
        ViewRef::Default { scope, kind } => {
            let Some(spec) = model.find(scope).and_then(|id| default_view(model, id, *kind)) else {
                return Built::NoView;
            };
            build_spec_in(model, &spec, keep, mode, stop)
        }
    };
    match result {
        Ok(b) => Built::Done(Box::new(b)),
        Err(_) => Built::Stopped,
    }
}

/// Writes every target of an export job (EDIT-7); the first failure ends
/// it as one sentence.
fn export(
    model: &Model,
    targets: &[ExportTarget],
    format: ExportFormat,
    png_width: Option<u32>,
    mode: Mode,
) -> Result<Vec<PathBuf>, String> {
    // An export is never stopped: its flag is never set.
    let never = AtomicBool::new(false);
    let mut written = Vec::with_capacity(targets.len());
    for target in targets {
        let Built::Done(build) = build(model, &target.view, &target.keep, mode, &never) else {
            return Err(format!(
                "cannot export view \"{}\": it is no longer in the file",
                target.view.key()
            ));
        };
        let bytes = match format {
            ExportFormat::Svg => c4rizer_core::export::svg(&build.scene).into_bytes(),
            // P2-664: the same functions as `c4rizer render --svg-text`.
            ExportFormat::SvgEmbedded => c4rizer_core::export::svg_embedded(&build.scene).into_bytes(),
            ExportFormat::SvgOutlined => c4rizer_core::export::svg_outlined(&build.scene)
                .map_err(|e| format!("cannot export view \"{}\" with outlined text: {e}", target.view.key()))?
                .into_bytes(),
            ExportFormat::Png => {
                let width = png_width.unwrap_or_else(|| {
                    u32::try_from(build.scene.bounds.width()).unwrap_or(0).saturating_mul(2)
                });
                c4rizer_core::export::png(&build.scene, width).map_err(|e| {
                    format!("cannot export view \"{}\" as PNG: {e}", target.view.key())
                })?
            }
            _ => unreachable!("text, JSON, theme and static go through `export_files`"),
        };
        export_files::replace_file(&target.path, &bytes)
            .map_err(|e| format!("cannot write {}: {e}", target.path.display()))?;
        written.push(target.path.clone());
    }
    Ok(written)
}

/// Writes a Structurizr text format, JSON, theme or the static site
/// (P3-431) with the same core functions and file names as `c4rizer
/// export -f` (P3-400): a per-view text format writes each target's
/// `structurizr-<key>.<ext>` to the target's path (`PlantUml` adds the
/// `-key.puml` legend beside it); a whole-workspace format writes every
/// file into the folder of the first target. Views a format cannot
/// express are skipped, as the CLI skips them (P3-405).
// DECISION: these formats are built from the file alone in light mode,
// as the CLI builds them (`pipeline::build_all`), so the files equal
// what `c4rizer export` writes; the canvas keep and the dark mode only
// reach the SVG and PNG exports.
// DECISION: `structurizr.dsl` of the JSON is the root file's text
// (`trees[0]`), none for a workspace JSON (P3-420), and the JSON is
// named after the root file's stem, as the CLI names it.
fn export_files(parsed: &Parsed, targets: &[ExportTarget], format: ExportFormat) -> Result<Vec<PathBuf>, String> {
    let model = &parsed.model;
    let Ok(all) = c4rizer_core::pipeline::build_all(model, &AtomicBool::new(false)) else {
        return Err("the export was stopped".to_owned());
    };
    // A workspace JSON is read through a stub `<name>.c4rizer-json.dsl`
    // beside it (`state::parse_doc`): the file is the JSON, with no DSL.
    let root = parsed.paths.first().cloned().unwrap_or_default();
    let root_name = root.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    let json_doc = root_name.ends_with(JSON_STUB_SUFFIX);
    let file = if json_doc { root.with_file_name(root_name.trim_end_matches(JSON_STUB_SUFFIX)) } else { root.clone() };
    let dsl = (!json_doc).then(|| parsed.trees.first().map(c4rizer_core::text::SyntaxTree::print)).flatten();
    let name = match format {
        ExportFormat::PlantUml => "plantuml/structurizr",
        ExportFormat::C4PlantUml => "plantuml/c4plantuml",
        ExportFormat::Mermaid => "mermaid",
        ExportFormat::WebSequenceDiagrams => "websequencediagrams",
        ExportFormat::Dot => "dot",
        ExportFormat::Ilograph => "ilograph",
        ExportFormat::Json => "json",
        ExportFormat::Theme => "theme",
        ExportFormat::Static => "static",
        ExportFormat::Svg | ExportFormat::Png | ExportFormat::SvgEmbedded | ExportFormat::SvgOutlined => {
            unreachable!("image formats go through `export`")
        }
    };
    let source = export_files::Source {
        model,
        all: &all,
        path: Some(&file),
        dsl: dsl.as_deref(),
    };
    let Some(files) = export_files::files(name, &source, false) else {
        unreachable!("every text, JSON, theme and static format is in export_files");
    };
    // CLI-1's guard, as `c4rizer export` applies it: keys equal up to case
    // would overwrite each other's files (per-view formats, static).
    if !matches!(format, ExportFormat::Json | ExportFormat::Theme | ExportFormat::Ilograph) {
        let keys: Vec<&str> = all.views.iter().map(|v| v.plan.key.as_str()).collect();
        if let Some((first, second)) = export_files::case_collision(&keys) {
            return Err(format!(
                "nothing exported: view keys \"{first}\" and \"{second}\" differ only in case, so their files would overwrite each other — rename one of the views"
            ));
        }
    }
    // Every file to write, decided before any is written (R1).
    let mut plan: Vec<(PathBuf, &[u8])> = Vec::new();
    let mut skipped = None;
    if format.whole_workspace() {
        let Some(folder) = targets.first().and_then(|t| t.path.parent()) else {
            return Err("nothing to export: no folder chosen".to_owned());
        };
        plan.extend(files.iter().map(|(n, b)| (folder.join(n), b.as_slice())));
    } else {
        let ext = format.extension();
        for target in targets {
            let key = target.view.key();
            let name = PathBuf::from(format!("structurizr-{key}.{ext}"));
            let Some((_, bytes)) = files.iter().find(|(n, _)| *n == name) else {
                skipped.get_or_insert(key);
                continue;
            };
            plan.push((target.path.clone(), bytes));
            let legend = PathBuf::from(format!("structurizr-{key}-key.{ext}"));
            if let Some((_, bytes)) = files.iter().find(|(n, _)| *n == legend) {
                let stem = target.path.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
                plan.push((target.path.with_file_name(format!("{stem}-key.{ext}")), bytes));
            }
        }
    }
    if plan.is_empty()
        && let Some(key) = skipped
    {
        return Err(format!("nothing to export: view \"{key}\" cannot be written in this format"));
    }
    let outputs: Vec<PathBuf> = plan.iter().map(|(p, _)| p.clone()).collect();
    refuse_inputs(parsed, &outputs)?;
    let mut written = Vec::new();
    for (path, bytes) in plan {
        if let Some(parent) = path.parent().filter(|p| !p.as_os_str().is_empty()) {
            std::fs::create_dir_all(parent).map_err(|e| format!("cannot create {}: {e}", parent.display()))?;
        }
        export_files::replace_file(&path, bytes).map_err(|e| format!("cannot write {}: {e}", path.display()))?;
        written.push(path);
    }
    Ok(written)
}

/// The name suffix of the stub a workspace JSON is parsed through
/// (`state::parse_doc`).
pub(crate) const JSON_STUB_SUFFIX: &str = ".c4rizer-json.dsl";

/// R1: an error naming both paths when an output is a file the
/// document was read from (the root file, every included or extended
/// file; same device and inode), so the export writes nothing.
fn refuse_inputs(parsed: &Parsed, outputs: &[PathBuf]) -> Result<(), String> {
    match export_files::input_hit(outputs, &parsed.paths) {
        Some((out, input)) => Err(format!(
            "export refused: {} is a file this diagram is read from ({}) — choose another folder or name",
            out.display(),
            input.display()
        )),
        None => Ok(()),
    }
}

/// The worker thread's loop: wait for a request, take every queued one,
/// keep `latest` of them, run the first with a fresh stop flag published
/// in `running`, send its reply, repeat until the request channel closes.
fn run_thread(
    inbox: &Receiver<Request>,
    outbox: &Sender<Reply>,
    running: &Mutex<Option<Running>>,
    wake: &Wake,
) {
    let mut pending: Vec<Request> = Vec::new();
    loop {
        if pending.is_empty() {
            match inbox.recv() {
                Ok(r) => pending.push(r),
                Err(_) => return,
            }
        }
        pending.extend(inbox.try_iter());
        // DECISION: `latest` runs again before every job, so a request
        // that arrived while the previous job ran can still drop the
        // queued ones it supersedes.
        pending = latest(std::mem::take(&mut pending));
        if pending.is_empty() {
            continue;
        }
        let request = pending.remove(0);
        let stop = Arc::new(AtomicBool::new(false));
        if let Ok(mut slot) = running.lock() {
            *slot = Some(Running {
                generation: request.generation,
                job: request.job.clone(),
                stop: Arc::clone(&stop),
            });
        }
        let reply = serve(request, &stop);
        if let Ok(mut slot) = running.lock() {
            *slot = None;
        }
        if let Some(reply) = reply {
            if outbox.send(reply).is_err() {
                return;
            }
            wake();
        }
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;
    use std::path::PathBuf;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::time::{Duration, Instant};

    use c4rizer_core::geom::Point;
    use c4rizer_core::model::{Parsed, ViewKind, parse};
    use c4rizer_core::pipeline::{PREVIEW_ROUTE, ViewBuild, rebuild_after_move};
    use c4rizer_core::place::Keep;
    use c4rizer_core::view::BoxId;

    use super::{
        ExportFormat, ExportTarget, Generation, Job, Reply, Request, ViewRef, Worker, latest,
        serve, stops,
    };
    use crate::testutil::{SHOP, TempDir, heavy};

    /// Size pin for the hot `Generation` (EDIT-25).
    #[test]
    fn generation_is_8_bytes() {
        assert_eq!(std::mem::size_of::<super::Generation>(), 8);
    }

    fn shop() -> Arc<Parsed> {
        Arc::new(parse(SHOP).expect("fixture parses"))
    }

    fn declared(key: &str) -> ViewRef {
        ViewRef::Declared(key.to_owned())
    }

    fn built(parsed: &Parsed, key: &str) -> Arc<ViewBuild> {
        let reply = serve(
            Request {
                generation: Generation(1),
                job: build_job(parsed, key, true),
            },
            &AtomicBool::new(false),
        );
        match reply {
            Some(Reply::Built { build, .. }) => build,
            other => panic!("expected Built, got {other:?}"),
        }
    }

    fn build_job(parsed: &Parsed, key: &str, shown: bool) -> Job {
        Job::Build {
            parsed: Arc::new(parsed.clone()),
            view: declared(key),
            keep: Keep::default(),
            shown,
            mode: c4rizer_core::model::Mode::Light,
        }
    }

    fn drag_job(parsed: &Arc<Parsed>, previous: &Arc<ViewBuild>, x: i32) -> Job {
        let mut moved = BTreeMap::new();
        moved.insert(BoxId(0), Point::new(x, 10));
        Job::Drag {
            parsed: Arc::clone(parsed),
            previous: Arc::clone(previous),
            moved,
        }
    }

    fn export_job(parsed: &Arc<Parsed>) -> Job {
        Job::Export {
            parsed: Arc::clone(parsed),
            targets: Vec::new(),
            format: ExportFormat::Svg,
            png_width: None,
            mode: c4rizer_core::model::Mode::Light,
        }
    }

    fn req(generation: u64, job: Job) -> Request {
        Request {
            generation: Generation(generation),
            job,
        }
    }

    fn gens(requests: &[Request]) -> Vec<u64> {
        requests.iter().map(|r| r.generation.0).collect()
    }

    #[test]
    fn view_ref_key_and_extension() {
        assert_eq!(declared("context").key(), "context");
        let d = ViewRef::Default {
            scope: "web".to_owned(),
            kind: ViewKind::Component,
        };
        assert_eq!(d.key(), "web-component");
        assert_eq!(ExportFormat::Svg.extension(), "svg");
        assert_eq!(ExportFormat::Png.extension(), "png");
        assert_eq!(Generation(u64::MAX).next(), Generation(u64::MAX));
        assert_eq!(Generation(4).next(), Generation(5));
    }

    /// EDIT-25 (plan editor-02): of three queued drags and a shown build
    /// after them, `latest` keeps only the build; of three drags alone,
    /// only the newest.
    #[test]
    fn edit_25_stale_requests_dropped() {
        let parsed = shop();
        let previous = built(&parsed, "containers");
        let drags = |from: u64| (0..3).map(|i| req(from + i, drag_job(&parsed, &previous, 10 + i32::try_from(i).unwrap()))).collect::<Vec<_>>();
        let mut queue = drags(1);
        queue.push(req(4, build_job(&parsed, "containers", true)));
        assert_eq!(gens(&latest(queue)), vec![4]);
        assert_eq!(gens(&latest(drags(1))), vec![3]);
        // A drag after the build is kept beside it.
        let mut queue = vec![req(1, build_job(&parsed, "containers", true))];
        queue.extend(drags(2));
        assert_eq!(gens(&latest(queue)), vec![1, 4]);
        // Two shown builds: only the newest.
        let queue = vec![
            req(1, build_job(&parsed, "containers", true)),
            req(2, build_job(&parsed, "context", true)),
        ];
        assert_eq!(gens(&latest(queue)), vec![2]);
    }

    /// EDIT-14: background builds are dropped while a shown build or a
    /// drag is queued, and never run before it.
    #[test]
    fn edit_14_background_builds_never_delay_shown_view() {
        let parsed = shop();
        let previous = built(&parsed, "containers");
        let queue = vec![
            req(1, build_job(&parsed, "context", false)),
            req(2, build_job(&parsed, "containers", true)),
            req(2, build_job(&parsed, "context", false)),
        ];
        assert_eq!(gens(&latest(queue)), vec![2], "only the shown build");
        let queue = vec![
            req(1, build_job(&parsed, "context", false)),
            req(2, drag_job(&parsed, &previous, 30)),
        ];
        assert_eq!(gens(&latest(queue)), vec![2]);
        // Alone: one per view, the newest, none older than a shown build.
        let queue = vec![
            req(1, build_job(&parsed, "context", false)),
            req(3, build_job(&parsed, "context", false)),
            req(3, build_job(&parsed, "containers", false)),
        ];
        assert_eq!(gens(&latest(queue)), vec![3, 3]);
        // A running background build stops for a drag or a shown build.
        let bg = build_job(&parsed, "context", false);
        assert!(stops(&bg, &drag_job(&parsed, &previous, 30)));
        assert!(stops(&bg, &build_job(&parsed, "containers", true)));
        assert!(!stops(&bg, &build_job(&parsed, "containers", false)));
        let shown = build_job(&parsed, "containers", true);
        assert!(!stops(&shown, &drag_job(&parsed, &previous, 30)));
        assert!(stops(&shown, &build_job(&parsed, "context", true)));
        let drag = drag_job(&parsed, &previous, 30);
        assert!(stops(&drag, &drag_job(&parsed, &previous, 40)));
    }

    /// EDIT-25: `serve` of a drag job re-routes only lines touching the
    /// moved box (pipeline's `rebuild_after_move`) and keeps the
    /// generation.
    #[test]
    fn edit_25_drag_reply_keeps_generation() {
        let parsed = shop();
        let previous = built(&parsed, "containers");
        let x = previous.routes.centres[0].x + 40;
        let reply = serve(req(17, drag_job(&parsed, &previous, x)), &AtomicBool::new(false));
        let Some(Reply::Dragged { generation, build }) = reply else {
            panic!("expected Dragged, got {reply:?}");
        };
        assert_eq!(generation, Generation(17));
        let mut moved = BTreeMap::new();
        moved.insert(BoxId(0), Point::new(x, 10));
        let expected = rebuild_after_move(&parsed.model, &previous, &moved, &PREVIEW_ROUTE, &AtomicBool::new(false)).unwrap();
        assert_eq!(*build, expected);
        assert_eq!(build.scene.centres[0], Point::new(x, 10));
        // A stopped drag gives no reply.
        let stopped = AtomicBool::new(true);
        assert!(serve(req(18, drag_job(&parsed, &previous, x + 10)), &stopped).is_none());
    }

    fn vertex_job(parsed: &Arc<Parsed>, previous: &Arc<ViewBuild>, x: i32) -> Job {
        Job::DragVertex {
            parsed: Arc::clone(parsed),
            previous: Arc::clone(previous),
            edge: c4rizer_core::view::EdgeId(0),
            vertices: vec![Point::new(x, -20)],
        }
    }

    /// P3-311, EDIT-25: a vertex drag is served off the UI thread like a
    /// box drag (`rebuild_with_vertices` with `PREVIEW_ROUTE`), stale ones
    /// dropped and stopped the same way.
    #[test]
    fn p3_311_vertex_drag_job_served_and_dropped_like_drag() {
        let parsed = shop();
        let previous = built(&parsed, "containers");
        let x = previous.routes.centres[0].x;
        let reply = serve(req(9, vertex_job(&parsed, &previous, x)), &AtomicBool::new(false));
        let Some(Reply::Dragged { generation, build }) = reply else {
            panic!("expected Dragged, got {reply:?}");
        };
        assert_eq!(generation, Generation(9));
        let expected = c4rizer_core::pipeline::rebuild_with_vertices(
            &parsed.model,
            &previous,
            c4rizer_core::view::EdgeId(0),
            &[Point::new(x, -20)],
            &PREVIEW_ROUTE,
            &AtomicBool::new(false),
        )
        .unwrap();
        assert_eq!(*build, expected);
        assert!(serve(req(10, vertex_job(&parsed, &previous, x)), &AtomicBool::new(true)).is_none());
        // Of three vertex drags only the newest; a shown build after them wins.
        let queue: Vec<Request> = (0..3).map(|i| req(i + 1, vertex_job(&parsed, &previous, x + i32::try_from(i).unwrap()))).collect();
        assert_eq!(gens(&latest(queue)), vec![3]);
        let mut queue: Vec<Request> = (0..2).map(|i| req(i + 1, vertex_job(&parsed, &previous, x))).collect();
        queue.push(req(3, build_job(&parsed, "containers", true)));
        assert_eq!(gens(&latest(queue)), vec![3]);
        // A box drag and a vertex drag: the newer one.
        let queue = vec![req(1, drag_job(&parsed, &previous, 30)), req(2, vertex_job(&parsed, &previous, x))];
        assert_eq!(gens(&latest(queue)), vec![2]);
        // Stops: a running vertex drag stops for a newer one or a shown build.
        let running = vertex_job(&parsed, &previous, x);
        assert!(stops(&running, &vertex_job(&parsed, &previous, x + 1)));
        assert!(stops(&running, &build_job(&parsed, "containers", true)));
        assert!(stops(&build_job(&parsed, "context", false), &vertex_job(&parsed, &previous, x)));
    }

    /// EDIT-7: an export job is never dropped by `latest`, whatever
    /// follows it.
    #[test]
    fn edit_7_export_never_dropped() {
        let parsed = shop();
        let previous = built(&parsed, "containers");
        let queue = vec![
            req(1, export_job(&parsed)),
            req(2, drag_job(&parsed, &previous, 20)),
            req(3, export_job(&parsed)),
            req(4, build_job(&parsed, "containers", true)),
            req(5, build_job(&parsed, "context", false)),
        ];
        assert_eq!(gens(&latest(queue)), vec![1, 3, 4]);
    }

    /// EDIT-25: with a background build of a heavy view running on the
    /// thread, a drag request sent after it is served (its `Dragged`
    /// reply arrives) without waiting for that build to finish: `send`
    /// sets the build's stop flag, the build gives no reply.
    #[test]
    fn edit_25_drag_served_without_waiting_for_background_build() {
        let heavy = Arc::new(parse(&heavy(12)).expect("heavy parses"));
        let parsed = shop();
        let previous = built(&parsed, "containers");
        let worker = Worker::spawn(Arc::new(|| {}));
        worker.send(req(1, Job::Build {
            parsed: Arc::clone(&heavy),
            view: declared("heavy"),
            keep: Keep::default(),
            shown: false,
            mode: c4rizer_core::model::Mode::Light,
        }));
        // Wait until the thread runs it.
        let start = Instant::now();
        loop {
            if worker.running.lock().unwrap().as_ref().is_some_and(|r| r.generation == Generation(1)) {
                break;
            }
            assert!(start.elapsed() < Duration::from_secs(5), "background build never started");
            std::thread::sleep(Duration::from_millis(1));
        }
        let flag = Arc::clone(&worker.running.lock().unwrap().as_ref().unwrap().stop);
        let sent = Instant::now();
        worker.send(req(2, drag_job(&parsed, &previous, previous.routes.centres[0].x + 30)));
        assert!(flag.load(Ordering::Relaxed), "send set the stop flag");
        let mut replies = Vec::new();
        while !replies.iter().any(|r| matches!(r, Reply::Dragged { .. })) {
            assert!(sent.elapsed() < Duration::from_secs(10), "drag not served: {replies:?}");
            replies.extend(worker.drain());
            std::thread::sleep(Duration::from_millis(2));
        }
        // Give a stray Built time to show up; there must be none.
        std::thread::sleep(Duration::from_millis(50));
        replies.extend(worker.drain());
        assert!(
            replies.iter().all(|r| matches!(r, Reply::Dragged { generation: Generation(2), .. })),
            "{replies:?}"
        );
    }

    /// EDIT-7, EDIT-25: an `Export` is never stopped: `stops` is false for
    /// a running export whatever arrives, and `serve` of an export with a
    /// set flag still writes its files and replies `Exported`.
    #[test]
    fn edit_7_export_never_stopped() {
        let parsed = shop();
        let previous = built(&parsed, "containers");
        let export = export_job(&parsed);
        assert!(!stops(&export, &drag_job(&parsed, &previous, 30)));
        assert!(!stops(&export, &build_job(&parsed, "containers", true)));
        assert!(!stops(&export, &export_job(&parsed)));
        let dir = TempDir::new("export-stop");
        let path = dir.join("containers.svg");
        let job = Job::Export {
            parsed: Arc::clone(&parsed),
            targets: vec![ExportTarget {
                view: declared("containers"),
                keep: Keep::default(),
                path: path.clone(),
            }],
            format: ExportFormat::Svg,
            png_width: None,
            mode: c4rizer_core::model::Mode::Light,
        };
        let reply = serve(req(9, job), &AtomicBool::new(true));
        let Some(Reply::Exported { generation, result }) = reply else {
            panic!("expected Exported, got {reply:?}");
        };
        assert_eq!(generation, Generation(9));
        assert_eq!(result, Ok(vec![path.clone()]));
        let svg = std::fs::read_to_string(&path).unwrap();
        assert_eq!(svg, c4rizer_core::export::svg(&previous.scene));
        let _: PathBuf = path;
    }

    /// EDIT-15: `serve` builds a default view through
    /// `pipeline::default_view`, and replies `NoView` when it no longer
    /// opens.
    #[test]
    fn edit_15_default_view_built_or_no_view() {
        let parsed = shop();
        let view = ViewRef::Default {
            scope: "web".to_owned(),
            kind: ViewKind::Component,
        };
        let job = |view: &ViewRef| Job::Build {
            parsed: Arc::clone(&parsed),
            view: view.clone(),
            keep: Keep::default(),
            shown: true,
            mode: c4rizer_core::model::Mode::Light,
        };
        let reply = serve(req(3, job(&view)), &AtomicBool::new(false));
        let Some(Reply::Built { build, view: v, .. }) = reply else {
            panic!("expected Built, got {reply:?}");
        };
        assert_eq!(v, view);
        assert_eq!(build.scene.key, "web-component");
        assert!(build.scene.boxes.len() >= 2);
        // `shop` already has a container view: its default does not open.
        let gone = ViewRef::Default {
            scope: "shop".to_owned(),
            kind: ViewKind::Container,
        };
        let reply = serve(req(4, job(&gone)), &AtomicBool::new(false));
        assert!(matches!(reply, Some(Reply::NoView { generation: Generation(4), .. })), "{reply:?}");
        let reply = serve(req(5, job(&declared("nope"))), &AtomicBool::new(false));
        assert!(matches!(reply, Some(Reply::NoView { .. })), "{reply:?}");
    }
}

//! The eframe shell: panel layout (toolbar with "Fit view" and the side
//! panel toggles on top, view list and breadcrumb left, canvas centre,
//! details and style panel plus lint findings right, status bar),
//! turning egui input and panel clicks into `state::Event`s and running
//! the `state::Effect`s (file I/O, worker requests, dialogs).
//!
//! Depends on: `state`, `canvas`, `worker`, `files`, `history`,
//! `c4rizer-core`, `eframe`.

// Shape: `App` owns the `State`, the worker, the watcher and the file
// event channel. Each frame: drain worker replies and file events into
// `State::handle`, draw the panels from `State`'s readers (they return
// events for clicks), feed canvas input, run every returned effect. No
// decision is taken here that a test of `state` would miss.
//
// DECISION: dialogs (`rfd`) block the UI thread while open, like every
// native dialog; worker replies and file events queue meanwhile and are
// handled on the next frame.

use std::cell::Cell;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::mpsc::{self, Receiver, Sender};
use std::time::Instant;

use c4rizer_core::model::{Mode, ViewKind};
use eframe::egui;

use crate::camera::ScreenPoint;
use crate::canvas;
use crate::files::{self, FileEvent, UiState, Watcher};
use crate::state::{Command, Dialog, Effect, Event, Problem, State, ViewEntry};
use crate::theme;
use crate::worker::{ExportFormat, ViewRef, Wake, Worker};

mod frame;
mod inplace;
mod panels;

pub(crate) use inplace::focus_keys;
#[cfg(test)]
pub(crate) use inplace::suppress as suppress_edited;

/// The startup report request (EDIT-14).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Startup {
    /// Taken first thing in `main`: the process start for EDIT-14.
    // DECISION: `Instant::now()` at the top of `main` stands for the
    // process start; the runtime's pre-`main` setup (well under 1 ms) is
    // not counted. Reading the OS process start time would need
    // platform code or a new dependency.
    pub began: Instant,
    /// `--startup-report`: print ms to the first presented frame and
    /// exit.
    pub report: bool,
}

/// The running editor. Serves EDIT-1..29 through `State`.
pub struct App {
    /// The headless state machine.
    pub state: State,
    /// The pipeline thread (EDIT-25).
    pub worker: Worker,
    /// The directory watcher of the open file (EDIT-1, EDIT-23); `None`
    /// before an open or when watching failed.
    pub watcher: Option<Watcher>,
    /// Sender cloned into each watcher.
    pub file_tx: Sender<FileEvent>,
    /// File events from the watcher.
    pub file_rx: Receiver<FileEvent>,
    /// Where the per-user state is saved (`files::state_file`).
    pub ui_state_path: Option<PathBuf>,
    /// EDIT-14 startup measurement.
    pub startup: Startup,
    /// True once the first frame was presented.
    pub presented: bool,
    // DECISION: the EDIT-32 panel flags are private fields so `App`'s
    // pub fields (the step-4 contract) stay as they are; only `new`
    // builds an `App`.
    /// Left and right side panels open (EDIT-32); `None` until the first
    /// frame knows the window width.
    panels: Option<(bool, bool)>,
    /// The watcher of the open diagram's included files (P2-141), `None`
    /// when it includes none or watching failed.
    includes: Option<files::FilesWatcher>,
    /// Sender cloned into each include watcher.
    include_tx: Sender<(PathBuf, FileEvent)>,
    /// Included-file events from the include watcher.
    include_rx: Receiver<(PathBuf, FileEvent)>,
    /// The explore pages (P3-9), shown instead of the canvas while open.
    explore: crate::explore::Explore,
    /// The view map section of the left panel (P4-100).
    view_map: crate::view_map::ViewMapPanel,
    /// The canvas region on screen last frame, for the overlays placed
    /// over it.
    canvas_rect: Cell<egui::Rect>,
    /// The canvas size last frame (the fit note).
    canvas_size: Cell<ScreenPoint>,
    /// The window width last frame: crossing under 1000 px closes both
    /// panels (behaviour change 1).
    last_width: Option<f32>,
    /// The context menu on screen last frame (behaviour change 6).
    menu_rect: Cell<Option<egui::Rect>>,
}

/// Window width from which the side panels start open (EDIT-32).
const WIDE_WINDOW_PX: f32 = 1000.0;

/// Share of the window width each side panel may take at most, so the
/// canvas keeps at least 60% (EDIT-32).
// DECISION: 19%, not 20%: the last percent absorbs float rounding and
// the panels' frame so the canvas is never a hair under 60%.
const SIDE_PANEL_SHARE: f32 = 0.19;

/// Width a side panel adds beyond its size: the separator line and the
/// resize grip, px (EDIT-32).
// DECISION: 4px covers egui's 1px separator plus rounding with room to
// spare; the real-window screenshot is the check.
const PANEL_CHROME_PX: f32 = 4.0;

/// Smallest width a side panel can be dragged to, px (EDIT-32).
const SIDE_PANEL_MIN_PX: f32 = 120.0;

/// True when the side panels start open at this window width (EDIT-32).
fn side_panels_start_open(window_width: f32) -> bool {
    window_width >= WIDE_WINDOW_PX
}

/// The drag range of one side panel at this window width, px: both at
/// their widest still leave the canvas 60% of the width (EDIT-32).
fn side_panel_range(window_width: f32) -> (f32, f32) {
    let max = (window_width * SIDE_PANEL_SHARE - PANEL_CHROME_PX).max(0.0);
    (SIDE_PANEL_MIN_PX.min(max), max)
}

/// True when F was pressed this frame with no modifier and no text field
/// has focus: "Fit view" (EDIT-30). The event's own modifiers count,
/// so Ctrl+F is not a fit.
// DECISION: read in the shell, not added to `state::Key`, so the pub
// `Key` enum is unchanged; the text-focus rule is the same as
// Backspace's (EDIT-16).
fn fit_key_pressed(ctx: &egui::Context) -> bool {
    !ctx.text_edit_focused()
        && ctx.input(|i| {
            i.events.iter().any(|e| {
                matches!(e, egui::Event::Key { key: egui::Key::F, pressed: true, repeat: false, modifiers, .. }
                    if !(modifiers.ctrl || modifiers.command || modifiers.alt))
            })
        })
}

impl App {
    /// Builds the app: loads per-user state, spawns the worker with a
    /// repaint waker, installs the fonts, and opens `file` when given
    /// (EDIT-13). Serves EDIT-13, EDIT-14.
    #[must_use]
    pub fn new(cc: &eframe::CreationContext<'_>, file: Option<PathBuf>, startup: Startup) -> App {
        theme::install(&cc.egui_ctx);
        let ui_state_path = files::state_file();
        let ui = ui_state_path.as_deref().map(UiState::load).unwrap_or_default();
        let worker = Worker::spawn(waker(&cc.egui_ctx));
        let (file_tx, file_rx) = mpsc::channel();
        let (include_tx, include_rx) = mpsc::channel();
        let mut app = App {
            state: State::new(ui),
            worker,
            watcher: None,
            file_tx,
            file_rx,
            ui_state_path,
            startup,
            presented: false,
            panels: None,
            includes: None,
            include_tx,
            include_rx,
            explore: crate::explore::Explore::default(),
            view_map: crate::view_map::ViewMapPanel::default(),
            canvas_rect: Cell::new(egui::Rect::NOTHING),
            canvas_size: Cell::new(ScreenPoint { x: 0.0, y: 0.0 }),
            last_width: None,
            menu_rect: Cell::new(None),
        };
        if let Some(path) = file {
            app.dispatch(&cc.egui_ctx, Event::Command(Command::Open(path)));
        }
        app
    }

    /// Feeds `event` to the state and runs the effects. Serves EDIT-1..29.
    pub fn dispatch(&mut self, ctx: &egui::Context, event: Event) {
        let effects = self.state.handle(event);
        self.run(ctx, effects);
        self.watch_includes(ctx);
    }

    /// Restarts the include watcher when the set of included files of the
    /// last parse changed (P2-141).
    // DECISION: the shell polls `State::included_files` after each event
    // instead of a new `Effect` variant (pub, step-4 contract), like the
    // canvas size of EDIT-30.
    fn watch_includes(&mut self, ctx: &egui::Context) {
        let wanted = self.state.included_files();
        let watched = self.includes.as_ref().map_or(&[][..], |w| w.paths.as_slice());
        if wanted == watched {
            return;
        }
        self.includes = None;
        if wanted.is_empty() {
            return;
        }
        match files::watch_files(&wanted, self.include_tx.clone(), waker(ctx)) {
            Ok(w) => self.includes = Some(w),
            Err(e) => {
                self.state.io_error = Some(format!("cannot watch the included files: {e}; their live reload is off"));
            }
        }
    }

    /// Runs effects in order: reads and checked writes through `files`
    /// (results fed back as events), watcher replacement, worker sends,
    /// dialogs (answers fed back as commands), per-user state saves.
    /// Serves EDIT-7, EDIT-13, EDIT-22..24.
    pub fn run(&mut self, ctx: &egui::Context, effects: Vec<Effect>) {
        for effect in effects {
            match effect {
                Effect::Read(path) => {
                    let event = match files::read(&path) {
                        Ok((text, hash)) => FileEvent::Loaded { path, text, hash },
                        Err(e) => FileEvent::OpenFailed {
                            error: format!("cannot open {}: {e}", path.display()),
                            path,
                        },
                    };
                    self.dispatch(ctx, Event::File(event));
                }
                Effect::Watch(path) => {
                    self.watcher = None;
                    match Watcher::start(&path, self.file_tx.clone(), waker(ctx)) {
                        Ok(watcher) => self.watcher = Some(watcher),
                        // DECISION: `FileEvent` has no watch-failure
                        // variant (a pub change), so the shell writes the
                        // sentence into the state's I/O problem directly;
                        // editing goes on without live reload.
                        Err(e) => {
                            self.state.io_error =
                                Some(format!("cannot watch {}: {e}; live reload is off", path.display()));
                        }
                    }
                }
                Effect::Write(request) => {
                    let event = files::write_checked(&request);
                    self.dispatch(ctx, Event::File(event));
                }
                Effect::Work(request) => self.worker.send(request),
                Effect::Dialog(dialog) => {
                    if let Some(command) = self.ask(dialog) {
                        self.dispatch(ctx, Event::Command(command));
                    }
                }
                Effect::SaveUiState(ui) => {
                    // DECISION: a failed save of the per-user state is
                    // not worth a status line; the diagram is unaffected.
                    if let Some(path) = &self.ui_state_path {
                        let _ = ui.save(path);
                    }
                }
                Effect::Fullscreen(on) => ctx.send_viewport_cmd(egui::ViewportCommand::Fullscreen(on)),
                Effect::OpenUrl(url) => ctx.open_url(egui::OpenUrl::new_tab(url)),
            }
        }
    }

    /// Shows `dialog` (blocking, `rfd`) and turns its answer into a
    /// command; `None` on cancel (EDIT-7, EDIT-13).
    fn ask(&mut self, dialog: Dialog) -> Option<Command> {
        match dialog {
            Dialog::Open { dir } => files::open_dialog(dir.as_deref()).map(Command::Open),
            Dialog::ExportView {
                view,
                dir,
                name,
                format,
            } => files::save_dialog(&dir, &name, format).map(|path| Command::Export {
                targets: vec![(view, path)],
                format,
            }),
            Dialog::ExportAll { dir, format } => files::folder_dialog(&dir).map(|folder| Command::Export {
                targets: export_all_targets(&self.state, &folder, format),
                format,
            }),
            Dialog::ExportDocument { dir, name } => files::export_document_dialog(&dir, &name).map(|path| Command::Export {
                targets: export_document_targets(&self.state, &path),
                format: ExportFormat::Pdf,
            }),
            Dialog::SaveAs { dir, name, text } => {
                let path = files::save_as_dialog(&dir, &name)?;
                // DECISION: Save as copies the open file's text and opens
                // the copy; files it `!include`s are not copied, and a
                // relative include then resolves from the copy's folder.
                // A failed write is the shell's I/O sentence, as a failed
                // watch is, since `FileEvent` has no variant for it.
                match files::write_atomic(&path, &text) {
                    Ok(_) => Some(Command::Open(path)),
                    Err(e) => {
                        self.state.io_error = Some(format!("cannot save as {}: {e}", path.display()));
                        None
                    }
                }
            }
        }
    }

    /// The left panel with the view map in its bottom strip (P4-100),
    /// drawn first so it claims that strip; a click on a node is the view
    /// list's `ShowView` (P4-109).
    fn left_side(&mut self, ui: &mut egui::Ui) -> Vec<Event> {
        let mut out = Vec::new();
        if let Some(parsed) = self.state.doc.as_ref().and_then(|d| d.parsed.clone()) {
            let current = self.state.view.clone();
            if let Some(view) = self.view_map.show_in_panel(ui, &parsed, current.as_ref()) {
                out.push(Event::Command(Command::ShowView(view)));
            }
        }
        out.extend(self.left_panel(ui));
        out
    }

    /// Forward from the toolbar (P4-115): a view pick, so it leaves
    /// Explore as Back does.
    fn go_forward(&mut self, ctx: &egui::Context) {
        self.explore.open = false;
        let effects = self.state.forward();
        self.run(ctx, effects);
    }

    /// The centre: the backdrop, the page (`canvas::paint_page`), the
    /// scene and marks (`canvas::paint_items`, `canvas::paint`), the marks
    /// outside the paper, the in-place fields in the edited item, in
    /// place of its texts (`inplace`; changes as `Event::Text`, focus
    /// moves as Tab presses), the tooltip card, the right-click
    /// menu and canvas input; the start screen with no file open. Returns
    /// the events. Serves EDIT-5, EDIT-9, EDIT-12, EDIT-13, EDIT-28.
    #[must_use]
    pub fn canvas(&self, ui: &mut egui::Ui) -> Vec<Event> {
        let (response, painter) = ui.allocate_painter(ui.available_size(), egui::Sense::click_and_drag());
        let painter = painter.with_clip_rect(response.rect);
        self.canvas_rect.set(response.rect);
        self.paint_backdrop(&painter, response.rect);
        if self.state.doc.is_none() {
            let mut child = ui.new_child(egui::UiBuilder::new().max_rect(response.rect));
            return self.start_screen(&mut child);
        }
        let list = self.state.draw_list();
        // While the zoom moves, text is drawn from a few cached sizes.
        canvas::track_zoom(ui.ctx(), list.camera.zoom);
        // EDIT-31: the page in the export background first.
        // P2-701: the paper of a view with a canvas size, under the page.
        canvas::paint_page(&painter, &list, self.state.paper_rect());
        // EDIT-5: the edited item's texts are typed in place, not drawn.
        let slots = match (list.editing, list.scene, &self.state.text_edit) {
            (Some(item), Some(scene), Some(edit)) => crate::state::inplace::slots(scene, item, &edit.fields),
            _ => Vec::new(),
        };
        let mut items = canvas::paint_items(&list);
        inplace::suppress(&mut items, &slots, &list.camera);
        canvas::paint(&painter, &items);
        // P3-311: the selected line's vertex handles.
        canvas::paint(&painter, &canvas::vertex_handle_items(&list, &self.state.vertex_handles()));
        // P4-213, P4-214: its end handles, and the snap points while one is dragged.
        canvas::paint(&painter, &canvas::anchor_items(&list, &self.state.end_handles(), &self.state.snap_marks()));
        self.paint_paper_marks(&painter, response.rect);
        // P2-708 by drag: a grab hand over a draggable label, a closed one
        // while it slides.
        if (response.hovered() || response.dragged())
            && let Some(grabbing) = self.state.label_cursor()
        {
            ui.ctx().set_cursor_icon(if grabbing {
                egui::CursorIcon::Grabbing
            } else {
                egui::CursorIcon::Grab
            });
        }
        let mut inputs = canvas::inputs(ui.ctx(), &response);
        // EDIT-5: a press on a field is the field's, not a canvas click
        // (which would end the edit).
        inplace::keep_presses_on_fields(&mut inputs, &slots, &list.camera);
        let mut out: Vec<Event> = inputs.into_iter().map(Event::Input).collect();
        // P2-723: the tooltip overlay, below the item's bottom-right corner.
        if let (Some(tip), Some(scene)) = (&list.tooltip, list.scene)
            && let Some(bounds) = scene.item_bounds(tip.item)
        {
            let corner = list.camera.to_screen(bounds.max);
            let pos = response.rect.min + egui::vec2(corner.x + 8.0, corner.y + 8.0);
            // DECISION: the tooltip takes no pointer, so the canvas
            // under it stays hovered (P2-712 clears the hover over any
            // overlay that does) and the tooltip never flickers.
            egui::Area::new(egui::Id::unique("c4rizer-tooltip"))
                .fixed_pos(pos)
                .order(egui::Order::Tooltip)
                .interactable(false)
                .show(ui.ctx(), |ui| frame::tip_card(ui, tip));
        }
        // EDIT-5: every field of the edited item typed in the shape.
        if let Some(edit) = &self.state.text_edit {
            out.extend(inplace::fields(ui, response.rect.min, &list.camera, edit, &slots, self.state.mode()));
        }
        // editor-ux-02: the right-click menu, at the pointer.
        self.menu_rect.set(self.context_menu(ui, response.rect.min, &mut out));
        out
    }

    /// True when the first view (or the start screen, or the empty canvas
    /// of a file that does not parse) is on screen (EDIT-14).
    fn first_view_ready(&self) -> bool {
        self.state.scene().is_some()
            || self.state.view.is_none()
            || self.state.doc.as_ref().is_none_or(|d| d.parsed.is_none())
    }
}

/// A waker that repaints `ctx` (worker replies, file events).
/// "Export all views" (EDIT-7): one `<folder>/<view key>.<ext>` target
/// per declared view, in declaration order.
// DECISION: a private free function, not a `State` method, so the
// frozen contract gains no item; the shell's folder answer goes through
// it and the headless test calls it directly.
pub(crate) fn export_all_targets(state: &State, folder: &Path, format: ExportFormat) -> Vec<(ViewRef, PathBuf)> {
    state
        .views()
        .into_iter()
        // P2-180: unsupported views are skipped.
        .filter(|v| matches!(v.view, ViewRef::Declared(_)) && v.kind.drawable())
        .map(|v| {
            let path = folder.join(files::export_name(&v.view.key(), format));
            (v.view, path)
        })
        .collect()
}

/// Every view as one PDF (File > Export): the targets of
/// `export_all_targets`, each naming `path`, so they become that file's
/// pages in declaration order.
pub(crate) fn export_document_targets(state: &State, path: &Path) -> Vec<(ViewRef, PathBuf)> {
    export_all_targets(state, Path::new(""), ExportFormat::Pdf)
        .into_iter()
        .map(|(view, _)| (view, path.to_path_buf()))
        .collect()
}

/// The view list's text for `entry` (EDIT-8): `<label> (<keyword>)`,
/// then ` — not yet supported` for a view whose kind is not drawn
/// (P2-123).
pub(crate) fn view_label(entry: &ViewEntry) -> String {
    if entry.kind.drawable() {
        format!("{} ({})", entry.label, entry.kind.keyword())
    } else {
        format!("{} ({}) — not yet supported", entry.label, entry.kind.keyword())
    }
}

/// The view list of the left panel (EDIT-8, P2-737, behaviour change 11):
/// one row per view, the key column first (mono, keys aligned), then the
/// title cut to the room left with an ellipsis; the full title (and
/// EDIT-8's kind, P2-123's note) as tooltip. Pushes the events its clicks
/// make; returns each row's text as drawn and rect.
fn view_list(ui: &mut egui::Ui, state: &State, out: &mut Vec<Event>) -> Vec<(String, egui::Rect)> {
    // egui stores a panel's width as its content's, so without this
    // claim the panel shrank frame after frame to its minimum.
    ui.set_min_width(ui.available_width());
    ui.spacing_mut().item_spacing.y = 0.0;
    let key_w = panels::key_column(ui, state);
    let mut rows = Vec::new();
    for entry in state.views() {
        let (line, response) = panels::view_row(ui, &entry, key_w);
        rows.push((line, response.rect));
        if response.clicked() && !entry.current {
            out.push(Event::Command(Command::ShowView(entry.view)));
        }
    }
    rows
}

/// The view kind as the paper's label names it: `C1 · SYSTEM CONTEXT`.
fn level_title(kind: ViewKind) -> &'static str {
    match kind {
        ViewKind::SystemLandscape => "system landscape",
        ViewKind::SystemContext => "system context",
        ViewKind::Container => "container",
        ViewKind::Component => "component",
        ViewKind::Filtered => "filtered",
        ViewKind::Custom => "custom",
        ViewKind::Image => "image",
        ViewKind::Dynamic => "dynamic",
        ViewKind::Deployment => "deployment",
        _ => "view",
    }
}

/// A shape as the style panel names it: the reference keyword in words,
/// first word capitalised (`MobileDevicePortrait` reads `Mobile device
/// portrait`, `Box` stays `Box`).
// DECISION: derived from `Shape::keyword` rather than a second table, so
// the 19 names cannot drift from what `edit` writes (P2-417).
fn shape_name(shape: c4rizer_core::model::Shape) -> String {
    let mut out = String::new();
    for c in shape.keyword().chars() {
        if c.is_ascii_uppercase() && !out.is_empty() {
            out.push(' ');
            out.push(c.to_ascii_lowercase());
        } else {
            out.push(c);
        }
    }
    out
}

/// The style panel's shape choice: one drop-down of every panel shape in
/// a form row (P2-736, EDIT-17). `pick` gets the shape chosen. Returns
/// the row's rect.
fn shape_row(
    ui: &mut egui::Ui,
    current: Option<c4rizer_core::model::Shape>,
    pick: &mut dyn FnMut(c4rizer_core::model::Shape),
) -> egui::Rect {
    let names: Vec<String> = crate::state::PANEL_SHAPES.iter().map(|&s| shape_name(s)).collect();
    let at = current.and_then(|c| crate::state::PANEL_SHAPES.iter().position(|&s| s == c));
    let shown = current.map_or_else(|| "differs".to_owned(), shape_name);
    let inner = ui.horizontal(|ui| {
        crate::kit::form_row(ui, "Shape", |ui| crate::kit::select(ui, "shape", &shown, &names, at))
    });
    if let Some(i) = inner.inner
        && Some(i) != at
    {
        pick(crate::state::PANEL_SHAPES[i]);
    }
    inner.response.rect
}

/// The view list as drawn headless in a left panel `width` px wide (the
/// app's panel settings) after `frames` frames (P2-737).
#[cfg(test)]
pub(crate) struct DrawnViewList {
    /// Each row's line and height, px.
    pub rows: Vec<(String, f32)>,
    /// The height of one widget row, px.
    pub row_height: f32,
    /// The panel's width after the last frame, px.
    pub panel_width: f32,
}

#[cfg(test)]
pub(crate) fn draw_view_list(state: &State, width: f32, frames: usize) -> DrawnViewList {
    let ctx = egui::Context::default();
    let mut drawn = DrawnViewList {
        rows: Vec::new(),
        row_height: 0.0,
        panel_width: 0.0,
    };
    for _ in 0..frames {
        let input = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(1600.0, 900.0))),
            ..Default::default()
        };
        let mut output = ctx.run_ui(input, |ui| {
            let panel = egui::Panel::left("c4rizer-left")
                .resizable(true)
                .default_size(width)
                .size_range(width.min(SIDE_PANEL_MIN_PX)..=width)
                .show(ui, |ui| {
                    let mut out = Vec::new();
                    let rows = view_list(ui, state, &mut out);
                    drawn.rows = rows.into_iter().map(|(l, r)| (l, r.height())).collect();
                    // redesign 2026-11-19: a view row is `size-row` high, one row.
                    drawn.row_height = theme::SIZE_ROW;
                });
            drawn.panel_width = panel.response.rect.width();
        });
        output.textures_delta.clear();
    }
    drawn
}

/// The formats of the export panel, as `c4rizer export -f` names them
/// (P3-400, P3-431), `svg` first (the default); animation is deferred
/// (P3-432). PDF is not one of them: it is in File > Export only.
pub(crate) fn export_choices() -> [(&'static str, ExportFormat); 11] {
    [
        ("svg", ExportFormat::Svg),
        ("png", ExportFormat::Png),
        ("plantuml", ExportFormat::PlantUml),
        ("plantuml/c4plantuml", ExportFormat::C4PlantUml),
        ("mermaid", ExportFormat::Mermaid),
        ("websequencediagrams", ExportFormat::WebSequenceDiagrams),
        ("dot", ExportFormat::Dot),
        ("ilograph", ExportFormat::Ilograph),
        ("json", ExportFormat::Json),
        ("theme", ExportFormat::Theme),
        ("static", ExportFormat::Static),
    ]
}

/// The SVG text choices of the editor's export, `name` first (the
/// default), as `--svg-text` names them (P2-663).
pub(crate) fn svg_text_choices() -> [(&'static str, ExportFormat); 3] {
    [
        ("name", ExportFormat::Svg),
        ("embed", ExportFormat::SvgEmbedded),
        ("outline", ExportFormat::SvgOutlined),
    ]
}

/// A crumb's text: a C4 level, then the crumb's element (the workspace
/// name for the workspace crumb), e.g. `Landscape: Shop › Container: Shop
/// › Component: Web app`; the label alone when there is no level to name
/// (P2-734). The last crumb names the level of the view on screen, so the
/// current level is always visible; an earlier crumb names the level of
/// the view a click opens.
// DECISION: the last crumb reads the open view's kind and the others the
// kind of `State::crumb_target`, which keeps the SRS example on a
// component view and tells a context view from a container view of the
// same system. The workspace crumb whose target is the view on screen
// (no landscape view, the first context view open) says `Workspace`, so
// it never repeats the last crumb's level. Needs `State`, so the stub's
// `&Crumb` grew one.
pub(crate) fn crumb_label(state: &State, n: usize, crumb: &crate::state::Crumb) -> String {
    let open = state.view.as_ref();
    let last = n + 1 == state.breadcrumb().len();
    let kind = if last {
        open.and_then(|v| state.kind_of(v))
    } else {
        let target = state.crumb_target(n);
        if n == 0 && target.is_some() && target.as_ref() == open {
            return format!("Workspace: {}", crumb.label);
        }
        target.and_then(|v| state.kind_of(&v))
    };
    match kind {
        Some(kind) => format!("{}: {}", level_name(kind), crumb.label),
        None => crumb.label.clone(),
    }
}

/// The C4 level a view kind shows, as the breadcrumb names it (P2-734).
fn level_name(kind: ViewKind) -> &'static str {
    match kind {
        ViewKind::SystemLandscape => "Landscape",
        ViewKind::SystemContext => "Context",
        ViewKind::Container => "Container",
        ViewKind::Component => "Component",
        ViewKind::Filtered => "Filtered",
        ViewKind::Custom => "Custom",
        ViewKind::Image => "Image",
        ViewKind::Dynamic => "Dynamic",
        ViewKind::Deployment => "Deployment",
        _ => "View",
    }
}

/// True when the style panel's shape choice is one drop-down, never a
/// wrapped row: drawn headless at the side panel's narrowest width, the
/// shape row (`shape_row`) is one widget row high (P2-736).
#[cfg(test)]
pub(crate) fn shape_choice_is_dropdown() -> bool {
    let ctx = egui::Context::default();
    let mut rows = (0.0_f32, 0.0_f32);
    let input = egui::RawInput {
        screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(SIDE_PANEL_MIN_PX, 400.0))),
        ..Default::default()
    };
    let mut output = ctx.run_ui(input, |ui| {
        ui.set_max_width(SIDE_PANEL_MIN_PX);
        let row = shape_row(ui, Some(c4rizer_core::model::Shape::Box), &mut |_| {});
        rows = (row.height(), ui.spacing().interact_size.y);
    });
    output.textures_delta.clear();
    rows.0 <= rows.1 + 1.0
}

/// The view list's one-line text for `entry`: its title cut to `chars`
/// characters with an ellipsis, then ` · ` and the key; and the full
/// title for the tooltip. A view without a title shows its key once
/// (P2-737).
// DECISION: ` · ` between title and key, so the line ends with the key
// as P2-737 reads; the kind keyword of EDIT-8's `view_label` moves to
// the tooltip line with the not-yet-supported note (P2-123).
pub(crate) fn view_list_line(entry: &ViewEntry, chars: usize) -> (String, String) {
    let key = entry.view.key();
    let title = entry.label.clone();
    if title == key {
        return (key, title);
    }
    let cut = if title.chars().count() > chars {
        let mut t: String = title.chars().take(chars.saturating_sub(1)).collect();
        t.push('…');
        t
    } else {
        title.clone()
    };
    (format!("{cut} · {key}"), title)
}

/// The fields of the details panel that show an edit affordance (a pencil
/// mark and a hover outline): the in-place editor's fields of the single
/// selected item, by label; none in view-only mode, for several items or
/// on a view not drawn yet (P2-739, EDIT-5).
pub(crate) fn editable_detail_fields(state: &State) -> Vec<&'static str> {
    if state.read_only() || state.selection.len() != 1 {
        return Vec::new();
    }
    let (Some(model), Some(view), Some(item)) = (state.model(), state.view.as_ref(), state.selection.iter().next())
    else {
        return Vec::new();
    };
    if state.unsupported(view) {
        return Vec::new();
    }
    crate::state::text_fields(model, view, item).iter().map(|f| f.label).collect()
}

/// The status bar's parse error text (EDIT-11, P2-136): `parse error at
/// <line>:<column>: ...` for the root file (whose path the bar already
/// shows), `parse error at <path>:<line>:<column>: ...` for an error in
/// an included file; `None` without a parse error.
pub(crate) fn problem_text(state: &State) -> Option<String> {
    let Some(Problem::Parse(e)) = state.status().problem else {
        return None;
    };
    Some(match state.parse_error_file() {
        Some(path) => format!("parse error at {}:{e}", path.display()),
        None => format!("parse error at {e}"),
    })
}

fn waker(ctx: &egui::Context) -> Wake {
    let ctx = ctx.clone();
    Arc::new(move || ctx.request_repaint())
}

/// What the startup report does this frame (EDIT-14).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Report {
    /// Nothing: no report asked, already printed, or not ready yet.
    Wait,
    /// This frame shows the first view: mark it presented and ask for one
    /// more frame.
    Present,
    /// The previous frame was presented: print and close.
    Print,
}

// DECISION: the report is printed at the start of the frame after the
// first one that shows the first view, so the measured time includes
// that frame's presentation; it slightly overstates, never understates.
fn report_step(report: bool, ready: bool, presented: bool) -> Report {
    match (report, ready, presented) {
        (true, _, true) => Report::Print,
        (true, true, false) => Report::Present,
        _ => Report::Wait,
    }
}

/// The report line: whole ms from `began` to `now`, no unit (EDIT-14).
fn report_line(began: Instant, now: Instant) -> String {
    now.saturating_duration_since(began).as_millis().to_string()
}

impl eframe::App for App {
    /// Drains worker replies and file events into the state (also while
    /// the window is hidden). Serves EDIT-1, EDIT-25.
    fn logic(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        for reply in self.worker.drain() {
            self.dispatch(ctx, Event::Worker(reply));
        }
        let events: Vec<FileEvent> = self.file_rx.try_iter().collect();
        for event in events {
            self.dispatch(ctx, Event::File(event));
        }
        let included: Vec<(PathBuf, FileEvent)> = self.include_rx.try_iter().collect();
        for (path, event) in included {
            let effects = self.state.included_file(&path, &event);
            self.run(ctx, effects);
            self.watch_includes(ctx);
        }
    }

    /// Lays out the panels and the canvas and dispatches their events;
    /// with `--startup-report`, prints the ms since `Startup::began` after
    /// the first frame that shows the first view (or the empty start
    /// screen) and closes the window (EDIT-14).
    #[expect(clippy::too_many_lines, reason = "the frame's panels top to bottom")]
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        match report_step(self.startup.report, self.first_view_ready(), self.presented) {
            Report::Wait => {}
            Report::Present => {
                self.presented = true;
                ctx.request_repaint();
            }
            Report::Print => {
                println!("{}", report_line(self.startup.began, Instant::now()));
                self.startup.report = false;
                ctx.send_viewport_cmd(egui::ViewportCommand::Close);
            }
        }
        let mut events = file_shortcuts(&ctx);
        let width = ctx.content_rect().width();
        let narrow = !side_panels_start_open(width);
        let (_, mut right) = *self.panels.get_or_insert_with(|| {
            let open = !narrow;
            // EDIT-32: a narrow window starts with the view list closed too.
            self.state.aids.view_list = open;
            (open, open)
        });
        // Behaviour change 1: crossing under 1000 px closes both panels,
        // at start and on every resize, not only at start.
        if narrow && self.last_width.is_some_and(side_panels_start_open) {
            self.state.aids.view_list = false;
            right = false;
        }
        self.last_width = Some(width);
        // P2-727: the view list panel is the state's (`n` toggles it).
        let was_left = self.state.aids.view_list;
        let mut fit = fit_key_pressed(&ctx);
        let mut dark = self.state.mode() == Mode::Dark;
        let bar = frame::BarState {
            left: was_left,
            right,
            explore: self.explore.open,
            forward: false,
        };
        let bar = egui::Panel::top("c4rizer-topbar")
            .exact_size(frame::BAR_H)
            .frame(egui::Frame::new().fill(theme::BG_PANEL))
            .show(ui, |ui| self.top_bar(ui, narrow, bar, &mut events))
            .inner;
        let (mut left, mut right) = (bar.left, bar.right);
        // Narrow: the toggles open one panel at a time.
        if narrow && left && right {
            if was_left {
                left = false;
            } else {
                right = false;
            }
        }
        // P3-9: the explore pages replace the canvas while on; behaviour
        // change 15: the right panel closes instead of dimming.
        self.explore.open = bar.explore;
        self.panels = Some((left, right));
        // DECISION: the top-bar toggle writes the view-only flag directly,
        // as the shell writes `io_error`; it changes no file (P2-730).
        self.state.aids.view_list = left;
        if bar.forward {
            self.go_forward(&ctx);
        }
        egui::Panel::bottom("c4rizer-status")
            .exact_size(theme::SIZE_ROW)
            .frame(egui::Frame::new().fill(theme::BG_PANEL))
            .show(ui, |ui| self.status_bar(ui, narrow, self.explore.open, &mut events, &mut fit, &mut dark));
        let mode = if dark { Mode::Dark } else { Mode::Light };
        if mode != self.state.mode() {
            let effects = self.state.set_mode(mode);
            self.run(&ctx, effects);
        }
        let (min, max) = side_panel_range(width);
        let panel_frame = egui::Frame::new().fill(theme::BG_PANEL);
        if left {
            egui::Panel::left("c4rizer-left")
                .resizable(true)
                .frame(panel_frame)
                .default_size(220.0_f32.clamp(min, max))
                .size_range(min..=max)
                .show(ui, |ui| events.extend(self.left_side(ui)));
        }
        // DECISION: with no file open the right panel stays shut and its
        // toggle disabled (design pack `screen-Start`): it has nothing to
        // show; the toggle's state is kept for when a file opens.
        if right && !self.explore.open && self.state.doc.is_some() {
            egui::Panel::right("c4rizer-right")
                .resizable(true)
                .frame(panel_frame)
                .default_size(240.0_f32.clamp(min, max))
                .size_range(min..=max)
                .show(ui, |ui| events.extend(self.right_panel(ui)));
        }
        // Behaviour change 6: a press outside the open menu, in a panel
        // too, closes it; on the canvas the canvas's own press does.
        if matches!(self.state.overlay, Some(crate::state::Overlay::ContextMenu { .. }))
            && let Some(menu) = self.menu_rect.get()
            && let Some(pos) = ctx.input(|i| if i.pointer.any_pressed() { i.pointer.interact_pos() } else { None })
            && !menu.contains(pos)
            && !self.canvas_rect.get().contains(pos)
        {
            events.insert(
                0,
                Event::Input(crate::state::Input::KeyDown {
                    key: crate::state::Key::Escape,
                    mods: crate::state::Modifiers::default(),
                    text_focus: false,
                }),
            );
        }
        if self.explore.open
            && let Some(parsed) = self.state.doc.as_ref().and_then(|d| d.parsed.clone())
        {
            self.explore_frame(ui, &ctx, &parsed, events);
            return;
        }
        egui::CentralPanel::default().frame(egui::Frame::new().fill(theme::BG_CANVAS)).show(ui, |ui| {
            let size = ui.available_size();
            self.canvas_size.set(ScreenPoint { x: size.x, y: size.y });
            // P2-711: the canvas size anchors the zoom keys and the fits.
            let effects = self.state.handle(Event::Input(crate::state::Input::CanvasSize {
                size: ScreenPoint { x: size.x, y: size.y },
            }));
            self.run(&ctx, effects);
            if fit {
                self.state.fit_view();
            }
            events.extend(self.canvas(ui));
        });
        events.extend(self.overlays(&ctx));
        for event in events {
            self.dispatch(&ctx, event);
        }
    }
}

/// The File menu's keys (owner request 2026-11-23): Ctrl+O opens, Ctrl+S
/// saves, Ctrl+Shift+S saves as (Cmd on macOS). Consumed, so the canvas
/// never sees them.
fn file_shortcuts(ctx: &egui::Context) -> Vec<Event> {
    use egui::{Key, KeyboardShortcut, Modifiers};
    ctx.input_mut(|i| {
        // Ctrl+Shift+S first: Ctrl+S would also match it.
        let commands = [
            (Modifiers::COMMAND | Modifiers::SHIFT, Key::S, Command::ShowSaveAsDialog),
            (Modifiers::COMMAND, Key::S, Command::Save),
            (Modifiers::COMMAND, Key::O, Command::ShowOpenDialog),
        ];
        commands
            .into_iter()
            .filter(|(mods, key, _)| i.consume_shortcut(&KeyboardShortcut::new(*mods, *key)))
            .map(|(_, _, command)| Event::Command(command))
            .collect()
    })
}

/// The panel and overlay events that pass while Explore is open (P3-507),
/// and whether Explore closes: a view pick (view list, breadcrumb) passes
/// and closes it, so that view opens; fitting, full screen and opening
/// another file pass; every other event, all that could write the file,
/// is dropped.
// DECISION: an allow-list, so a control added later stays off in Explore
// until it is listed here.
pub(crate) fn explore_gate(events: Vec<Event>) -> (Vec<Event>, bool) {
    let mut leave = false;
    let kept = events
        .into_iter()
        .filter(|event| match event {
            Event::Command(Command::ShowView(_) | Command::Crumb(_) | Command::Back) => {
                leave = true;
                true
            }
            // File menu: saving, Save as and export write no diagram edit.
            Event::Command(
                Command::Open(_)
                | Command::ShowOpenDialog
                | Command::Fit(_)
                | Command::Fullscreen
                | Command::Save
                | Command::ShowSaveAsDialog
                | Command::ExportView(_)
                | Command::ExportAll(_),
            ) => true,
            _ => false,
        })
        .collect();
    (kept, leave)
}

impl App {
    /// The explore pages in the central panel (P3-9), and the actions
    /// their clicks make: open a view with a box selected, or copy text.
    /// Nothing here writes the file (P3-507).
    fn explore_frame(
        &mut self,
        ui: &mut egui::Ui,
        ctx: &egui::Context,
        parsed: &Arc<c4rizer_core::model::Parsed>,
        mut events: Vec<Event>,
    ) {
        let actions = egui::CentralPanel::default()
            .frame(egui::Frame::new().fill(theme::BG_PANEL))
            .show(ui, |ui| {
                self.explore.mode = self.state.mode();
                self.explore.sync(parsed);
                self.explore.show(ui, &parsed.model)
            })
            .inner;
        for action in actions {
            match action {
                crate::explore::Action::Open { view, element } => {
                    self.explore.open = false;
                    let effects = self.state.reveal(view, element);
                    self.run(ctx, effects);
                }
                crate::explore::Action::Copy(text) => {
                    ctx.copy_text(text.clone());
                    self.state.notice = Some(format!("Copied {text}"));
                }
            }
        }
        events.extend(self.overlays(ctx));
        let (events, leave) = explore_gate(events);
        if leave {
            self.explore.open = false;
        }
        for event in events {
            self.dispatch(ctx, event);
        }
    }
}

#[cfg(test)]
mod tests {
    use c4rizer_core::model::ElementKey;
    use std::time::{Duration, Instant};

    use std::sync::Arc;
    use std::sync::atomic::AtomicBool;

    use c4rizer_core::model::ViewKind;

    use super::{Report, export_all_targets, report_line, report_step};
    use crate::files::{self, FileEvent, UiState};
    use crate::state::{Command, Effect, Event, State};
    use crate::testutil::{SHOP, TempDir};
    use crate::worker::{self, ExportFormat, ViewRef};

    /// Runs `effects` synchronously, as the shell would without a window.
    fn run(state: &mut State, effects: Vec<Effect>) {
        for effect in effects {
            let next = match effect {
                Effect::Read(path) => {
                    let (text, hash) = files::read(&path).expect("read fixture");
                    state.handle(Event::File(FileEvent::Loaded { path, text, hash }))
                }
                Effect::Write(request) => state.handle(Event::File(files::write_checked(&request))),
                Effect::Work(request) => match worker::serve(request, &AtomicBool::new(false)) {
                    Some(reply) => state.handle(Event::Worker(reply)),
                    None => Vec::new(),
                },
                Effect::Watch(_)
                | Effect::Dialog(_)
                | Effect::SaveUiState(_)
                | Effect::Fullscreen(_)
                | Effect::OpenUrl(_) => Vec::new(),
            };
            run(state, next);
        }
    }

    /// EDIT-7 "Export all views": the folder answer becomes one
    /// `<folder>/<view key>.<ext>` target per declared view (a drilled-in
    /// default view is not one), every declared view is written there,
    /// and a file of the same name is replaced.
    #[test]
    fn edit_7_export_all_writes_every_declared_view() {
        let dir = TempDir::new("export-all");
        let path = dir.join("shop.dsl");
        std::fs::write(&path, SHOP).expect("write fixture");
        let mut state = State::new(UiState::default());
        let effects = state.handle(Event::Command(Command::Open(path)));
        run(&mut state, effects);
        let effects = state.handle(Event::Command(Command::ShowView(ViewRef::Default {
            scope: ElementKey::from_written("web"),
            kind: ViewKind::Component,
        })));
        run(&mut state, effects);
        assert!(
            state.views().iter().any(|v| matches!(v.view, ViewRef::Default { .. })),
            "the default view is listed, so the filter is exercised"
        );

        let folder = dir.join("out");
        std::fs::create_dir_all(&folder).expect("out folder");
        let old_file = folder.join("context.svg");
        std::fs::write(&old_file, "stale").expect("stale file");

        let png = export_all_targets(&state, &folder, ExportFormat::Png);
        let png_paths: Vec<_> = png.iter().map(|(_, p)| p.clone()).collect();
        assert_eq!(png_paths, [folder.join("context.png"), folder.join("containers.png")]);

        let targets = export_all_targets(&state, &folder, ExportFormat::Svg);
        let expected = vec![
            (ViewRef::Declared("context".to_owned()), folder.join("context.svg")),
            (ViewRef::Declared("containers".to_owned()), folder.join("containers.svg")),
        ];
        assert_eq!(targets, expected);
        let effects = state.handle(Event::Command(Command::Export {
            targets,
            format: ExportFormat::Svg,
        }));
        run(&mut state, effects);

        let parsed = Arc::new(c4rizer_core::model::parse(SHOP).expect("fixture parses"));
        for (view, out) in &expected {
            let written = std::fs::read_to_string(out).expect("svg written");
            let job = worker::Job::Build {
                parsed: Arc::clone(&parsed),
                view: view.clone(),
                keep: c4rizer_core::place::Keep::default(),
                shown: true,
                mode: c4rizer_core::model::Mode::Light,
            };
            let request = worker::Request {
                generation: worker::Generation(1),
                job,
            };
            let Some(worker::Reply::Built { build, .. }) = worker::serve(request, &AtomicBool::new(false)) else {
                panic!("{view:?} builds");
            };
            assert_eq!(written, c4rizer_core::export::svg(&build.scene), "{}", out.display());
        }
        let mut names: Vec<_> = std::fs::read_dir(&folder)
            .expect("out folder")
            .map(|e| e.expect("entry").file_name().into_string().expect("utf-8"))
            .collect();
        names.sort();
        assert_eq!(names, ["containers.svg", "context.svg"], "replaced in place, nothing else");
    }

    /// EDIT-14: the startup report is printed once, after the first
    /// presented frame, as whole ms on one line.
    #[test]
    fn edit_14_startup_report_line_format() {
        let began = Instant::now();
        let line = report_line(began, began + Duration::from_micros(1_234_567));
        assert_eq!(line, "1234");
        assert!(!line.contains('\n') && line.parse::<u64>().is_ok());

        // Frames: not ready, ready (presented), next frame prints, then
        // nothing more once the report flag is cleared.
        assert_eq!(report_step(true, false, false), Report::Wait);
        assert_eq!(report_step(true, true, false), Report::Present);
        assert_eq!(report_step(true, true, true), Report::Print);
        assert_eq!(report_step(false, true, true), Report::Wait);
        assert_eq!(report_step(false, true, false), Report::Wait, "no report asked");
    }

    /// One frame of `ctx` with `events`, the UI built by `build`; returns
    /// what `fit_key_pressed` said during it.
    fn frame_with(
        ctx: &eframe::egui::Context,
        events: Vec<eframe::egui::Event>,
        build: impl Fn(&mut eframe::egui::Ui),
    ) -> bool {
        let mut pressed = false;
        let input = eframe::egui::RawInput {
            events,
            ..Default::default()
        };
        let mut output = ctx.run_ui(input, |ui| {
            build(ui);
            pressed = super::fit_key_pressed(ui.ctx());
        });
        output.textures_delta.clear();
        pressed
    }

    fn f_key(modifiers: eframe::egui::Modifiers) -> eframe::egui::Event {
        eframe::egui::Event::Key {
            key: eframe::egui::Key::F,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers,
        }
    }

    /// EDIT-30: F fits the view, but not while a text field has focus,
    /// and not with Ctrl (that chord is not "Fit view").
    #[test]
    fn edit_30_f_key_fits_unless_text_field_has_focus() {
        use eframe::egui::{Context, Id, Modifiers, TextEdit};
        let ctx = Context::default();
        assert!(frame_with(&ctx, vec![f_key(Modifiers::NONE)], |_| {}));
        assert!(!frame_with(&ctx, vec![], |_| {}), "no key, no fit");
        assert!(!frame_with(&ctx, vec![f_key(Modifiers::CTRL)], |_| {}));

        let id = Id::unique("field");
        let field = |ui: &mut eframe::egui::Ui| {
            let mut text = String::new();
            let r = ui.add(TextEdit::singleline(&mut text).id(id));
            r.request_focus();
        };
        let _ = frame_with(&ctx, vec![], field);
        assert!(
            !frame_with(&ctx, vec![f_key(Modifiers::NONE)], field),
            "typing an F"
        );
    }

    /// EDIT-32: under 1000px of window width the side panels start
    /// collapsed; from 1000px they start open.
    #[test]
    fn edit_32_side_panels_start_collapsed_under_1000px() {
        assert!(!super::side_panels_start_open(848.0));
        assert!(!super::side_panels_start_open(999.0));
        assert!(super::side_panels_start_open(1000.0));
        assert!(super::side_panels_start_open(1920.0));
    }

    /// EDIT-32: with both side panels at their widest, the canvas keeps at
    /// least 60% of the window width, at every window width.
    #[test]
    fn edit_32_canvas_keeps_60_percent_with_both_panels_wide() {
        for width in [320.0_f32, 848.0, 999.0, 1000.0, 1280.0, 1920.0, 3840.0] {
            let (min, max) = super::side_panel_range(width);
            assert!(min <= max, "{width}: {min} > {max}");
            assert!(min >= 0.0);
            let canvas = width - 2.0 * (max + super::PANEL_CHROME_PX);
            assert!(canvas >= 0.6 * width, "{width}: canvas {canvas}");
        }
    }

    /// The style panel names shapes in words (redesign 2026-11-19), one
    /// distinct name per panel shape.
    #[test]
    fn shape_names_read_as_words() {
        use c4rizer_core::model::Shape;
        assert_eq!(super::shape_name(Shape::Box), "Box");
        assert_eq!(super::shape_name(Shape::RoundedBox), "Rounded box");
        assert_eq!(super::shape_name(Shape::MobileDevicePortrait), "Mobile device portrait");
        let mut names: Vec<String> = crate::state::PANEL_SHAPES.iter().map(|&s| super::shape_name(s)).collect();
        names.sort();
        names.dedup();
        assert_eq!(names.len(), crate::state::PANEL_SHAPES.len());
    }
}

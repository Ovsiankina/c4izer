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

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::mpsc::{self, Receiver, Sender};
use std::time::Instant;

use c4rizer_core::edit::{StyleProperty, StyleTarget};
use c4rizer_core::model::{Colour, Mode, ViewKind};
use eframe::egui;

use crate::camera::ScreenPoint;
use crate::canvas;
use crate::files::{self, FileEvent, UiState, Watcher};
use crate::state::{
    Alignment, Axis, CanvasStep, Command, Dialog, Effect, Event, NavLevel, Overlay, Problem, State, ViewEntry,
};
use crate::worker::{ExportFormat, ViewRef, Wake, Worker};

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
        canvas::install_fonts(&cc.egui_ctx);
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
    fn ask(&self, dialog: Dialog) -> Option<Command> {
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
        }
    }

    /// The left panel: Open button, recent files when no file is open,
    /// breadcrumb, view list. Returns the events its clicks make.
    /// Serves EDIT-8, EDIT-13, EDIT-16.
    #[must_use]
    pub fn left_panel(&self, ui: &mut egui::Ui) -> Vec<Event> {
        let mut out = Vec::new();
        if ui.button("Open…").clicked() {
            out.push(Event::Command(Command::ShowOpenDialog));
        }
        if self.state.doc.is_none() {
            ui.separator();
            ui.strong("Recent files");
            if self.state.ui.recent.is_empty() {
                ui.label("none yet");
            }
            for path in &self.state.ui.recent {
                if ui.link(path.display().to_string()).clicked() {
                    out.push(Event::Command(Command::Open(path.clone())));
                }
            }
            return out;
        }
        ui.separator();
        ui.horizontal_wrapped(|ui| {
            for (i, crumb) in self.state.breadcrumb().iter().enumerate() {
                if i > 0 {
                    ui.label("›");
                }
                // P2-734: each crumb names a C4 level, the last the one on screen.
                if ui.link(crumb_label(&self.state, i, crumb)).clicked() {
                    out.push(Event::Command(Command::Crumb(i)));
                }
            }
        });
        ui.separator();
        ui.strong("Views");
        view_list(ui, &self.state, &mut out);
        out
    }

    /// The right panel: details, style panel (with target choice), align
    /// and distribute buttons, lint findings, export buttons. Rows wrap,
    /// so the panel fits its EDIT-32 width cap. Serves
    /// EDIT-5, EDIT-6, EDIT-7, EDIT-10, EDIT-17, EDIT-20.
    #[must_use]
    #[expect(clippy::too_many_lines, reason = "the panel's widgets top to bottom")]
    pub fn right_panel(&self, ui: &mut egui::Ui) -> Vec<Event> {
        let mut out = Vec::new();
        let mut events: Vec<Event> = Vec::new();
        let mut command = |c: Command| out.push(Event::Command(c));
        ui.horizontal_wrapped(|ui| {
            if ui.add_enabled(self.state.history.can_undo(), egui::Button::new("Undo")).clicked() {
                command(Command::Undo);
            }
            if ui.add_enabled(self.state.history.can_redo(), egui::Button::new("Redo")).clicked() {
                command(Command::Redo);
            }
        });
        // P2-700..P2-704: the canvas commands of Structurizr's editor.
        ui.horizontal_wrapped(|ui| {
            if ui.button("Auto-place").on_hover_text("Place every box afresh (one undo step)").clicked() {
                command(Command::AutoPlace);
            }
            // P2-702: Alt held keeps every box where it is.
            let alt = ui.input(|i| i.modifiers.alt);
            for (label, hover, step) in [
                ("Canvas −", "Decrease canvas size (Alt: keep positions)", CanvasStep::Decrease),
                ("Canvas auto", "Smallest canvas size that holds the diagram", CanvasStep::Auto),
                ("Canvas +", "Increase canvas size (Alt: keep positions)", CanvasStep::Increase),
            ] {
                if ui.button(label).on_hover_text(hover).clicked() {
                    command(Command::Canvas {
                        step,
                        keep_positions: alt,
                    });
                }
            }
            if ui.button("Magnet").on_hover_text("Bring boxes onto the canvas").clicked() {
                command(Command::BringOntoCanvas);
            }
            if ui.button("Centre").on_hover_text("Centre the diagram or the selected boxes").clicked() {
                command(Command::Centre);
            }
        });
        ui.separator();
        egui::ScrollArea::vertical().show(ui, |ui| {
            if let Some(d) = self.state.details() {
                // P2-739: the in-place editor's fields show a pencil and
                // a hover outline; a double click opens the editor (as
                // Enter does, EDIT-5) on the field clicked.
                let editable = editable_detail_fields(&self.state);
                let mut field = |ui: &mut egui::Ui, label: &'static str, text: egui::WidgetText| {
                    let Some(at) = editable.iter().position(|&f| f == label) else {
                        ui.label(text);
                        return;
                    };
                    let r = ui
                        .horizontal_wrapped(|ui| {
                            ui.label(text);
                            ui.weak("✏");
                        })
                        .response
                        .interact(egui::Sense::click());
                    let r = r.on_hover_text(format!("Double click or Enter to edit the {label}"));
                    if r.hovered() {
                        ui.painter().rect_stroke(
                            r.rect.expand(2.0),
                            2.0,
                            ui.visuals().widgets.hovered.bg_stroke,
                            egui::StrokeKind::Outside,
                        );
                    }
                    if r.double_clicked() {
                        events.push(Event::Input(crate::state::Input::KeyDown {
                            key: crate::state::Key::Enter,
                            mods: crate::state::Modifiers::default(),
                            text_focus: false,
                        }));
                        for _ in 0..at {
                            events.push(Event::Input(crate::state::Input::KeyDown {
                                key: crate::state::Key::Tab,
                                mods: crate::state::Modifiers::default(),
                                text_focus: false,
                            }));
                        }
                    }
                };
                field(ui, "name", egui::RichText::new(&d.name).strong().into());
                ui.label(&d.kind);
                if let Some(t) = &d.technology {
                    field(ui, "technology", format!("Technology: {t}").into());
                }
                // P3-131: a deployment node's instances and environment.
                if let Some(n) = &d.instances {
                    ui.label(format!("Instances: {n}"));
                }
                if let Some(env) = &d.environment {
                    ui.label(format!("Environment: {env}"));
                }
                if let Some(text) = &d.description {
                    field(ui, "description", text.into());
                }
                if !d.tags.is_empty() {
                    ui.label(format!("Tags: {}", d.tags.join(", ")));
                }
                if let Some(from) = &d.lifted_from {
                    ui.label(format!("Lifted from: {from}"));
                }
                for other in &d.merged {
                    ui.label(format!("Also: {other}"));
                }
                ui.separator();
            }
            if let Some(panel) = self.state.style_panel() {
                style_panel(ui, &panel, &mut command);
            }
            ui.strong("Lint");
            let findings = self.state.findings();
            let notices = self.state.notices();
            if findings.is_empty() && notices.is_empty() {
                ui.label("no findings");
            }
            for (i, f) in findings.iter().enumerate() {
                if ui.selectable_label(false, &f.message).clicked() {
                    command(Command::PickFinding(i));
                }
            }
            // P2-126: notices name the construct, its file and line; they
            // select nothing.
            for notice in &notices {
                ui.label(format!("notice: {notice}"));
            }
            ui.separator();
            ui.strong("Export");
            // P2-663: the SVG text choice, `name` by default; it lives in
            // egui's memory and never reaches the diagram file (P2-665).
            let id = egui::Id::unique("c4rizer-svg-text");
            let choices = svg_text_choices();
            let mut pick = ui.ctx().data(|d| d.get_temp::<usize>(id)).unwrap_or(0).min(choices.len() - 1);
            // The label sits before a narrow combo, wrapping under it in
            // a narrow panel; a label after the combo wrapped letter by
            // letter (seen in the real window).
            ui.horizontal_wrapped(|ui| {
                ui.label("SVG text");
                egui::ComboBox::from_id_salt("c4rizer-svg-text-combo")
                    .width(70.0)
                    .selected_text(choices[pick].0)
                    .show_ui(ui, |ui| {
                        for (i, (label, _)) in choices.iter().enumerate() {
                            ui.selectable_value(&mut pick, i, *label);
                        }
                    });
            });
            ui.ctx().data_mut(|d| d.insert_temp(id, pick));
            let svg = choices[pick].1;
            ui.horizontal_wrapped(|ui| {
                if ui.button("View as SVG").clicked() {
                    command(Command::ExportView(svg));
                }
                if ui.button("View as PNG").clicked() {
                    command(Command::ExportView(ExportFormat::Png));
                }
            });
            ui.horizontal_wrapped(|ui| {
                if ui.button("All views as SVG").clicked() {
                    command(Command::ExportAll(svg));
                }
                if ui.button("All views as PNG").clicked() {
                    command(Command::ExportAll(ExportFormat::Png));
                }
            });
        });
        out.extend(events);
        out
    }

    /// The status bar from `State::status`: file path, problem (parse error with line and
    /// column, missing file, I/O error), notice, read-only flag. Serves
    /// EDIT-11, EDIT-19, EDIT-24, EDIT-27.
    pub fn status_bar(&self, ui: &mut egui::Ui) {
        let status = self.state.status();
        ui.horizontal_wrapped(|ui| {
            match &self.state.doc {
                Some(doc) => ui.label(doc.path.display().to_string()),
                None => ui.label("no file open"),
            };
            match &status.problem {
                Some(Problem::Parse(_)) => {
                    let text = problem_text(&self.state).unwrap_or_default();
                    ui.colored_label(egui::Color32::RED, text);
                }
                Some(Problem::Missing) => {
                    ui.colored_label(egui::Color32::RED, "the file is gone; showing the last diagram");
                }
                Some(Problem::Io(e)) => {
                    ui.colored_label(egui::Color32::RED, e);
                }
                None => {}
            }
            if let Some(notice) = &status.notice {
                ui.label(notice);
            }
            if self.state.doc.is_some() && self.state.read_only() {
                ui.strong("read-only");
            }
        });
    }

    /// The centre: the canvas (`canvas::paint_items`, `canvas::paint`),
    /// the in-place text field over the edited item (its changes as
    /// `Event::Text`), and canvas input. Returns the events. Serves EDIT-5, EDIT-9, EDIT-12, EDIT-28.
    #[must_use]
    pub fn canvas(&self, ui: &mut egui::Ui) -> Vec<Event> {
        let (response, painter) = ui.allocate_painter(ui.available_size(), egui::Sense::click_and_drag());
        let painter = painter.with_clip_rect(response.rect);
        let list = self.state.draw_list();
        // EDIT-31: the page in the export background first; outside it
        // the central panel keeps the app's panel colour.
        // P2-701: the paper of a view with a canvas size, under the page.
        canvas::paint_page(&painter, &list, self.state.paper_rect());
        canvas::paint(&painter, &canvas::paint_items(&list));
        let mut out: Vec<Event> = canvas::inputs(ui.ctx(), &response).into_iter().map(Event::Input).collect();
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
                .show(ui.ctx(), |ui| {
                    egui::Frame::popup(ui.style()).show(ui, |ui| {
                        let d = &tip.details;
                        ui.strong(&d.name);
                        ui.label(&d.kind);
                        if let Some(t) = &d.technology {
                            ui.label(format!("Technology: {t}"));
                        }
                        if let Some(text) = &d.description {
                            ui.label(text);
                        }
                        if !d.tags.is_empty() {
                            ui.label(format!("Tags: {}", d.tags.join(", ")));
                        }
                        if let Some(url) = &tip.url {
                            ui.label(format!("URL: {url}"));
                        }
                        for (name, value) in &tip.properties {
                            ui.label(format!("{name}: {value}"));
                        }
                        if tip.sticky {
                            ui.weak("Escape closes");
                        }
                    });
                });
        }
        if let (Some(item), Some(scene), Some(edit)) = (list.editing, list.scene, &self.state.text_edit)
            && let (Some(bounds), Some(field)) = (scene.item_bounds(item), edit.fields.get(edit.current))
        {
            let a = list.camera.to_screen(bounds.min);
            let b = list.camera.to_screen(bounds.max);
            let origin = response.rect.min;
            let width = (b.x - a.x).max(160.0);
            let top = origin.y + f32::midpoint(a.y, b.y) - 12.0;
            let rect = egui::Rect::from_min_size(egui::pos2(origin.x + a.x, top), egui::vec2(width, 24.0));
            let mut buffer = field.buffer.clone();
            let id = egui::Id::unique(("c4rizer-text-edit", edit.current));
            let edited = ui.put(
                rect,
                egui::TextEdit::singleline(&mut buffer)
                    .id(id)
                    .hint_text(field.label)
                    .desired_width(width),
            );
            if !edited.has_focus() {
                edited.request_focus();
            }
            if buffer != field.buffer {
                out.push(Event::Text(buffer));
            }
        }
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

/// The style panel's widgets (EDIT-17, EDIT-20).
fn style_panel(ui: &mut egui::Ui, panel: &crate::state::StylePanel, command: &mut impl FnMut(Command)) {
    ui.strong(format!("Style ({} box{})", panel.boxes, if panel.boxes == 1 { "" } else { "es" }));
    // DECISION: the target choice lives in egui's temporary memory
    // (index 0: this box only, i: the i-th shared tag), so the panel
    // stays a pure reader of `State`.
    let id = egui::Id::unique("c4rizer-style-target");
    let mut choice = ui.ctx().data(|d| d.get_temp::<usize>(id)).unwrap_or(0);
    if choice > panel.tags.len() {
        choice = 0;
    }
    let name = |i: usize| {
        if i == 0 {
            "this box only".to_owned()
        } else {
            format!("all {}", panel.tags[i - 1])
        }
    };
    egui::ComboBox::from_id_salt("c4rizer-style-target-combo")
        .selected_text(name(choice))
        .show_ui(ui, |ui| {
            for i in 0..=panel.tags.len() {
                ui.selectable_value(&mut choice, i, name(i));
            }
        });
    ui.ctx().data_mut(|d| d.insert_temp(id, choice));
    let target = if choice == 0 {
        StyleTarget::ThisBox
    } else {
        StyleTarget::Tag(panel.tags[choice - 1].clone())
    };
    // DECISION: every colour-picker change is written at once (one
    // undo step each): the canvas follows the picker live (EDIT-17).
    let mut colour_row = |ui: &mut egui::Ui, label: &str, value: Option<Colour>, make: fn(Option<Colour>) -> StyleProperty| {
        ui.horizontal_wrapped(|ui| {
            ui.label(label);
            let mut rgb = value.map_or([128, 128, 128], |c| [c.r, c.g, c.b]);
            if ui.color_edit_button_srgb(&mut rgb).changed() {
                command(Command::SetStyle {
                    target: target.clone(),
                    property: make(Some(Colour {
                        r: rgb[0],
                        g: rgb[1],
                        b: rgb[2],
                    })),
                });
            }
            if value.is_none() {
                ui.label("(differs)");
            }
        });
    };
    colour_row(ui, "Fill", panel.background, StyleProperty::Background);
    colour_row(ui, "Text", panel.color, StyleProperty::Color);
    // P2-736: one drop-down, never a wrapped row of buttons.
    shape_row(ui, panel.shape, &mut |shape| {
        command(Command::SetStyle {
            target: target.clone(),
            property: StyleProperty::Shape(Some(shape)),
        });
    });
    if ui.button("Reset to tag style").clicked() {
        command(Command::ResetStyle);
    }
    if panel.boxes > 1 {
        ui.horizontal_wrapped(|ui| {
            for (label, how) in [
                ("Left", Alignment::Left),
                ("Centre", Alignment::Centre),
                ("Right", Alignment::Right),
                ("Top", Alignment::Top),
                ("Middle", Alignment::Middle),
                ("Bottom", Alignment::Bottom),
            ] {
                if ui.button(label).clicked() {
                    command(Command::Align(how));
                }
            }
        });
        ui.horizontal_wrapped(|ui| {
            if ui.button("Distribute horizontally").clicked() {
                command(Command::Distribute(Axis::Horizontal));
            }
            if ui.button("Distribute vertically").clicked() {
                command(Command::Distribute(Axis::Vertical));
            }
        });
    }
    ui.separator();
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

/// The view list of the left panel (EDIT-8, P2-737): one row per view,
/// the title cut to the panel's width with an ellipsis, then the key; the
/// full title (and EDIT-8's kind, P2-123's note) as tooltip. Pushes the
/// events its clicks make; returns each row's line and rect.
fn view_list(ui: &mut egui::Ui, state: &State, out: &mut Vec<Event>) -> Vec<(String, egui::Rect)> {
    // egui stores a panel's width as its content's, and the cut below
    // follows the width it is given, so without this claim the panel
    // shrank frame after frame to its minimum.
    ui.set_min_width(ui.available_width());
    let mut rows = Vec::new();
        // P2-737: one line per view, the title cut to the panel's width,
        // the full title (and EDIT-8's kind, P2-123's note) as tooltip.
        // DECISION: the cut is measured with the button font: the longest
        // title prefix whose line fits the panel less the label's padding
        // (a character estimate wrapped in the real window); one
        // character is always kept.
        let room = ui.available_width() - 2.0 * ui.spacing().button_padding.x - 2.0;
        let font = egui::TextStyle::Button.resolve(ui.style());
        let ctx = ui.ctx().clone();
        let fits = |line: &str| {
            ctx.fonts_mut(|f| f.layout_no_wrap(line.to_owned(), font.clone(), egui::Color32::WHITE).size().x)
                <= room
        };
        for entry in state.views() {
            let full = entry.label.chars().count();
            let budget = (1..=full).rev().find(|&n| fits(&view_list_line(&entry, n).0)).unwrap_or(1);
            let (line, title) = view_list_line(&entry, budget);
            let line_text = line.clone();
            let tip = format!("{title}\n{}", view_label(&entry));
            let text = if entry.kind.drawable() {
                egui::RichText::new(line)
            } else {
                egui::RichText::new(line).weak()
            };
            // DECISION: the button truncates rather than wraps, so a key
            // wider than the panel (the cut keeps the key whole) still
            // stays on its one row, elided at the end.
            let response = ui
                .add(egui::Button::selectable(entry.current, text).truncate())
                .on_hover_text(tip);
            rows.push((line_text, response.rect));
            if response.clicked() && !entry.current {
                out.push(Event::Command(Command::ShowView(entry.view)));
            }
        }
    rows
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
                    drawn.row_height = ui.spacing().interact_size.y;
                });
            drawn.panel_width = panel.response.rect.width();
        });
        output.textures_delta.clear();
    }
    drawn
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

/// The style panel's shape choice: one drop-down of every panel shape
/// (P2-736, EDIT-17). `pick` gets the shape chosen. Returns the row's
/// rect.
fn shape_row(
    ui: &mut egui::Ui,
    current: Option<c4rizer_core::model::Shape>,
    pick: &mut dyn FnMut(c4rizer_core::model::Shape),
) -> egui::Rect {
    ui.horizontal(|ui| {
        ui.label("Shape");
        let mut chosen = current;
        // Fills the rest of the row, so the row never outgrows the panel.
        egui::ComboBox::from_id_salt("c4rizer-shape-combo")
            .width((ui.available_width() - 2.0 * ui.spacing().icon_width).max(48.0))
            .selected_text(current.map_or("(differs)", |s| s.keyword()))
            .show_ui(ui, |ui| {
                for shape in crate::state::PANEL_SHAPES {
                    ui.selectable_value(&mut chosen, Some(shape), shape.keyword());
                }
            });
        if let Some(shape) = chosen
            && chosen != current
        {
            pick(shape);
        }
    })
    .response
    .rect
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
        let mut events = Vec::new();
        let width = ctx.content_rect().width();
        let (_, mut right) = *self.panels.get_or_insert_with(|| {
            let open = side_panels_start_open(width);
            // EDIT-32: a narrow window starts with the view list closed too.
            self.state.aids.view_list = open;
            (open, open)
        });
        // P2-727: the view list panel is the state's (`n` toggles it).
        let mut left = self.state.aids.view_list;
        let mut fit = fit_key_pressed(&ctx);
        let mut dark = self.state.mode() == Mode::Dark;
        egui::Panel::top("c4rizer-toolbar").show(ui, |ui| {
            ui.horizontal(|ui| {
                ui.toggle_value(&mut left, "Views");
                if ui.button("Fit view").on_hover_text("F").clicked() {
                    fit = true;
                }
                // P2-705: fit width, height, content (`w`, `h`, `c`).
                for (label, key, how) in [
                    ("Fit width", "W", crate::camera::Fit::Width),
                    ("Fit height", "H", crate::camera::Fit::Height),
                ] {
                    if ui.button(label).on_hover_text(key).clicked() {
                        events.push(Event::Command(Command::Fit(how)));
                    }
                }
                // TODO(srs): P2-706, the key. DECISION: reading taken (B
                // editor-breadth 1): F11 and this toolbar item; `f` stays
                // EDIT-30's Fit view.
                if ui
                    .selectable_label(self.state.aids.fullscreen, "Fullscreen")
                    .on_hover_text("F11")
                    .clicked()
                {
                    events.push(Event::Command(Command::Fullscreen));
                }
                // P2-433: rebuilds the shown view in that mode; writes
                // nothing to the file.
                ui.toggle_value(&mut dark, "Dark")
                    .on_hover_text("Show and export the diagram in dark mode");
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    ui.toggle_value(&mut right, "Details");
                });
            });
        });
        self.panels = Some((left, right));
        // DECISION: the toolbar toggle writes the view-only flag directly,
        // as the shell writes `io_error`; it changes no file (P2-730).
        self.state.aids.view_list = left;
        let mode = if dark { Mode::Dark } else { Mode::Light };
        if mode != self.state.mode() {
            let effects = self.state.set_mode(mode);
            self.run(&ctx, effects);
        }
        egui::Panel::bottom("c4rizer-status").show(ui, |ui| self.status_bar(ui));
        let (min, max) = side_panel_range(width);
        if left {
            egui::Panel::left("c4rizer-left")
                .resizable(true)
                .default_size(220.0_f32.clamp(min, max))
                .size_range(min..=max)
                .show(ui, |ui| events.extend(self.left_panel(ui)));
        }
        if right {
            egui::Panel::right("c4rizer-right")
                .resizable(true)
                .default_size(280.0_f32.clamp(min, max))
                .size_range(min..=max)
                .show(ui, |ui| events.extend(self.right_panel(ui)));
        }
        egui::CentralPanel::default().show(ui, |ui| {
            let size = ui.available_size();
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

impl App {
    /// The list open over the canvas (P2-715 quick navigation, P2-725
    /// tag filter, P2-726 perspectives, P2-729 navigator), as a window;
    /// returns the events its widgets make.
    #[expect(clippy::too_many_lines, reason = "one flat arm per overlay window")]
    fn overlays(&self, ctx: &egui::Context) -> Vec<Event> {
        let mut out = Vec::new();
        let Some(overlay) = self.state.overlay.clone() else {
            return out;
        };
        match overlay {
            Overlay::QuickNavigation { query, highlighted } => {
                egui::Window::new("Quick navigation")
                    .collapsible(false)
                    .resizable(false)
                    .anchor(egui::Align2::CENTER_TOP, egui::vec2(0.0, 40.0))
                    .show(ctx, |ui| {
                        let mut buffer = query.clone();
                        let field = ui.add(egui::TextEdit::singleline(&mut buffer).hint_text("view key or title"));
                        if !field.has_focus() {
                            field.request_focus();
                        }
                        if buffer != query {
                            out.push(Event::Text(buffer));
                        }
                        for (i, entry) in self.state.quick_nav_entries().into_iter().enumerate() {
                            if ui.selectable_label(i == highlighted, view_label(&entry)).clicked() {
                                out.push(Event::Command(Command::ShowView(entry.view)));
                            }
                        }
                    });
            }
            Overlay::TagFilter => {
                egui::Window::new("Filter by tag")
                    .collapsible(false)
                    .resizable(false)
                    .anchor(egui::Align2::RIGHT_TOP, egui::vec2(-20.0, 40.0))
                    .show(ctx, |ui| {
                        let chosen = &self.state.aids.tags;
                        let choices = self.state.tag_choices();
                        if choices.is_empty() {
                            ui.label("no user-defined tags in this view");
                        }
                        for tag in choices {
                            let mut on = chosen.contains(&tag);
                            if ui.checkbox(&mut on, &tag).changed() {
                                let mut next: Vec<String> = chosen.iter().filter(|t| **t != tag).cloned().collect();
                                if on {
                                    next.push(tag.clone());
                                }
                                out.push(Event::Command(Command::TagFilter(next)));
                            }
                        }
                        ui.horizontal(|ui| {
                            if ui.button("Clear").clicked() {
                                out.push(Event::Command(Command::TagFilter(Vec::new())));
                            }
                            if ui.button("Close").clicked() {
                                out.push(Event::Command(Command::TagFilterPanel));
                            }
                        });
                    });
            }
            Overlay::Perspectives => {
                egui::Window::new("Perspectives")
                    .collapsible(false)
                    .resizable(false)
                    .anchor(egui::Align2::RIGHT_TOP, egui::vec2(-20.0, 40.0))
                    .show(ctx, |ui| {
                        let current = self.state.aids.perspective.clone();
                        if ui.selectable_label(current.is_none(), "none").clicked() {
                            out.push(Event::Command(Command::Perspective(None)));
                        }
                        for name in self.state.perspective_choices() {
                            if ui.selectable_label(current.as_deref() == Some(name.as_str()), &name).clicked() {
                                out.push(Event::Command(Command::Perspective(Some(name))));
                            }
                        }
                        if ui.button("Close").clicked() {
                            out.push(Event::Command(Command::PerspectivePanel));
                        }
                    });
            }
            Overlay::Navigator => {
                egui::Window::new("Diagram navigator")
                    .collapsible(false)
                    .resizable(false)
                    .anchor(egui::Align2::CENTER_TOP, egui::vec2(0.0, 40.0))
                    .show(ctx, |ui| {
                        // P2-729: the search field filters all three groups.
                        // DECISION: it takes focus like quick navigation's,
                        // so typing searches at once; Escape still closes.
                        let query = self.state.nav_query();
                        let mut buffer = query.to_owned();
                        let field = ui.add(egui::TextEdit::singleline(&mut buffer).hint_text("search views"));
                        if !field.has_focus() {
                            field.request_focus();
                        }
                        if buffer != query {
                            out.push(Event::Text(buffer));
                        }
                        let entries = self.state.navigator();
                        for (level, title) in [
                            (NavLevel::Above, "Above"),
                            (NavLevel::NextTo, "Next to"),
                            (NavLevel::Below, "Below"),
                        ] {
                            ui.strong(title);
                            let mut any = false;
                            for (_, entry) in entries.iter().filter(|(l, _)| *l == level) {
                                any = true;
                                if ui.link(view_label(entry)).clicked() {
                                    out.push(Event::Command(Command::ShowView(entry.view.clone())));
                                }
                            }
                            if !any {
                                ui.weak("none");
                            }
                        }
                        if ui.button("Close").clicked() {
                            out.push(Event::Command(Command::Navigator));
                        }
                    });
            }
        }
        out
    }
}

#[cfg(test)]
mod tests {
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
            scope: "web".to_owned(),
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
}

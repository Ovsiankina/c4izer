//! The eframe shell: panel layout (view list and breadcrumb left, canvas
//! centre, details and style panel plus lint findings right, status bar),
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
use c4rizer_core::model::{Colour, Shape};
use eframe::egui;

use crate::canvas;
use crate::files::{self, FileEvent, UiState, Watcher};
use crate::state::{Alignment, Axis, Command, Dialog, Effect, Event, Problem, State};
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
        let mut app = App {
            state: State::new(ui),
            worker,
            watcher: None,
            file_tx,
            file_rx,
            ui_state_path,
            startup,
            presented: false,
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
                if ui.link(&crumb.label).clicked() {
                    out.push(Event::Command(Command::Crumb(i)));
                }
            }
        });
        ui.separator();
        ui.strong("Views");
        for entry in self.state.views() {
            let label = format!("{} ({})", entry.label, entry.kind.keyword());
            if ui.selectable_label(entry.current, label).clicked() && !entry.current {
                out.push(Event::Command(Command::ShowView(entry.view)));
            }
        }
        out
    }

    /// The right panel: details, style panel (with target choice), align
    /// and distribute buttons, lint findings, export buttons. Serves
    /// EDIT-5, EDIT-6, EDIT-7, EDIT-10, EDIT-17, EDIT-20.
    #[must_use]
    pub fn right_panel(&self, ui: &mut egui::Ui) -> Vec<Event> {
        let mut out = Vec::new();
        let mut command = |c: Command| out.push(Event::Command(c));
        ui.horizontal(|ui| {
            if ui.add_enabled(self.state.history.can_undo(), egui::Button::new("Undo")).clicked() {
                command(Command::Undo);
            }
            if ui.add_enabled(self.state.history.can_redo(), egui::Button::new("Redo")).clicked() {
                command(Command::Redo);
            }
        });
        ui.separator();
        egui::ScrollArea::vertical().show(ui, |ui| {
            if let Some(d) = self.state.details() {
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
            if findings.is_empty() {
                ui.label("no findings");
            }
            for (i, f) in findings.iter().enumerate() {
                if ui.selectable_label(false, &f.message).clicked() {
                    command(Command::PickFinding(i));
                }
            }
            ui.separator();
            ui.strong("Export");
            ui.horizontal(|ui| {
                if ui.button("View as SVG").clicked() {
                    command(Command::ExportView(ExportFormat::Svg));
                }
                if ui.button("View as PNG").clicked() {
                    command(Command::ExportView(ExportFormat::Png));
                }
            });
            ui.horizontal(|ui| {
                if ui.button("All views as SVG").clicked() {
                    command(Command::ExportAll(ExportFormat::Svg));
                }
                if ui.button("All views as PNG").clicked() {
                    command(Command::ExportAll(ExportFormat::Png));
                }
            });
        });
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
                Some(Problem::Parse(e)) => {
                    ui.colored_label(egui::Color32::RED, format!("parse error at {e}"));
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
        canvas::paint(&painter, &canvas::paint_items(&list));
        let mut out: Vec<Event> = canvas::inputs(ui.ctx(), &response).into_iter().map(Event::Input).collect();
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
        ui.horizontal(|ui| {
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
    ui.horizontal(|ui| {
        ui.label("Shape");
        for shape in [Shape::Box, Shape::Person, Shape::Cylinder] {
            if ui.selectable_label(panel.shape == Some(shape), format!("{shape:?}")).clicked() {
                command(Command::SetStyle {
                    target: target.clone(),
                    property: StyleProperty::Shape(Some(shape)),
                });
            }
        }
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
        ui.horizontal(|ui| {
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
fn export_all_targets(state: &State, folder: &Path, format: ExportFormat) -> Vec<(ViewRef, PathBuf)> {
    state
        .views()
        .into_iter()
        .filter(|v| matches!(v.view, ViewRef::Declared(_)))
        .map(|v| {
            let path = folder.join(files::export_name(&v.view.key(), format));
            (v.view, path)
        })
        .collect()
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
        egui::Panel::bottom("c4rizer-status").show(ui, |ui| self.status_bar(ui));
        egui::Panel::left("c4rizer-left")
            .default_size(220.0)
            .show(ui, |ui| events.extend(self.left_panel(ui)));
        egui::Panel::right("c4rizer-right")
            .default_size(280.0)
            .show(ui, |ui| events.extend(self.right_panel(ui)));
        egui::CentralPanel::default().show(ui, |ui| events.extend(self.canvas(ui)));
        for event in events {
            self.dispatch(&ctx, event);
        }
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
                Effect::Watch(_) | Effect::Dialog(_) | Effect::SaveUiState(_) => Vec::new(),
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
}

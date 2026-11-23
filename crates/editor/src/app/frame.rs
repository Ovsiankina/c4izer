//! The window frame (redesign 2026-11-19, `guides/01-app-frame.md`): the
//! top bar (where am I, what did I change, which mode), the status bar
//! (how am I looking at it), the canvas backdrop with its paper marks,
//! the start screen and the overlays.

use c4rizer_core::geom::{Point, Rect as SceneRect};
use eframe::egui::{self, Align2, Color32, CornerRadius, Pos2, Rect, Sense, Stroke, Vec2};

use super::{App, crumb_label, problem_text, view_label};
use crate::icons::{self, Icon};
use crate::kit::{self, Button, Look, Segment};
use crate::state::{Command, Event, MenuEntry, NavLevel, Overlay, Problem};
use crate::worker::ExportFormat;
use crate::theme::{
    self, ACCENT, BG_CANVAS, BG_OVERLAY, BG_RAISED, DANGER, INK, INK_2, INK_3, INK_DISABLED, LINE, LINE_2, OK, PAD,
    SIZE_BAR, SIZE_CONTROL, SIZE_ROW, WARN,
};

/// What the top bar's toggles were set to this frame.
#[expect(clippy::struct_excessive_bools, reason = "independent toggles of the top bar")]
pub(super) struct BarState {
    /// The views panel.
    pub left: bool,
    /// The details panel.
    pub right: bool,
    /// Explore on.
    pub explore: bool,
    /// Forward was clicked.
    pub forward: bool,
}

/// A vertical hairline at `x` over the bar `rect`.
fn rule(ui: &egui::Ui, x: f32, rect: Rect) {
    let x = x.round() + 0.5;
    ui.painter()
        .line_segment([egui::pos2(x, rect.min.y), egui::pos2(x, rect.max.y)], Stroke::new(1.0, LINE));
}

/// A top-bar cell: 8 px padding, then `add`, then a hairline.
fn cell<R>(ui: &mut egui::Ui, bar: Rect, rtl: bool, add: impl FnOnce(&mut egui::Ui) -> R) -> R {
    ui.add_space(8.0);
    let inner = ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 4.0;
        add(ui)
    });
    ui.add_space(8.0);
    let x = if rtl { inner.response.rect.min.x - 8.0 } else { inner.response.rect.max.x + 8.0 };
    rule(ui, x, bar);
    inner.inner
}

impl App {
    /// The top bar: Views toggle · File, Settings menus · file · Back, Forward · breadcrumb ·
    /// Undo, Redo · `Diagram / Explore` · Details toggle.
    pub(super) fn top_bar(&self, ui: &mut egui::Ui, narrow: bool, mut bar: BarState, events: &mut Vec<Event>) -> BarState {
        let rect = ui.max_rect();
        ui.horizontal_centered(|ui| {
            ui.spacing_mut().item_spacing.x = 0.0;
            cell(ui, rect, false, |ui| {
                let r = Button::icon(Icon::PanelLeft).look(Look::Ghost).on(bar.left).show(ui);
                if kit::tip(r, "Views panel", Some("N")).clicked() {
                    bar.left = !bar.left;
                }
            });
            cell(ui, rect, false, |ui| {
                file_menu(ui, &self.state, events);
                settings_menu(ui, &self.state, events);
            });
            cell(ui, rect, false, |ui| {
                let name = self
                    .state
                    .doc
                    .as_ref()
                    .map_or_else(|| "Open…".to_owned(), |d| d.path.file_name().map_or_else(|| d.path.display().to_string(), |n| n.to_string_lossy().into_owned()));
                let font = theme::ui_font();
                let text = kit::elide(ui, &name, &font, 200.0 - 32.0);
                let w = 4.0 + 16.0 + 8.0 + ui.fonts_mut(|f| f.layout_no_wrap(text.clone(), font.clone(), INK).size().x) + 4.0;
                let (r, response) = ui.allocate_exact_size(Vec2::new(w, SIZE_CONTROL), Sense::click());
                if response.hovered() {
                    ui.painter().rect_filled(r, CornerRadius::same(2), BG_RAISED);
                }
                icons::paint(ui.painter(), Icon::Open, Rect::from_min_size(egui::pos2(r.min.x + 4.0, r.center().y - 8.0), Vec2::splat(16.0)), INK_2);
                ui.painter().text(egui::pos2(r.min.x + 28.0, r.center().y), Align2::LEFT_CENTER, text, font, INK);
                let path = self.state.doc.as_ref().map_or_else(|| "Open a workspace".to_owned(), |d| d.path.display().to_string());
                let response = kit::tip(response, &path, Some("Ctrl+O"));
                if response.clicked() {
                    events.push(Event::Command(Command::ShowOpenDialog));
                }
                kit::focus_ring(ui, &response);
            });
            cell(ui, rect, false, |ui| {
                let r = Button::icon(Icon::ChevronLeft).look(Look::Ghost).enabled(self.state.can_go_back()).show(ui);
                if kit::tip(r, "Back to the previous view", Some("Alt+Left")).clicked() {
                    events.push(Event::Command(Command::Back));
                }
                let r = Button::icon(Icon::ChevronRight).look(Look::Ghost).enabled(self.state.can_go_forward()).show(ui);
                if kit::tip(r, "Forward to the view Back left", Some("Alt+Right")).clicked() {
                    bar.forward = true;
                }
            });
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                ui.spacing_mut().item_spacing.x = 0.0;
                ui.add_space(8.0);
                let details = !bar.explore && self.state.doc.is_some();
                let r = Button::icon(Icon::PanelRight).look(Look::Ghost).on(bar.right && details).enabled(details).show(ui);
                if kit::tip(r, "Details panel", None).clicked() {
                    bar.right = !bar.right;
                }
                ui.add_space(8.0);
                rule(ui, ui.min_rect().min.x, rect);
                cell(ui, rect, true, |ui| {
                    let mut items = if narrow {
                        [Segment::icon(Icon::Diagram, "Diagram"), Segment::icon(Icon::Explore, "Explore")]
                    } else {
                        [
                            Segment::text("Diagram", "Diagram").with_icon(Icon::Diagram),
                            Segment::text("Explore", "Model, relationships, graph, tree, documentation and decisions").with_icon(Icon::Explore),
                        ]
                    };
                    items[0] = items[0].on(!bar.explore);
                    items[1] = items[1].on(bar.explore);
                    if self.state.doc.as_ref().is_none_or(|d| d.parsed.is_none()) {
                        items[1] = items[1].enabled(false);
                    }
                    match kit::segmented(ui, &items, false) {
                        Some(0) => bar.explore = false,
                        Some(1) => bar.explore = true,
                        _ => {}
                    }
                });
                cell(ui, rect, true, |ui| {
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        let r = Button::icon(Icon::Redo).look(Look::Ghost).enabled(self.state.history.can_redo()).show(ui);
                        if kit::tip(r, "Redo", Some("Ctrl+Shift+Z")).clicked() {
                            events.push(Event::Command(Command::Redo));
                        }
                        let r = Button::icon(Icon::Undo).look(Look::Ghost).enabled(self.state.history.can_undo()).show(ui);
                        if kit::tip(r, "Undo", Some("Ctrl+Z")).clicked() {
                            events.push(Event::Command(Command::Undo));
                        }
                    });
                });
                ui.with_layout(egui::Layout::left_to_right(egui::Align::Center), |ui| {
                    ui.add_space(PAD);
                    self.crumbs(ui, narrow, events);
                });
            });
        });
        bar
    }

    /// The breadcrumb: each crumb its C4 level as a label, then its name;
    /// the last is where you are. Narrow: `…` then the last (P2-734).
    fn crumbs(&self, ui: &mut egui::Ui, narrow: bool, events: &mut Vec<Event>) {
        let crumbs = self.state.breadcrumb();
        if crumbs.is_empty() {
            return;
        }
        ui.set_clip_rect(ui.max_rect().intersect(ui.clip_rect()));
        ui.spacing_mut().item_spacing.x = 8.0;
        let last = crumbs.len() - 1;
        let first = if narrow && last > 0 { last } else { 0 };
        if first > 0 {
            if crumb(ui, None, "…", false).clicked() {
                events.push(Event::Command(Command::Crumb(last - 1)));
            }
            separator(ui);
        }
        for (i, c) in crumbs.iter().enumerate().skip(first) {
            if i > first {
                separator(ui);
            }
            let text = crumb_label(&self.state, i, c);
            let (level, name) = match text.split_once(": ") {
                Some((level, name)) => (Some(level.to_owned()), name.to_owned()),
                None => (None, text.clone()),
            };
            if crumb(ui, level.as_deref(), &name, i == last).clicked() && i != last {
                events.push(Event::Command(Command::Crumb(i)));
            }
        }
    }

    /// The status bar from `State::status`: live dot and path · problem ·
    /// notice · read-only · lint count · fit note · camera group · Dark,
    /// Fullscreen. It never wraps; narrow, the notice, lint count and fit
    /// note drop and the path shortens to the file name. Serves EDIT-11,
    /// EDIT-19, EDIT-24, EDIT-27, P2-433, P2-705, P2-706.
    #[expect(clippy::too_many_lines, reason = "the bar's cells left to right")]
    #[expect(clippy::too_many_arguments, reason = "the bar's inputs and the two toggles it may flip")]
    pub(super) fn status_bar(&self, ui: &mut egui::Ui, narrow: bool, explore: bool, events: &mut Vec<Event>, fit: &mut bool, dark: &mut bool) {
        let rect = ui.max_rect();
        let status = self.state.status();
        let code = theme::code_font();
        ui.horizontal_centered(|ui| {
            ui.spacing_mut().item_spacing.x = 0.0;
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                ui.spacing_mut().item_spacing.x = 0.0;
                // Dark, Fullscreen.
                ui.add_space(4.0);
                let r = Button::icon(Icon::Fullscreen).look(Look::Ghost).on(self.state.aids.fullscreen).show(ui);
                if kit::tip(r, "Fullscreen", Some("F11")).clicked() {
                    events.push(Event::Command(Command::Fullscreen));
                }
                let r = Button::icon(Icon::Dark).look(Look::Ghost).on(*dark).show(ui);
                // P2-433: rebuilds the shown view in that mode; writes
                // nothing to the file.
                if kit::tip(r, "Dark diagram: show and export the diagram dark", None).clicked() {
                    *dark = !*dark;
                }
                ui.add_space(4.0);
                rule(ui, ui.min_rect().min.x, rect);
                let canvas_on = self.state.doc.is_some() && !explore;
                if canvas_on {
                    // Zoom − % +.
                    ui.add_space(4.0);
                    let key = |k| {
                        Event::Input(crate::state::Input::KeyDown {
                            key: k,
                            mods: crate::state::Modifiers::default(),
                            text_focus: false,
                        })
                    };
                    if kit::tip(Button::icon(Icon::Plus).look(Look::Ghost).show(ui), "Zoom in", Some("+")).clicked() {
                        events.push(key(crate::state::Key::Plus));
                    }
                    let zoom = format!("{:.0}%", self.state.camera.zoom * 100.0);
                    let (r, _) = ui.allocate_exact_size(Vec2::new(44.0, SIZE_CONTROL), Sense::hover());
                    ui.painter().text(r.center(), Align2::CENTER_CENTER, zoom, code.clone(), INK);
                    if kit::tip(Button::icon(Icon::Minus).look(Look::Ghost).show(ui), "Zoom out", Some("-")).clicked() {
                        events.push(key(crate::state::Key::Minus));
                    }
                    ui.add_space(4.0);
                    rule(ui, ui.min_rect().min.x, rect);
                    // The fits (EDIT-30, P2-705).
                    ui.add_space(4.0);
                    if kit::tip(Button::icon(Icon::FitHeight).look(Look::Ghost).show(ui), "Fit height", Some("H")).clicked() {
                        events.push(Event::Command(Command::Fit(crate::camera::Fit::Height)));
                    }
                    if kit::tip(Button::icon(Icon::FitWidth).look(Look::Ghost).show(ui), "Fit width", Some("W")).clicked() {
                        events.push(Event::Command(Command::Fit(crate::camera::Fit::Width)));
                    }
                    if kit::tip(Button::icon(Icon::FitView).look(Look::Ghost).show(ui), "Fit view", Some("F")).clicked() {
                        *fit = true;
                    }
                    ui.add_space(4.0);
                    rule(ui, ui.min_rect().min.x, rect);
                }
                let text_cell = |ui: &mut egui::Ui, icon: Option<(Icon, Color32)>, text: &str, colour: Color32| {
                    ui.add_space(PAD);
                    ui.label(theme::code(text, colour));
                    if let Some((icon, c)) = icon {
                        ui.add_space(8.0);
                        let (r, _) = ui.allocate_exact_size(Vec2::splat(16.0), Sense::hover());
                        icons::paint(ui.painter(), icon, r, c);
                    }
                    ui.add_space(PAD);
                    rule(ui, ui.min_rect().min.x, rect);
                };
                if canvas_on && !narrow && self.fit_capped() {
                    text_cell(ui, None, "fit stops at 9 px text", INK_2);
                }
                if self.state.doc.is_some() && !narrow {
                    let n = self.state.findings().len();
                    if n == 0 {
                        text_cell(ui, None, "no findings", INK_2);
                    } else {
                        let text = format!("{n} finding{}", if n == 1 { "" } else { "s" });
                        text_cell(ui, Some((Icon::Warn, WARN)), &text, WARN);
                    }
                }
                // P3-420: a workspace JSON says why it is read-only.
                if explore {
                    text_cell(ui, Some((Icon::Lock, INK_2)), "Explore is read-only", INK_2);
                } else if self.state.json_doc() {
                    text_cell(ui, Some((Icon::Lock, INK_2)), "read-only: workspace JSON", INK_2);
                } else if self.state.doc.is_some() && self.state.read_only() {
                    text_cell(ui, Some((Icon::Lock, INK_2)), "read-only", INK_2);
                }
                if let Some(notice) = &status.notice
                    && !narrow
                {
                    text_cell(ui, None, notice, INK_2);
                }
                let problem = match &status.problem {
                    Some(Problem::Parse(_)) => problem_text(&self.state).map(|t| format!("{t} — showing the last diagram")),
                    Some(Problem::Missing) => Some("the file is gone; showing the last diagram".to_owned()),
                    Some(Problem::Io(e)) => Some(e.clone()),
                    None => None,
                };
                if let Some(text) = problem {
                    text_cell(ui, Some((Icon::Warn, DANGER)), &text, DANGER);
                }
                // Live dot and path, filling the rest.
                ui.with_layout(egui::Layout::left_to_right(egui::Align::Center), |ui| {
                    ui.set_clip_rect(ui.max_rect().intersect(ui.clip_rect()));
                    ui.add_space(PAD);
                    // DECISION: `awaited_build` only filters stale replies and stays set,
                    // so routing reads as a drag reply awaited or a view not drawn yet.
                    let routing = self.state.awaited_drag.is_some() || (self.state.view.is_some() && self.state.scene().is_none());
                    let dot = match &self.state.doc {
                        None => INK_DISABLED,
                        Some(_) if routing => ACCENT,
                        Some(_) => OK,
                    };
                    let (r, _) = ui.allocate_exact_size(Vec2::new(6.0, 6.0), Sense::hover());
                    ui.painter().circle_filled(r.center(), 3.0, dot);
                    ui.add_space(8.0);
                    let text = match &self.state.doc {
                        None => "no file open".to_owned(),
                        Some(_) if routing => "routing".to_owned(),
                        Some(doc) if narrow => doc.path.file_name().map_or_else(|| doc.path.display().to_string(), |n| n.to_string_lossy().into_owned()),
                        Some(doc) => doc.path.display().to_string(),
                    };
                    let room = ui.available_width() - PAD;
                    ui.label(theme::code(kit::elide(ui, &text, &code, room), INK_2));
                });
            });
        });
    }

    /// True when the camera sits at the fit's floor (9 px text, P2-738)
    /// and the view does not fit the canvas there (behaviour change 7).
    fn fit_capped(&self) -> bool {
        let Some(scene) = self.state.scene() else { return false };
        let cam = self.state.camera;
        if (cam.zoom - crate::camera::MIN_FIT_ZOOM).abs() > 1e-3 {
            return false;
        }
        let a = cam.to_screen(scene.bounds.min);
        let b = cam.to_screen(scene.bounds.max);
        let canvas = self.canvas_size.get();
        b.x - a.x > canvas.x + 1.0 || b.y - a.y > canvas.y + 1.0
    }

    /// The canvas backdrop: `bg-canvas` with a 20 px dot grid in `line-2`
    /// that moves with the pan.
    pub(super) fn paint_backdrop(&self, painter: &egui::Painter, rect: Rect) {
        painter.rect_filled(rect, CornerRadius::ZERO, BG_CANVAS);
        let origin = self.state.camera.to_screen(Point::new(0, 0));
        let step = 20.0;
        let ox = rect.min.x + origin.x.rem_euclid(step);
        let oy = rect.min.y + origin.y.rem_euclid(step);
        // DECISION: the dots are one unfeathered mesh, each snapped to a
        // whole pixel: a feathered 1 px rect fades to nearly nothing on a
        // pixel boundary, so the grid vanished on the start screen.
        let mut mesh = egui::Mesh::default();
        let mut y = oy;
        while y < rect.max.y {
            let mut x = ox;
            while x < rect.max.x {
                mesh.add_colored_rect(Rect::from_min_size(egui::pos2(x.floor(), y.floor()), Vec2::splat(1.0)), LINE_2);
                x += step;
            }
            y += step;
        }
        painter.add(egui::Shape::mesh(mesh));
    }

    /// The marks outside the paper: corner ticks, the view's key and kind
    /// above it, the paper size below (`components/Paper`).
    pub(super) fn paint_paper_marks(&self, painter: &egui::Painter, rect: Rect) {
        let Some(scene) = self.state.scene() else { return };
        let page = crate::canvas::page_rect(scene.bounds, self.state.paper_rect());
        let cam = self.state.camera;
        let top_left = cam.to_screen(page.min);
        let bottom_right = cam.to_screen(page.max);
        let r = Rect::from_min_max(
            rect.min + Vec2::new(top_left.x, top_left.y),
            rect.min + Vec2::new(bottom_right.x, bottom_right.y),
        );
        let tick = Stroke::new(1.0, theme::LINE_STRONG);
        // Each tick: 8 px out from the paper's corner, 8 px long.
        let (offset, length) = (8.0, 8.0);
        for (corner, dx, dy) in [
            (r.left_top(), -1.0, -1.0),
            (r.right_top(), 1.0, -1.0),
            (r.left_bottom(), -1.0, 1.0),
            (r.right_bottom(), 1.0, 1.0),
        ] {
            let at = corner + Vec2::new(dx * offset, dy * offset);
            painter.line_segment([at, at - Vec2::new(dx * length, 0.0)], tick);
            painter.line_segment([at, at - Vec2::new(0.0, dy * length)], tick);
        }
        let label = |text: String| -> egui::text::LayoutJob {
            let mut job = egui::text::LayoutJob::default();
            job.append(
                &text.to_uppercase(),
                0.0,
                egui::TextFormat {
                    font_id: theme::label_font(),
                    color: INK_3,
                    extra_letter_spacing: 0.6,
                    ..Default::default()
                },
            );
            job
        };
        if let Some(view) = &self.state.view {
            let kind = self.state.kind_of(view).map(super::level_title).unwrap_or_default();
            let mut text = format!("{}  ·  {kind}", view.key());
            if matches!(self.state.status().problem, Some(Problem::Parse(_) | Problem::Missing)) {
                text.push_str("  ·  last good");
            }
            let g = painter.layout_job(label(text));
            painter.galley(egui::pos2(r.min.x + 16.0, r.min.y - 6.0 - g.size().y), g, INK_3);
        }
        let w = page.max.x - page.min.x;
        let h = page.max.y - page.min.y;
        let g = painter.layout_job(label(format!("paper {w} × {h}")));
        painter.galley(egui::pos2(r.max.x - 16.0 - g.size().x, r.max.y + 6.0), g, INK_3);
    }

    /// The start screen in the canvas: `Open…` and the recent files, name
    /// first, folder in mono (EDIT-13, behaviour change 10).
    pub(super) fn start_screen(&self, ui: &mut egui::Ui) -> Vec<Event> {
        let mut out = Vec::new();
        let rect = ui.max_rect();
        let width = 488.0_f32.min(rect.width() - 32.0);
        let recent = &self.state.ui.recent;
        #[allow(clippy::cast_precision_loss)]
        let height = 24.0 + 32.0 + 24.0 + 24.0 + 24.0 + 12.0 + 36.0 * recent.len().max(1) as f32 + 24.0;
        let top = (rect.center().y - height / 2.0).max(rect.min.y + 24.0);
        let area = Rect::from_min_size(egui::pos2(rect.center().x - width / 2.0, top), Vec2::new(width, height));
        let mut ui = ui.new_child(egui::UiBuilder::new().max_rect(area.shrink(24.0)).layout(egui::Layout::top_down(egui::Align::Min)));
        ui.spacing_mut().item_spacing.y = 0.0;
        ui.label(
            egui::RichText::new("Open a workspace")
                .font(egui::FontId::new(28.0, egui::FontFamily::Name(theme::MEDIUM.into())))
                .extra_letter_spacing(-0.5)
                .color(INK),
        );
        ui.add_space(24.0);
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 8.0;
            let r = Button::new("Open…").with_icon(Icon::Open).look(Look::Primary).show(ui);
            if kit::tip(r, "Open a workspace", Some("Ctrl+O")).clicked() {
                out.push(Event::Command(Command::ShowOpenDialog));
            }
            ui.label(egui::RichText::new("or start with a path:").color(INK_2));
            ui.label(theme::code("c4rizer-editor FILE", INK_2));
        });
        ui.add_space(24.0);
        ui.label(theme::label("Recent files"));
        ui.add_space(8.0);
        kit_rule(&mut ui, LINE_2);
        if recent.is_empty() {
            let (r, _) = ui.allocate_exact_size(Vec2::new(ui.available_width(), 36.0), Sense::hover());
            ui.painter().text(egui::pos2(r.min.x + 8.0, r.center().y), Align2::LEFT_CENTER, "none yet", theme::ui_font(), INK_3);
            kit_rule(&mut ui, LINE_2);
        }
        for path in recent {
            if recent_row(&mut ui, path).clicked() {
                out.push(Event::Command(Command::Open(path.clone())));
            }
            kit_rule(&mut ui, LINE_2);
        }
        out
    }

    /// The right-click menu at the pointer: a label naming the item, then
    /// its entries (editor-ux-02, behaviour change 5).
    pub(super) fn context_menu(&self, ui: &egui::Ui, origin: Pos2, out: &mut Vec<Event>) -> Option<Rect> {
        let Some(Overlay::ContextMenu { entries }) = &self.state.overlay else { return None };
        let at = origin + Vec2::new(self.state.menu_at.x, self.state.menu_at.y);
        let details = self.state.details();
        let relationship = details.as_ref().is_some_and(|d| d.kind == "Relationship");
        // The design's head: the box's name, or `Relationship`.
        let head = details.map(|d| if relationship { d.kind } else { d.name });
        let rows = menu_rows(entries, relationship);
        let shown = egui::Area::new(egui::Id::unique("c4rizer-context-menu"))
            .fixed_pos(at)
            .order(egui::Order::Foreground)
            .show(ui.ctx(), |ui| {
                kit::overlay_frame(ui).show(ui, |ui| {
                    // As wide as its widest row (8 + icon 16 + 8 + text + 8),
                    // at least 200 px (`.c4-menu`), not the screen's width.
                    let font = theme::ui_font();
                    let widest = rows
                        .iter()
                        .map(|r| ui.fonts_mut(|f| f.layout_no_wrap(r.text.clone(), font.clone(), INK).size().x))
                        .fold(0.0_f32, f32::max);
                    ui.set_width((widest + 40.0).clamp(192.0, 480.0));
                    ui.spacing_mut().item_spacing.y = 0.0;
                    if let Some(head) = &head {
                        egui::Frame::new().inner_margin(egui::Margin::symmetric(8, 4)).show(ui, |ui| {
                            ui.label(theme::label(head));
                        });
                    }
                    for row in &rows {
                        if row.separated {
                            ui.add_space(4.0);
                            kit_rule(ui, LINE_2);
                            ui.add_space(4.0);
                        }
                        let r = kit::menu_row(ui, Some(row.icon), &row.text, false, row.entry.is_some());
                        if let Some(entry) = row.entry
                            && r.clicked()
                        {
                            out.push(Event::Command(Command::ContextAction(entry.action.clone())));
                        }
                    }
                });
            });
        Some(shown.response.rect)
    }

    /// The lists open over the canvas (P2-715 quick navigation, P2-725
    /// tag filter, P2-726 perspectives, P2-729 navigator); returns the
    /// events their widgets make.
    #[expect(clippy::too_many_lines, reason = "one flat arm per overlay window")]
    pub(super) fn overlays(&self, ctx: &egui::Context) -> Vec<Event> {
        let mut out = Vec::new();
        let Some(overlay) = self.state.overlay.clone() else {
            return out;
        };
        let canvas = self.canvas_rect.get();
        match overlay {
            Overlay::QuickNavigation { query, highlighted } => {
                let width = 480.0_f32.min(canvas.width() - 32.0);
                let pos = egui::pos2(canvas.center().x - width / 2.0, canvas.min.y + 56.0);
                egui::Area::new(egui::Id::unique("c4rizer-quick-nav"))
                    .fixed_pos(pos)
                    .order(egui::Order::Foreground)
                    .show(ctx, |ui| {
                        kit::overlay_frame(ui).inner_margin(egui::Margin::ZERO).show(ui, |ui| {
                            ui.set_width(width);
                            ui.spacing_mut().item_spacing.y = 0.0;
                            egui::Frame::new().inner_margin(egui::Margin::same(4)).show(ui, |ui| {
                                ui.horizontal(|ui| {
                                    ui.add_space(12.0);
                                    let (r, _) = ui.allocate_exact_size(Vec2::new(16.0, 36.0), Sense::hover());
                                    icons::paint(ui.painter(), Icon::Search, Rect::from_center_size(r.center(), Vec2::splat(16.0)), INK_3);
                                    ui.add_space(4.0);
                                    let mut buffer = query.clone();
                                    let field = ui.add(
                                        egui::TextEdit::singleline(&mut buffer)
                                            .hint_text(egui::RichText::new("view key or title").color(INK_3))
                                            .font(egui::FontId::proportional(13.0))
                                            .frame(egui::Frame::NONE)
                                            .desired_width(f32::INFINITY)
                                            .align(egui::Align2::LEFT_CENTER)
                                            .min_size(Vec2::new(0.0, 36.0)),
                                    );
                                    if !field.has_focus() {
                                        field.request_focus();
                                    }
                                    if buffer != query {
                                        out.push(Event::Text(buffer));
                                    }
                                });
                            });
                            kit_rule(ui, LINE_2);
                            egui::Frame::new().inner_margin(egui::Margin::same(4)).show(ui, |ui| {
                                let entries = self.state.quick_nav_entries();
                                if entries.is_empty() {
                                    ui.add_space(8.0);
                                    ui.horizontal(|ui| {
                                        ui.add_space(8.0);
                                        ui.label(egui::RichText::new("no view matches").color(INK_3));
                                    });
                                    ui.add_space(8.0);
                                }
                                for (i, entry) in entries.into_iter().enumerate() {
                                    let (r, response) = ui.allocate_exact_size(Vec2::new(ui.available_width(), SIZE_ROW), Sense::click());
                                    // `components/Palette`: the row under the
                                    // keyboard (or pointer) is `bg-raised`; the
                                    // open view keeps its `accent-soft`.
                                    let lit = i == highlighted || response.hovered();
                                    if entry.current {
                                        ui.painter().rect_filled(r, CornerRadius::same(2), theme::ACCENT_SOFT);
                                    } else if lit {
                                        ui.painter().rect_filled(r, CornerRadius::same(2), BG_RAISED);
                                    }
                                    let key = entry.view.key();
                                    let code = theme::code_font();
                                    let g = ui.painter().layout_no_wrap(key.clone(), code, if entry.current { ACCENT } else { INK });
                                    let kw = g.size().x;
                                    ui.painter().galley(egui::pos2(r.min.x + 12.0, r.center().y - g.size().y / 2.0), g, INK);
                                    let font = theme::ui_font();
                                    let title = if entry.label == key { String::new() } else { entry.label.clone() };
                                    let room = r.max.x - 12.0 - (r.min.x + 12.0 + kw + 8.0);
                                    ui.painter().text(
                                        egui::pos2(r.min.x + 12.0 + kw + 8.0, r.center().y),
                                        Align2::LEFT_CENTER,
                                        kit::elide(ui, &title, &font, room),
                                        font,
                                        if lit || entry.current { INK } else { INK_2 },
                                    );
                                    if response.on_hover_text(view_label(&entry)).clicked() {
                                        out.push(Event::Command(Command::ShowView(entry.view)));
                                    }
                                }
                            });
                            kit_rule(ui, LINE_2);
                            let (r, _) = ui.allocate_exact_size(Vec2::new(ui.available_width(), SIZE_ROW), Sense::hover());
                            let mut foot = ui.new_child(
                                egui::UiBuilder::new()
                                    .max_rect(r.shrink2(Vec2::new(12.0, 0.0)))
                                    .layout(egui::Layout::left_to_right(egui::Align::Center)),
                            );
                            foot.spacing_mut().item_spacing.x = 4.0;
                            kit::kbd(&mut foot, "↑");
                            kit::kbd(&mut foot, "↓");
                            foot.label(theme::code("move", INK_3));
                            foot.add_space(12.0);
                            kit::kbd(&mut foot, "Enter");
                            foot.label(theme::code("open", INK_3));
                            foot.add_space(12.0);
                            kit::kbd(&mut foot, "Esc");
                            foot.label(theme::code("close", INK_3));
                        });
                    });
            }
            // Drawn over the canvas at the pointer (`App::canvas`).
            Overlay::ContextMenu { .. } => {}
            Overlay::TagFilter => {
                small_window(ctx, canvas, "Filter by tag", &mut out, &Command::TagFilterPanel, |ui, out| {
                    let chosen = &self.state.aids.tags;
                    let choices = self.state.tag_choices();
                    if choices.is_empty() {
                        ui.label(egui::RichText::new("no user-defined tags in this view").color(INK_2));
                    }
                    for tag in choices {
                        let mut on = chosen.contains(&tag);
                        if kit::checkbox(ui, &mut on, &tag).changed() {
                            let mut next: Vec<String> = chosen.iter().filter(|t| **t != tag).cloned().collect();
                            if on {
                                next.push(tag.clone());
                            }
                            out.push(Event::Command(Command::TagFilter(next)));
                        }
                    }
                    Some(("Clear", Command::TagFilter(Vec::new())))
                });
            }
            Overlay::Perspectives => {
                small_window(ctx, canvas, "Perspectives", &mut out, &Command::PerspectivePanel, |ui, out| {
                    let current = self.state.aids.perspective.clone();
                    if kit::menu_row(ui, None, "none", current.is_none(), true).clicked() {
                        out.push(Event::Command(Command::Perspective(None)));
                    }
                    for name in self.state.perspective_choices() {
                        let on = current.as_deref() == Some(name.as_str());
                        if kit::menu_row(ui, None, &name, on, true).clicked() {
                            out.push(Event::Command(Command::Perspective(Some(name))));
                        }
                    }
                    None
                });
            }
            Overlay::Navigator => {
                small_window(ctx, canvas, "Diagram navigator", &mut out, &Command::Navigator, |ui, out| {
                    // P2-729: the search field filters all three groups.
                    // DECISION: it takes focus like quick navigation's, so
                    // typing searches at once; Escape still closes.
                    let query = self.state.nav_query();
                    let mut buffer = query.to_owned();
                    let field = ui.scope(|ui| {
                        kit::field_style(ui);
                        ui.add(
                            egui::TextEdit::singleline(&mut buffer)
                                .hint_text(egui::RichText::new("search views").color(INK_3))
                                .desired_width(f32::INFINITY)
                                .align(egui::Align2::LEFT_CENTER)
                                .min_size(Vec2::new(0.0, SIZE_CONTROL)),
                        )
                    });
                    if !field.inner.has_focus() {
                        field.inner.request_focus();
                    }
                    if buffer != query {
                        out.push(Event::Text(buffer));
                    }
                    let entries = self.state.navigator();
                    for (level, title) in [(NavLevel::Above, "Above"), (NavLevel::NextTo, "Next to"), (NavLevel::Below, "Below")] {
                        ui.add_space(4.0);
                        ui.label(theme::label(title));
                        let mut any = false;
                        for (_, entry) in entries.iter().filter(|(l, _)| *l == level) {
                            any = true;
                            if kit::menu_row(ui, None, &view_label(entry), false, true).clicked() {
                                out.push(Event::Command(Command::ShowView(entry.view.clone())));
                            }
                        }
                        if !any {
                            ui.label(egui::RichText::new("none").color(INK_3));
                        }
                    }
                    None
                });
            }
        }
        out
    }
}

/// One row of the context menu: what it says, its icon, the entry it
/// runs (`None`: shown disabled), and whether a rule comes before it.
#[derive(Debug, PartialEq)]
pub(super) struct MenuRow<'a> {
    pub text: String,
    pub icon: Icon,
    pub entry: Option<&'a MenuEntry>,
    pub separated: bool,
}

/// The context menu's rows for `entries` (behaviour change 5,
/// `components/Menu`): every entry the item can have, in groups split by
/// a rule, an entry that cannot run now shown disabled so the menu keeps
/// its shape. A box: `Enter …` | `Open URL`. A line: `Release start`,
/// `Release end` | `Group as bus`, or `Ungroup bus` and `Remove from
/// bus` | `Bridge on the other line` | `Open URL`.
// DECISION: `State` offers only the entries that can run (and opens the
// menu on a box or line even when there are none, owner decision
// 2026-11-19), so the disabled rows are filled in here from the labels
// `State` gives. "Group as bus" replaces the
// ungroup pair when offered, as the two never apply together; the bridge
// row shows only at a crossing, where the pointer decides it. An entry
// no slot names is still shown, last, so nothing is ever dropped.
pub(super) fn menu_rows(entries: &[MenuEntry], relationship: bool) -> Vec<MenuRow<'_>> {
    // (label, text when disabled, icon); an empty group list is skipped.
    type Slot = (&'static str, &'static str, Icon);
    let enter: Slot = ("Enter", "Enter a view", Icon::Enter);
    let url: Slot = ("Open URL", "Open URL", Icon::External);
    let matches = |slot: &str, e: &MenuEntry| e.label == slot || e.label.starts_with(&format!("{slot} "));
    let has = |slot: &str| entries.iter().any(|e| matches(slot, e));
    let groups: Vec<Vec<Slot>> = if relationship {
        let bus = if has("Group as bus") {
            vec![("Group as bus", "Group as bus", Icon::Bus)]
        } else {
            vec![("Ungroup bus", "Ungroup bus", Icon::Bus), ("Remove from bus", "Remove from bus", Icon::Minus)]
        };
        let bridge = if has("Bridge on the other line") {
            vec![("Bridge on the other line", "Bridge on the other line", Icon::Routing)]
        } else {
            Vec::new()
        };
        vec![
            vec![("Release start", "Release start", Icon::Reset), ("Release end", "Release end", Icon::Reset)],
            bus,
            bridge,
            vec![url],
        ]
    } else {
        vec![vec![enter], vec![url]]
    };
    let mut rows = Vec::new();
    let mut used = vec![false; entries.len()];
    for group in groups.iter().filter(|g| !g.is_empty()) {
        let mut separated = !rows.is_empty();
        for &(slot, disabled, icon) in group {
            let mut any = false;
            for (i, e) in entries.iter().enumerate().filter(|(_, e)| matches(slot, e)) {
                used[i] = true;
                any = true;
                rows.push(MenuRow { text: e.label.clone(), icon, entry: Some(e), separated });
                separated = false;
            }
            if !any {
                rows.push(MenuRow { text: disabled.to_owned(), icon, entry: None, separated });
                separated = false;
            }
        }
    }
    for (e, _) in entries.iter().zip(&used).filter(|(_, u)| !**u) {
        rows.push(MenuRow { text: e.label.clone(), icon: Icon::ChevronRight, entry: Some(e), separated: false });
    }
    rows
}

/// A small window over the canvas, top right, 12 px in (`components/
/// Window`): a 28 px title bar with a close button, the body, and an
/// optional footer button.
#[expect(clippy::too_many_arguments, reason = "where, title, the close command, then the body")]
fn small_window(
    ctx: &egui::Context,
    canvas: Rect,
    title: &str,
    out: &mut Vec<Event>,
    close: &Command,
    body: impl FnOnce(&mut egui::Ui, &mut Vec<Event>) -> Option<(&'static str, Command)>,
) {
    let width = 280.0;
    let pos = egui::pos2(canvas.max.x - 12.0 - width, canvas.min.y + 12.0);
    egui::Area::new(egui::Id::unique(("c4rizer-window", title)))
        .fixed_pos(pos)
        .order(egui::Order::Foreground)
        .show(ctx, |ui| {
            kit::overlay_frame(ui).inner_margin(egui::Margin::ZERO).show(ui, |ui| {
                ui.set_width(width);
                ui.spacing_mut().item_spacing.y = 0.0;
                let (r, _) = ui.allocate_exact_size(Vec2::new(width, SIZE_ROW), Sense::hover());
                ui.painter().text(egui::pos2(r.min.x + 12.0, r.center().y), Align2::LEFT_CENTER, title, theme::strong_font(), INK);
                let mut bar = ui.new_child(
                    egui::UiBuilder::new()
                        .max_rect(Rect::from_min_max(egui::pos2(r.max.x - 4.0 - SIZE_CONTROL, r.min.y + 2.0), egui::pos2(r.max.x - 4.0, r.max.y - 2.0)))
                        .layout(egui::Layout::left_to_right(egui::Align::Center)),
                );
                if kit::tip(Button::icon(Icon::Close).look(Look::Ghost).show(&mut bar), "Close", Some("Esc")).clicked() {
                    out.push(Event::Command(close.clone()));
                }
                kit_rule(ui, LINE_2);
                let foot = egui::Frame::new()
                    .inner_margin(egui::Margin::symmetric(12, 8))
                    .show(ui, |ui| {
                        ui.set_width(width - 24.0);
                        ui.spacing_mut().item_spacing.y = 4.0;
                        body(ui, out)
                    })
                    .inner;
                if let Some((label, command)) = foot {
                    kit_rule(ui, LINE_2);
                    egui::Frame::new().inner_margin(egui::Margin::symmetric(12, 8)).show(ui, |ui| {
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            if Button::new(label).show(ui).clicked() {
                                out.push(Event::Command(command));
                            }
                        });
                    });
                }
            });
        });
}

/// A breadcrumb item: level label then name; hover lifts it unless it is
/// where you are.
/// The `File` button and its menu (owner request 2026-11-23): Export ▸
/// (the current view or the whole workspace, as SVG, PNG or PDF),
/// Print (not yet), Save, Save as…, Open…, Open recent ▸ (the start
/// screen's recent-file rows).
pub(super) fn file_menu(ui: &mut egui::Ui, state: &crate::state::State, events: &mut Vec<Event>) {
    let r = Button::new("File").look(Look::Ghost).show(ui);
    let r = kit::tip(r, "File menu", None);
    kit::focus_ring(ui, &r);
    let doc = state.doc.is_some();
    let view = doc && state.view.is_some() && state.doc.as_ref().is_some_and(|d| d.parsed.is_some());
    let parsed = state.doc.as_ref().is_some_and(|d| d.parsed.is_some());
    let svg = super::panels::svg_choice(ui.ctx());
    egui::Popup::menu(&r).show(|ui| {
        ui.set_min_width(240.0);
        ui.spacing_mut().item_spacing.y = 0.0;
        let export = file_row(ui, "Export", None, true, parsed);
        // Nothing to export without a parsed file: the row is greyed and
        // opens nothing.
        if parsed {
            egui::containers::menu::SubMenu::new().show(ui, &export, |ui| {
                ui.set_min_width(240.0);
                ui.spacing_mut().item_spacing.y = 0.0;
                menu_label(ui, "Current view");
                for (text, format) in [("SVG", svg), ("PNG", ExportFormat::Png), ("PDF", ExportFormat::Pdf)] {
                    if file_row(ui, text, None, false, view).clicked() {
                        events.push(Event::Command(Command::ExportView(format)));
                    }
                }
                menu_separator(ui);
                menu_label(ui, "Whole workspace");
                for (text, format) in [
                    ("SVG, one file per view", svg),
                    ("PNG, one file per view", ExportFormat::Png),
                    ("PDF, one page per view", ExportFormat::Pdf),
                ] {
                    if file_row(ui, text, None, false, parsed).clicked() {
                        events.push(Event::Command(Command::ExportAll(format)));
                    }
                }
            });
        }
        // Not built yet: shown so the menu has its final shape.
        file_row(ui, "Print… (coming soon)", None, false, false);
        menu_separator(ui);
        if file_row(ui, "Save", Some("Ctrl+S"), false, doc).clicked() {
            events.push(Event::Command(Command::Save));
        }
        if file_row(ui, "Save as…", Some("Ctrl+Shift+S"), false, doc).clicked() {
            events.push(Event::Command(Command::ShowSaveAsDialog));
        }
        menu_separator(ui);
        if file_row(ui, "Open…", Some("Ctrl+O"), false, true).clicked() {
            events.push(Event::Command(Command::ShowOpenDialog));
        }
        let recent = &state.ui.recent;
        let open_recent = file_row(ui, "Open recent", None, true, true);
        egui::containers::menu::SubMenu::new().show(ui, &open_recent, |ui| {
            ui.set_min_width(320.0);
            ui.spacing_mut().item_spacing.y = 0.0;
            if recent.is_empty() {
                file_row(ui, "none yet", None, false, false);
            }
            for path in recent {
                if recent_row(ui, path).clicked() {
                    events.push(Event::Command(Command::Open(path.clone())));
                    ui.close();
                }
            }
        });
    });
}

/// The `Settings` button beside `File` and its menu (owner request
/// 2026-11-23): the open view's settings, for now its crossing budget
/// (`maxCrossings`, CORE-17) with the count the view draws.
// DECISION: per view, as the DSL writes the budget (owner, 2026-11-23:
// a budget shared by the file makes no sense across views). The popup
// stays open on clicks inside, so the budget can be stepped and its
// effect read off the count below.
pub(super) fn settings_menu(ui: &mut egui::Ui, state: &crate::state::State, events: &mut Vec<Event>) {
    let r = Button::new("Settings").look(Look::Ghost).show(ui);
    let r = kit::tip(r, "Settings of the open view", None);
    kit::focus_ring(ui, &r);
    let budget = state.max_crossings();
    let drawn = state.scene().map(|s| s.crossings);
    egui::Popup::menu(&r)
        .close_behavior(egui::PopupCloseBehavior::CloseOnClickOutside)
        .show(|ui| {
            ui.set_min_width(260.0);
            ui.spacing_mut().item_spacing.y = 0.0;
            let Some(budget) = budget else {
                menu_label(ui, "View settings");
                file_row(ui, "Open a view to change its settings", None, false, false);
                return;
            };
            menu_label(ui, "View settings");
            egui::Frame::new().inner_margin(egui::Margin::symmetric(8, 4)).show(ui, |ui| {
                kit::form_row(ui, "Max crossings", |ui| {
                    let value = budget.to_string();
                    let items = [
                        Segment::icon(Icon::Minus, "One crossing fewer (one undo step)"),
                        Segment::text(&value, "Back to the default, 0 (one undo step)"),
                        Segment::icon(Icon::Plus, "One crossing more (one undo step)"),
                    ];
                    let next = match kit::segmented(ui, &items, true) {
                        Some(0) => budget.saturating_sub(1),
                        Some(1) => 0,
                        Some(_) => budget.saturating_add(1),
                        None => budget,
                    };
                    if next != budget {
                        events.push(Event::Command(Command::SetMaxCrossings(next)));
                    }
                });
            });
            if let Some(drawn) = drawn {
                let (text, ink) = if drawn > budget {
                    (format!("This view draws {drawn}: above the limit (L4)"), DANGER)
                } else {
                    (format!("This view draws {drawn}"), INK_3)
                };
                egui::Frame::new().inner_margin(egui::Margin::symmetric(8, 4)).show(ui, |ui| {
                    ui.label(egui::RichText::new(text).color(ink));
                });
            }
        });
}

/// One recent file (EDIT-13, behaviour change 10): its name in Geist
/// Medium, then its folder in mono, the full path as tooltip. The start
/// screen and File > Open recent draw it.
fn recent_row(ui: &mut egui::Ui, path: &std::path::Path) -> egui::Response {
    let (r, response) = ui.allocate_exact_size(Vec2::new(ui.available_width(), 36.0), Sense::click());
    if response.hovered() {
        ui.painter().rect_filled(r, CornerRadius::ZERO, BG_RAISED);
    }
    let name = path.file_name().map_or_else(|| path.display().to_string(), |n| n.to_string_lossy().into_owned());
    // DECISION: a path opened by bare name has an empty parent;
    // show it as `./`, not as the root `/`.
    let folder = path.parent().map_or_else(String::new, |p| {
        if p.as_os_str().is_empty() { "./".to_owned() } else { format!("{}/", p.display()) }
    });
    let name_font = egui::FontId::new(12.0, egui::FontFamily::Name(theme::MEDIUM.into()));
    let g = ui.painter().layout_no_wrap(name, name_font, INK);
    let nx = r.min.x + 8.0;
    let name_w = g.size().x;
    ui.painter().galley(egui::pos2(nx, r.center().y - g.size().y / 2.0), g, INK);
    let code = theme::code_font();
    let room = r.max.x - 8.0 - (nx + name_w + 12.0);
    ui.painter().text(
        egui::pos2(nx + name_w + 12.0, r.center().y),
        Align2::LEFT_CENTER,
        kit::elide(ui, &folder, &code, room),
        code,
        INK_3,
    );
    response.on_hover_text(path.display().to_string())
}

/// A File menu row: `kit::menu_row`, then the shortcut in mono at the
/// right, or a chevron when it opens a submenu.
fn file_row(ui: &mut egui::Ui, text: &str, key: Option<&str>, submenu: bool, enabled: bool) -> egui::Response {
    let r = kit::menu_row(ui, None, text, false, enabled);
    let ink = if enabled { INK_3 } else { INK_DISABLED };
    if submenu {
        let at = Rect::from_min_size(egui::pos2(r.rect.max.x - 8.0 - 16.0, r.rect.center().y - 8.0), Vec2::splat(16.0));
        icons::paint(ui.painter(), Icon::ChevronRight, at, ink);
    } else if let Some(key) = key {
        ui.painter().text(egui::pos2(r.rect.max.x - 8.0, r.rect.center().y), Align2::RIGHT_CENTER, key, theme::code_font(), ink);
    }
    r
}

/// A small label heading a group of menu rows.
fn menu_label(ui: &mut egui::Ui, text: &str) {
    egui::Frame::new().inner_margin(egui::Margin::symmetric(8, 4)).show(ui, |ui| {
        ui.label(theme::label(text));
    });
}

/// A hairline between groups of menu rows.
fn menu_separator(ui: &mut egui::Ui) {
    ui.add_space(4.0);
    kit_rule(ui, LINE_2);
    ui.add_space(4.0);
}

fn crumb(ui: &mut egui::Ui, level: Option<&str>, name: &str, here: bool) -> egui::Response {
    let font = theme::ui_font();
    let level_job = level.map(|l| {
        let mut job = egui::text::LayoutJob::default();
        job.append(
            &l.to_uppercase(),
            0.0,
            egui::TextFormat {
                font_id: theme::label_font(),
                color: INK_3,
                extra_letter_spacing: 0.6,
                ..Default::default()
            },
        );
        job
    });
    let lg = level_job.map(|j| ui.fonts_mut(|f| f.layout_job(j)));
    let ng = ui.fonts_mut(|f| f.layout_no_wrap(name.to_owned(), font, INK));
    let lw = lg.as_ref().map_or(0.0, |g| g.size().x + 6.0);
    let w = 4.0 + lw + ng.size().x + 4.0;
    let sense = if here { Sense::hover() } else { Sense::click() };
    let (r, response) = ui.allocate_exact_size(Vec2::new(w, SIZE_CONTROL), sense);
    if !here && response.hovered() {
        ui.painter().rect_filled(r, CornerRadius::same(2), BG_RAISED);
    }
    let mut x = r.min.x + 4.0;
    if let Some(g) = lg {
        let y = r.center().y - g.size().y / 2.0 + 0.5;
        ui.painter().galley(egui::pos2(x, y), g, INK_3);
        x += lw;
    }
    let ink = if here || response.hovered() { INK } else { INK_2 };
    let y = r.center().y - ng.size().y / 2.0;
    ui.painter().galley(egui::pos2(x, y), ng, ink);
    response
}

/// The `/` between crumbs.
fn separator(ui: &mut egui::Ui) {
    ui.label(egui::RichText::new("/").font(theme::code_font()).color(INK_DISABLED));
}

/// A full-width rule in `colour`.
fn kit_rule(ui: &mut egui::Ui, colour: Color32) {
    let (r, _) = ui.allocate_exact_size(Vec2::new(ui.available_width(), 1.0), Sense::hover());
    ui.painter().rect_filled(r, CornerRadius::ZERO, colour);
}

/// The overlay tooltip card over the canvas (P2-723): name, kind,
/// technology, description, tags, URL, properties (`components/Tooltip`).
pub(super) fn tip_card(ui: &mut egui::Ui, tip: &crate::state::Tooltip) {
    egui::Frame::popup(ui.style())
        .fill(BG_OVERLAY)
        .stroke(Stroke::new(1.0, LINE_2))
        .corner_radius(CornerRadius::same(theme::RADIUS_2))
        .inner_margin(egui::Margin::same(12))
        .show(ui, |ui| {
            ui.set_max_width(260.0 - 24.0);
            ui.spacing_mut().item_spacing.y = 8.0;
            let d = &tip.details;
            ui.label(theme::label(&d.kind));
            ui.label(egui::RichText::new(&d.name).font(egui::FontId::new(13.0, egui::FontFamily::Name(theme::SEMIBOLD.into()))).color(INK));
            if let Some(t) = &d.technology {
                ui.label(theme::code(t, INK_2));
            }
            if let Some(text) = &d.description {
                ui.label(egui::RichText::new(text).color(INK_2));
            }
            if !d.tags.is_empty() {
                ui.horizontal_wrapped(|ui| {
                    ui.spacing_mut().item_spacing = Vec2::splat(4.0);
                    for t in &d.tags {
                        kit::chip(ui, t, kit::ChipKind::Plain);
                    }
                });
            }
            if let Some(url) = &tip.url {
                ui.label(theme::code(url, theme::ACCENT));
            }
            for (name, value) in &tip.properties {
                kit::prop_row(ui, name, |ui| {
                    ui.label(egui::RichText::new(value).color(INK_2));
                });
            }
            if tip.sticky {
                ui.label(theme::code("Esc closes", INK_3));
            }
        });
}

/// The scene rect of `r` on screen relative to `origin`, for overlays.
#[allow(dead_code, reason = "kept for overlays placed on scene items")]
pub(super) fn screen_rect(origin: Pos2, cam: &crate::camera::Camera, r: SceneRect) -> Rect {
    let a = cam.to_screen(r.min);
    let b = cam.to_screen(r.max);
    Rect::from_min_max(origin + Vec2::new(a.x, a.y), origin + Vec2::new(b.x, b.y))
}

/// The top bar's height, for the panel.
pub(super) const BAR_H: f32 = SIZE_BAR;

#[cfg(test)]
mod tests {
    use super::menu_rows;
    use crate::state::{MenuAction, MenuEntry};

    /// Owner request 2026-11-23: the File menu opens on a click and shows
    /// every entry; Open recent lists the recent files as the start
    /// screen does; Open… asks the Open dialog. Without a file, Save,
    /// Save as and Export are greyed and do nothing.
    #[test]
    fn file_menu_opens_with_every_entry() {
        use eframe::egui::{self, Event as E, PointerButton, Pos2, RawInput, Rect, Shape, Vec2};

        use crate::state::{Command, Event};

        fn texts(shape: &Shape, out: &mut Vec<(String, Rect)>) {
            match shape {
                Shape::Text(t) => out.push((t.galley.text().to_owned(), t.visual_bounding_rect())),
                Shape::Vec(v) => v.iter().for_each(|s| texts(s, out)),
                _ => {}
            }
        }
        let ctx = egui::Context::default();
        crate::theme::install(&ctx);
        let ui_state = crate::files::UiState {
            recent: vec![std::path::PathBuf::from("/d/shop.dsl")],
            ..Default::default()
        };
        let state = crate::state::State::new(ui_state);
        let frame = |events: Vec<E>| -> (Vec<(String, Rect)>, Vec<Event>) {
            let input = RawInput {
                screen_rect: Some(Rect::from_min_size(Pos2::ZERO, Vec2::new(900.0, 700.0))),
                events,
                ..Default::default()
            };
            let mut out = Vec::new();
            let mut output = ctx.run_ui(input, |ui| {
                ui.horizontal(|ui| super::file_menu(ui, &state, &mut out));
            });
            output.textures_delta.clear();
            let mut shown = Vec::new();
            output.shapes.iter().for_each(|c| texts(&c.shape, &mut shown));
            (shown, out)
        };
        let at = |shown: &[(String, Rect)], text: &str| -> Pos2 {
            shown.iter().find(|(t, _)| t == text).map_or_else(|| panic!("{text:?} not in {shown:?}"), |(_, r)| r.center())
        };
        let click = |p: Pos2| {
            vec![
                E::PointerMoved(p),
                E::PointerButton { pos: p, button: PointerButton::Primary, pressed: true, modifiers: egui::Modifiers::NONE },
                E::PointerButton { pos: p, button: PointerButton::Primary, pressed: false, modifiers: egui::Modifiers::NONE },
            ]
        };
        let (shown, _) = frame(Vec::new());
        let file = at(&shown, "File");
        assert!(!shown.iter().any(|(t, _)| t == "Save as…"), "closed until clicked");
        frame(click(file));
        let (shown, _) = frame(Vec::new());
        for text in ["Export", "Print… (coming soon)", "Save", "Save as…", "Open…", "Open recent", "Ctrl+S", "Ctrl+Shift+S", "Ctrl+O"] {
            at(&shown, text);
        }
        // Greyed without a file: a click on Save does nothing.
        let (_, events) = frame(click(at(&shown, "Save")));
        assert!(events.is_empty(), "{events:?}");
        // Open recent: the start screen's rows.
        let (mut shown, _) = frame(Vec::new());
        if !shown.iter().any(|(t, _)| t == "Open recent") {
            frame(click(file));
            shown = frame(Vec::new()).0;
        }
        frame(vec![E::PointerMoved(at(&shown, "Open recent"))]);
        let (shown, _) = frame(Vec::new());
        at(&shown, "shop.dsl");
        at(&shown, "/d/");
        // Open… asks the Open dialog.
        let (_, events) = frame(click(at(&shown, "Open…")));
        assert!(matches!(events.as_slice(), [Event::Command(Command::ShowOpenDialog)]), "{events:?}");
    }

    /// Owner request 2026-11-23: the Settings button opens the open
    /// view's settings; the budget shows as written, a click on it resets
    /// it to the default, and the popup stays open for the next step.
    #[test]
    fn settings_menu_shows_the_view_budget() {
        use eframe::egui::{self, Event as E, PointerButton, Pos2, RawInput, Rect, Shape, Vec2};

        use crate::state::{Command, Event};
        fn texts(shape: &Shape, out: &mut Vec<String>) {
            match shape {
                Shape::Text(t) => out.push(t.galley.text().to_owned()),
                Shape::Vec(v) => v.iter().for_each(|s| texts(s, out)),
                _ => {}
            }
        }
        let (_dir, _path, mut h) = crate::file_menu_tests::shop("settings-popup");
        let effects = h.s.handle(Event::Command(Command::SetMaxCrossings(2)));
        h.run(effects);
        let state = h.s;
        let ctx = egui::Context::default();
        crate::theme::install(&ctx);
        let frame = |events: Vec<E>| -> (Vec<(String, Rect)>, Vec<Event>) {
            let input = RawInput {
                screen_rect: Some(Rect::from_min_size(Pos2::ZERO, Vec2::new(900.0, 700.0))),
                events,
                ..Default::default()
            };
            let mut out = Vec::new();
            let mut output = ctx.run_ui(input, |ui| {
                ui.horizontal(|ui| super::settings_menu(ui, &state, &mut out));
            });
            output.textures_delta.clear();
            let mut shown = Vec::new();
            for c in &output.shapes {
                let mut t = Vec::new();
                texts(&c.shape, &mut t);
                shown.extend(t.into_iter().map(|t| (t, c.shape.visual_bounding_rect())));
            }
            (shown, out)
        };
        let at = |shown: &[(String, Rect)], text: &str| -> Pos2 {
            shown.iter().find(|(t, _)| t == text).map_or_else(|| panic!("{text:?} not in {shown:?}"), |(_, r)| r.center())
        };
        let click = |p: Pos2| {
            vec![
                E::PointerMoved(p),
                E::PointerButton { pos: p, button: PointerButton::Primary, pressed: true, modifiers: egui::Modifiers::NONE },
                E::PointerButton { pos: p, button: PointerButton::Primary, pressed: false, modifiers: egui::Modifiers::NONE },
            ]
        };
        let (shown, _) = frame(Vec::new());
        assert!(!shown.iter().any(|(t, _)| t == "MAX CROSSINGS"), "closed until clicked");
        frame(click(at(&shown, "Settings")));
        let (shown, _) = frame(Vec::new());
        at(&shown, "VIEW SETTINGS");
        at(&shown, "MAX CROSSINGS");
        assert!(shown.iter().any(|(t, _)| t.starts_with("This view draws ")), "{shown:?}");
        let (_, events) = frame(click(at(&shown, "2")));
        assert!(matches!(events.as_slice(), [Event::Command(Command::SetMaxCrossings(0))]), "{events:?}");
        let (shown, _) = frame(Vec::new());
        at(&shown, "MAX CROSSINGS");
    }

    fn entry(label: &str) -> MenuEntry {
        MenuEntry {
            label: label.to_owned(),
            action: MenuAction::GroupBus,
        }
    }

    fn texts(rows: &[super::MenuRow<'_>]) -> Vec<(String, bool, bool)> {
        rows.iter().map(|r| (r.text.clone(), r.entry.is_some(), r.separated)).collect()
    }

    /// Behaviour change 5: a box with a URL but no deeper view shows
    /// `Enter` disabled above the URL entry, split by a rule.
    #[test]
    fn box_menu_keeps_its_shape() {
        let url = entry("Open URL https://x.org");
        let rows = menu_rows(std::slice::from_ref(&url), false);
        assert_eq!(
            texts(&rows),
            [("Enter a view".to_owned(), false, false), ("Open URL https://x.org".to_owned(), true, true)]
        );
        assert_eq!(rows[1].entry, Some(&url));
    }

    /// Behaviour change 5: a bundled line without pins or URL shows the
    /// release and URL entries disabled, the bus pair enabled.
    #[test]
    fn line_menu_greys_what_cannot_run() {
        let entries = [entry("Ungroup bus"), entry("Remove from bus")];
        let rows = menu_rows(&entries, true);
        assert_eq!(
            texts(&rows),
            [
                ("Release start".to_owned(), false, false),
                ("Release end".to_owned(), false, false),
                ("Ungroup bus".to_owned(), true, true),
                ("Remove from bus".to_owned(), true, false),
                ("Open URL".to_owned(), false, true),
            ]
        );
    }

    /// "Group as bus" takes the bus group's place; an entry no slot names
    /// is still shown.
    #[test]
    fn line_menu_group_and_unknown_entries() {
        let entries = [entry("Group as bus"), entry("Something new")];
        let rows = menu_rows(&entries, true);
        let shown: Vec<String> = rows.iter().map(|r| r.text.clone()).collect();
        assert_eq!(shown, ["Release start", "Release end", "Group as bus", "Open URL", "Something new"]);
        assert!(rows.last().is_some_and(|r| r.entry.is_some()));
    }

    /// Owner decision 2026-11-19 (behaviour change 5): with no entry the
    /// menu still has its rows, every one disabled, so a click on any of
    /// them runs nothing.
    #[test]
    fn menu_with_no_entry_is_all_disabled() {
        let box_rows = menu_rows(&[], false);
        assert_eq!(
            texts(&box_rows),
            [("Enter a view".to_owned(), false, false), ("Open URL".to_owned(), false, true)]
        );
        let line_rows = menu_rows(&[], true);
        assert!(!line_rows.is_empty());
        assert!(line_rows.iter().all(|r| r.entry.is_none()), "{:?}", texts(&line_rows));
    }
}

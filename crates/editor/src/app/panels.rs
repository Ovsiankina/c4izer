//! The side panels as stacks of sections (redesign 2026-11-19,
//! `guides/01-app-frame.md`): left, the views list with the view map at
//! its foot; right, selection Details · Style · Line · Steps · Canvas ·
//! Lint · Export. A section that does not apply is left out, never
//! dimmed (behaviour change 12).

use c4rizer_core::edit::{StyleProperty, StyleTarget};
use c4rizer_core::model::{Colour, CrossingStyle, ElementKey, LabelMode};
use eframe::egui::{self, Align2, Color32, CornerRadius, Rect, Sense, Vec2};

use super::{App, editable_detail_fields, export_choices, svg_text_choices, view_label, view_list_line};
use crate::icons::{self, Icon};
use crate::kit::{self, Button, ChipKind, Look, Segment};
use crate::state::{Alignment, Axis, CanvasStep, Command, Event, Selected, State};
use crate::theme::{self, ACCENT, ACCENT_SOFT, BG_RAISED, INK, INK_2, INK_3, INK_DISABLED, SIZE_ROW, WARN};
use crate::worker::ExportFormat;

fn cmd(out: &mut Vec<Event>, c: Command) {
    out.push(Event::Command(c));
}

fn colour32(c: Colour) -> Color32 {
    Color32::from_rgb(c.r, c.g, c.b)
}

/// `State::style_panel` with the fill and text colours the selected
/// boxes are drawn in on the shown scene, when they all agree: a box
/// whose file sets no colour is drawn in the house palette (redesign
/// 2026-11-19), so the swatches and the head's dot show what the canvas
/// shows rather than the `#dddddd` / `#000000` keyword defaults.
// DECISION: read from the scene in the editor, as the house palette is
// applied when the view is built and `Model::style_of` stays
// mode-independent (P2-430); with no scene box to read, the model's
// value stays.
fn shown_style(state: &State) -> Option<crate::state::StylePanel> {
    let mut panel = state.style_panel()?;
    let (Some(scene), Some(model)) = (state.scene(), state.model()) else { return Some(panel) };
    let ids: Vec<_> = state
        .selection
        .iter()
        .filter_map(|s| match s {
            Selected::Element(key) => model.find(key),
            _ => None,
        })
        .collect();
    let drawn: Vec<_> = scene.boxes.iter().filter(|b| ids.contains(&b.element)).collect();
    if drawn.is_empty() || drawn.len() != ids.len() {
        return Some(panel);
    }
    let agree = |f: fn(&c4rizer_core::scene::SceneBox) -> Colour| {
        let first = f(drawn[0]);
        drawn.iter().all(|b| f(b) == first).then_some(first)
    };
    if panel.background.is_some() {
        panel.background = agree(|b| b.fill).or(panel.background);
    }
    if panel.color.is_some() {
        panel.color = agree(|b| b.name.colour).or(panel.color);
    }
    Some(panel)
}

impl App {
    /// The right panel's sections, top to bottom. Serves EDIT-5, EDIT-6,
    /// EDIT-7, EDIT-10, EDIT-17, EDIT-20.
    #[must_use]
    pub fn right_panel(&self, ui: &mut egui::Ui) -> Vec<Event> {
        let mut out = Vec::new();
        ui.spacing_mut().item_spacing = Vec2::new(4.0, 0.0);
        egui::ScrollArea::vertical()
            .id_salt("c4rizer-right-scroll")
            .auto_shrink([false, false])
            .show(ui, |ui| {
                ui.set_width(ui.available_width());
                self.selection_section(ui, &mut out);
                let style = shown_style(&self.state);
                if let Some(panel) = &style {
                    style_section(ui, panel, &mut out);
                }
                line_section(ui, &self.state, &mut out);
                steps_section(ui, &self.state, &mut out);
                self.canvas_section(ui, style.is_none(), &mut out);
                lint_section(ui, &self.state, &mut out);
                export_section(ui, &mut out);
            });
        out
    }

    /// Selection Details: kind label, name as the title, description, then
    /// property rows; the in-place editor's fields show a pencil and a
    /// hover outline, a double click opens the editor on that field
    /// (P2-739, EDIT-5).
    fn selection_section(&self, ui: &mut egui::Ui, out: &mut Vec<Event>) {
        egui::Frame::new()
            .inner_margin(egui::Margin {
                left: 12,
                right: 12,
                top: 12,
                bottom: 12,
            })
            .show(ui, |ui| {
                ui.set_width(ui.available_width());
                ui.spacing_mut().item_spacing = Vec2::new(4.0, 8.0);
                let count = self.state.selection.len();
                let Some(d) = self.state.details() else {
                    kit::head_label(ui, if count > 1 { "several items" } else { "nothing selected" }, None);
                    ui.label(
                        egui::RichText::new("Click a box or a line to see it here.\nRight-click a box to enter its views.")
                            .color(INK_2),
                    );
                    return;
                };
                let boxes = self
                    .state
                    .selection
                    .iter()
                    .filter(|s| matches!(s, Selected::Element(_)))
                    .count();
                let dot = shown_style(&self.state).and_then(|p| p.background).map(colour32);
                if boxes > 1 {
                    kit::head_label(ui, &format!("{boxes} boxes"), None);
                    let names: Vec<String> = self
                        .state
                        .selection
                        .iter()
                        .filter_map(|s| match s {
                            Selected::Element(key) => self
                                .state
                                .model()
                                .and_then(|m| m.find(key).map(|id| m.element(id).name.value.clone())),
                            _ => None,
                        })
                        .collect();
                    ui.label(egui::RichText::new(names.join(", ")).font(theme::title_font()).color(INK));
                } else {
                    kit::head_label(ui, &d.kind, dot);
                    let editable = editable_detail_fields(&self.state);
                    editable_field(ui, &editable, "name", field_text(&d.name, true), out);
                    if let Some(text) = &d.description
                        && d.kind != "Relationship"
                    {
                        editable_field(ui, &editable, "description", field_text(text, false), out);
                    }
                    if let Some(t) = &d.technology {
                        if d.kind == "Relationship" {
                            ui.label(theme::code(t, INK_2));
                        } else {
                            kit::prop_row(ui, "tech", |ui| {
                                editable_field(ui, &editable, "technology", field_text(t, false), out);
                            });
                        }
                    }
                }
                if let Some(Selected::Relationship(r)) = self.state.selection.iter().next()
                    && count == 1
                {
                    relationship_rows(ui, &self.state, r);
                }
                detail_rows(ui, &d);
            });
        kit::hairline(ui);
    }

    /// The view as a whole: Auto-place, paper size, Magnet, Centre, Remove
    /// vertices, crossings, Callout labels (P2-700..P2-704, P3-313,
    /// P4-318, P4-521). Folded by default while boxes are selected.
    fn canvas_section(&self, ui: &mut egui::Ui, default_open: bool, out: &mut Vec<Event>) {
        if self.state.doc.is_none() || self.state.scene().is_none() || self.state.read_only() {
            return;
        }
        kit::section(ui, "canvas", "Canvas", None, default_open, |ui| {
            let r = kit::tip(
                Button::new("Auto-place").with_icon(Icon::AutoPlace).block().show(ui),
                "Place every box afresh (one undo step)",
                None,
            );
            if r.clicked() {
                cmd(out, Command::AutoPlace);
            }
            // P2-702: Alt held keeps every box where it is.
            let alt = ui.input(|i| i.modifiers.alt);
            kit::form_row(ui, "Paper", |ui| {
                let items = [
                    Segment::icon(Icon::Minus, "Smaller paper (Alt: keep positions)"),
                    Segment::text("Auto", "Smallest paper that holds the diagram"),
                    Segment::icon(Icon::Plus, "Larger paper (Alt: keep positions)"),
                ];
                if let Some(i) = kit::segmented(ui, &items, true) {
                    let step = [CanvasStep::Decrease, CanvasStep::Auto, CanvasStep::Increase][i];
                    cmd(out, Command::Canvas { step, keep_positions: alt });
                }
            });
            ui.columns(2, |cols| {
                cols[0].spacing_mut().item_spacing.x = 4.0;
                if kit::tip(Button::new("Magnet").with_icon(Icon::Magnet).block().show(&mut cols[0]), "Bring boxes onto the paper", None)
                    .clicked()
                {
                    cmd(out, Command::BringOntoCanvas);
                }
                if kit::tip(
                    Button::new("Centre").with_icon(Icon::Centre).block().show(&mut cols[1]),
                    "Centre the diagram or the selected boxes",
                    None,
                )
                .clicked()
                {
                    cmd(out, Command::Centre);
                }
            });
            if kit::tip(
                Button::new("Remove vertices").with_icon(Icon::VertexRemove).block().show(ui),
                "Remove every hand-added line vertex of this view (one undo step)",
                None,
            )
            .clicked()
            {
                cmd(out, Command::RemoveAllVertices);
            }
            // P4-521: how the view marks its crossings.
            if let Some(style) = self.state.crossing_style() {
                kit::form_row(ui, "Crossings", |ui| {
                    let choices = [CrossingStyle::Gap, CrossingStyle::Jump, CrossingStyle::Plain];
                    let items = [
                        Segment::text("Gap", "Cut one line with a T bar on each side (one undo step)").on(style == CrossingStyle::Gap),
                        Segment::text("Jump", "One line hops over the other (one undo step)").on(style == CrossingStyle::Jump),
                        Segment::text("Plain", "Lines cross unmarked (one undo step)").on(style == CrossingStyle::Plain),
                    ];
                    if let Some(i) = kit::segmented(ui, &items, true)
                        && choices[i] != style
                    {
                        cmd(out, Command::SetCrossingStyle(choices[i]));
                    }
                });
            }
            // P4-318: the view's label mode.
            if let Some(mode) = self.state.label_mode() {
                let mut callout = mode == LabelMode::Callout;
                let r = kit::checkbox(ui, &mut callout, "Callout labels");
                let r = kit::tip(r, "Labels beside their lines on a shelf with a slanted leader (one undo step)", None);
                if r.changed() {
                    cmd(out, Command::SetLabelMode(if callout { LabelMode::Callout } else { LabelMode::Line }));
                }
            }
        });
    }

    /// The left panel: the Views section, or the no-file note. Serves
    /// EDIT-8, EDIT-16.
    #[must_use]
    pub fn left_panel(&self, ui: &mut egui::Ui) -> Vec<Event> {
        let mut out = Vec::new();
        ui.spacing_mut().item_spacing = Vec2::new(4.0, 0.0);
        if self.state.doc.is_none() {
            // The note keeps the panel at its width; a resizable panel
            // otherwise shrinks to its widest line.
            ui.set_min_width(ui.available_width());
            egui::Frame::new().inner_margin(egui::Margin::same(12)).show(ui, |ui| {
                ui.spacing_mut().item_spacing.y = 8.0;
                ui.label(theme::label("No file open"));
                ui.label(egui::RichText::new("Views appear here once a workspace is open.").color(INK_2));
            });
            return out;
        }
        // P5-606: the folders `left_side` synced this frame, not a rebuild.
        let folders = self.view_folders.folders();
        let count = format!("{:02}", folders.iter().map(|f| f.views.len()).sum::<usize>());
        kit::section(ui, "views", "Views", Some((count, false)), true, |ui| {
            ui.spacing_mut().item_spacing.y = 0.0;
            super::view_list(ui, &self.state, &folders, self.view_folders.key_w, &mut out);
        });
        out
    }
}

/// The rows of one selected relationship: its ends by name, then its bus
/// when the line is bundled (`components/Details`, P4-426).
fn relationship_rows(ui: &mut egui::Ui, state: &State, r: &c4rizer_core::edit::RelationshipRef) {
    if let Some(model) = state.model() {
        let name = |key: &ElementKey| model.find(key).map_or_else(|| key.to_string(), |id| model.element(id).name.value.clone());
        kit::prop_row(ui, "from", |ui| {
            ui.label(egui::RichText::new(name(&r.source)).color(INK));
        });
        kit::prop_row(ui, "to", |ui| {
            ui.label(egui::RichText::new(name(&r.destination)).color(INK));
        });
    }
    if let [edge] = state.selected_lines().as_slice()
        && let Some(bus) = state.bus_of(*edge)
    {
        kit::prop_row(ui, "bus", |ui| {
            ui.label(egui::RichText::new(bus_text(bus.len())).color(INK_2));
        });
    }
}

/// The Bus row's value: `one of 3 lines`.
fn bus_text(lines: usize) -> String {
    format!("one of {lines} lines")
}

/// The property rows every kind of selection may have: deployment
/// instances and environment, tags, where a lifted line comes from, the
/// lines merged into it, and a dynamic step.
fn detail_rows(ui: &mut egui::Ui, d: &crate::state::Details) {
    // P3-131: a deployment node's instances and environment.
    if let Some(n) = &d.instances {
        kit::prop_row(ui, "instances", |ui| {
            ui.label(theme::code(n, INK));
        });
    }
    if let Some(env) = &d.environment {
        kit::prop_row(ui, "env", |ui| {
            ui.label(egui::RichText::new(env).color(INK));
        });
    }
    if !d.tags.is_empty() {
        kit::prop_row(ui, "tags", |ui| {
            ui.horizontal_wrapped(|ui| {
                ui.spacing_mut().item_spacing = Vec2::splat(4.0);
                for tag in &d.tags {
                    kit::chip(ui, tag, ChipKind::Plain);
                }
            });
        });
    }
    if let Some(from) = &d.lifted_from {
        kit::prop_row(ui, "lifted", |ui| {
            ui.label(egui::RichText::new(from).color(INK_2));
        });
    }
    for other in &d.merged {
        kit::prop_row(ui, "also", |ui| {
            ui.label(egui::RichText::new(other).color(INK_2));
        });
    }
    if let Some(step) = &d.step {
        // P3-218: number, ends and the relationship it stands for.
        kit::prop_row(ui, "step", |ui| {
            ui.label(egui::RichText::new(format!("{}: {}", step.order, step.ends)).color(INK));
        });
        kit::prop_row(ui, "for", |ui| {
            ui.label(egui::RichText::new(&step.relationship).color(INK_2));
        });
    }
}

/// A details field's text: the name as `title`, others as `ui`.
fn field_text(text: &str, title: bool) -> egui::RichText {
    if title {
        egui::RichText::new(text).font(theme::title_font()).color(INK)
    } else {
        egui::RichText::new(text).color(INK_2)
    }
}

/// A details field showing `rich` (`field_text`); editable ones get a
/// pencil and an outline on hover and open the in-place editor on a
/// double click (Enter, then Tab to the field).
fn editable_field(ui: &mut egui::Ui, editable: &[&'static str], label: &'static str, rich: egui::RichText, out: &mut Vec<Event>) {
    let Some(at) = editable.iter().position(|&f| f == label) else {
        ui.label(rich);
        return;
    };
    let width = ui.available_width();
    // The frame reaches 4 px past the text on each side, as the design's
    // `margin: 0 -4px; padding: 2px 4px`.
    let inner = egui::Frame::new().outer_margin(egui::Margin { left: -4, right: -4, top: 0, bottom: 0 }).inner_margin(egui::Margin::symmetric(4, 2)).show(ui, |ui| {
        ui.set_width(width);
        ui.with_layout(egui::Layout::top_down(egui::Align::Min), |ui| {
            ui.set_max_width(width - 16.0);
            ui.add(egui::Label::new(rich).wrap().halign(egui::Align::Min));
        });
    });
    let rect = inner.response.rect;
    let r = ui.interact(rect, ui.scope_id().with(("c4rizer-edit-field", label)), Sense::click());
    let r = r.on_hover_text(format!("Double click or Enter to edit the {label}"));
    if r.hovered() {
        ui.painter().rect_stroke(rect, CornerRadius::same(2), egui::Stroke::new(1.0, theme::LINE_STRONG), egui::StrokeKind::Inside);
        let pencil = Rect::from_min_size(egui::pos2(rect.max.x - 16.0, rect.min.y + 4.0), Vec2::splat(12.0));
        icons::paint(ui.painter(), Icon::Edit, pencil, INK_3);
    }
    if r.double_clicked() {
        out.push(Event::Input(crate::state::Input::KeyDown {
            key: crate::state::Key::Enter,
            mods: crate::state::Modifiers::default(),
            text_focus: false,
        }));
        // Enter opens on the first field; the Tab presses reach this one,
        // as a click on a field in the canvas does.
        out.extend(crate::app::focus_keys(0, at, editable.len()));
    }
}

/// The Style section (EDIT-17, EDIT-20): target, fill, text, shape; with
/// two boxes or more, Align and Spread; Reset to tag style.
fn style_section(ui: &mut egui::Ui, panel: &crate::state::StylePanel, out: &mut Vec<Event>) {
    let title = format!("Style · {} box{}", panel.boxes, if panel.boxes == 1 { "" } else { "es" });
    kit::section(ui, "style", &title, None, true, |ui| {
        // DECISION: the target choice lives in egui's temporary memory
        // (index 0: this box only, i: the i-th shared tag), so the panel
        // stays a pure reader of `State`.
        let id = egui::Id::unique("c4rizer-style-target");
        let mut choice = ui.ctx().data(|d| d.get_temp::<usize>(id)).unwrap_or(0);
        if choice > panel.tags.len() {
            choice = 0;
        }
        let names: Vec<String> = std::iter::once("this box only".to_owned())
            .chain(panel.tags.iter().map(|t| format!("all {t}")))
            .collect();
        kit::form_row(ui, "Apply to", |ui| {
            if let Some(i) = kit::select(ui, "style-target", &names[choice], &names, Some(choice)) {
                choice = i;
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
        for (label, value, make) in [
            ("Fill", panel.background, StyleProperty::Background as fn(Option<Colour>) -> StyleProperty),
            ("Text", panel.color, StyleProperty::Color),
        ] {
            kit::form_row(ui, label, |ui| {
                if let Some([r, g, b]) = kit::swatch(ui, label, value.map(|c| [c.r, c.g, c.b])) {
                    cmd(
                        out,
                        Command::SetStyle {
                            target: target.clone(),
                            property: make(Some(Colour { r, g, b })),
                        },
                    );
                }
            });
        }
        // P2-736: one drop-down, never a wrapped row of buttons.
        super::shape_row(ui, panel.shape, &mut |shape| {
            cmd(
                out,
                Command::SetStyle {
                    target: target.clone(),
                    property: StyleProperty::Shape(Some(shape)),
                },
            );
        });
        if panel.boxes > 1 {
            kit::form_row(ui, "Align", |ui| {
                let items = [
                    Segment::icon(Icon::AlignLeft, "Align left").key("L"),
                    Segment::icon(Icon::AlignCentre, "Align centre").key("C"),
                    Segment::icon(Icon::AlignRight, "Align right").key("R"),
                    Segment::icon(Icon::AlignTop, "Align top").key("T"),
                    Segment::icon(Icon::AlignMiddle, "Align middle").key("M"),
                    Segment::icon(Icon::AlignBottom, "Align bottom").key("B"),
                ];
                if let Some(i) = kit::segmented(ui, &items, true) {
                    let how = [
                        Alignment::Left,
                        Alignment::Centre,
                        Alignment::Right,
                        Alignment::Top,
                        Alignment::Middle,
                        Alignment::Bottom,
                    ][i];
                    cmd(out, Command::Align(how));
                }
            });
            kit::form_row(ui, "Spread", |ui| {
                let items = [
                    Segment::icon(Icon::DistributeH, "Distribute horizontally"),
                    Segment::icon(Icon::DistributeV, "Distribute vertically"),
                ];
                if let Some(i) = kit::segmented(ui, &items, true) {
                    cmd(out, Command::Distribute([Axis::Horizontal, Axis::Vertical][i]));
                }
            });
        }
        ui.vertical_centered(|ui| {
            if Button::new("Reset to tag style").with_icon(Icon::Reset).look(Look::Ghost).show(ui).clicked() {
                cmd(out, Command::ResetStyle);
            }
        });
    });
}

/// The Line section: the bus commands of the selected lines (P4-424,
/// P4-425), only when there are some.
fn line_section(ui: &mut egui::Ui, state: &State, out: &mut Vec<Event>) {
    let bus = state.bus_entries();
    if bus.is_empty() {
        return;
    }
    kit::section(ui, "line", "Line", None, true, |ui| {
        for entry in bus {
            let icon = if entry.label.starts_with("Remove") {
                Icon::Minus
            } else {
                Icon::Bus
            };
            if Button::new(&entry.label).with_icon(icon).block().show(ui).clicked() {
                cmd(out, Command::ContextAction(entry.action));
            }
        }
    });
}

/// The Steps of a dynamic view (P3-219, P3-220): one row per step in
/// number order; a click selects the step's line, the arrows move it.
fn steps_section(ui: &mut egui::Ui, state: &State, out: &mut Vec<Event>) {
    let rows = state.steps();
    if rows.is_empty() {
        return;
    }
    kit::section(ui, "steps", "Steps", Some((rows.len().to_string(), false)), true, |ui| {
        ui.spacing_mut().item_spacing.y = 0.0;
        for row in &rows {
            let width = ui.available_width();
            let (rect, response) = ui.allocate_exact_size(Vec2::new(width, SIZE_ROW), Sense::click());
            let p = ui.painter();
            if row.selected {
                p.rect_filled(rect, CornerRadius::same(2), ACCENT_SOFT);
            } else if response.hovered() {
                p.rect_filled(rect, CornerRadius::same(2), BG_RAISED);
            }
            let disc = Rect::from_center_size(egui::pos2(rect.min.x + 16.0, rect.center().y), Vec2::splat(16.0));
            let ring = if row.selected { ACCENT } else { theme::LINE_STRONG };
            p.circle_stroke(disc.center(), 7.5, egui::Stroke::new(1.0, ring));
            p.text(
                disc.center(),
                Align2::CENTER_CENTER,
                row.step.to_string(),
                egui::FontId::monospace(10.0),
                if row.selected { ACCENT } else { INK },
            );
            let room = rect.width() - 32.0 - 52.0;
            let font = theme::ui_font();
            p.text(
                egui::pos2(disc.max.x + 8.0, rect.center().y),
                Align2::LEFT_CENTER,
                kit::elide(ui, &row.label, &font, room),
                font,
                if row.selected { INK } else { INK_2 },
            );
            if response.clicked() {
                cmd(out, Command::SelectStep(row.step));
            }
            let mut child = ui.new_child(
                egui::UiBuilder::new()
                    .max_rect(Rect::from_min_max(egui::pos2(rect.max.x - 52.0, rect.min.y + 2.0), rect.max))
                    .layout(egui::Layout::left_to_right(egui::Align::Center)),
            );
            child.spacing_mut().item_spacing.x = 2.0;
            if kit::tip(Button::icon(Icon::ArrowUp).look(Look::Ghost).show(&mut child), "Move this step up", None).clicked() {
                cmd(out, Command::MoveStep { step: row.step, up: true });
            }
            if kit::tip(Button::icon(Icon::ArrowDown).look(Look::Ghost).show(&mut child), "Move this step down", None).clicked() {
                cmd(out, Command::MoveStep { step: row.step, up: false });
            }
        }
    });
}

/// The Lint section: findings (warn icon, text clamped to two lines),
/// then notices; the list scrolls after five rows so Export stays in
/// reach (behaviour change 13).
fn lint_section(ui: &mut egui::Ui, state: &State, out: &mut Vec<Event>) {
    if state.doc.is_none() {
        return;
    }
    let findings = state.findings();
    let notices = state.notices();
    let n = findings.len();
    kit::section(ui, "lint", "Lint", Some((n.to_string(), n > 0)), true, |ui| {
        if findings.is_empty() && notices.is_empty() {
            ui.label(egui::RichText::new("no findings").color(INK_2));
            return;
        }
        ui.spacing_mut().item_spacing.y = 0.0;
        let rows = findings.len() + notices.len();
        egui::ScrollArea::vertical()
            .id_salt("c4rizer-lint")
            .max_height(5.0 * 44.0)
            .auto_shrink([false, true])
            .show(ui, |ui| {
                for (i, f) in findings.iter().enumerate() {
                    if lint_row(ui, Icon::Warn, WARN, &f.message, true).clicked() {
                        cmd(out, Command::PickFinding(i));
                    }
                }
                // P2-126: notices name the construct, its file and line;
                // they select nothing.
                for notice in &notices {
                    lint_row(ui, Icon::Info, INK_3, &format!("notice: {notice}"), false);
                }
            });
        let _ = rows;
    });
}

/// One lint row: 32 px icon column, the text in two lines at most.
fn lint_row(ui: &mut egui::Ui, icon: Icon, icon_ink: Color32, text: &str, pick: bool) -> egui::Response {
    let width = ui.available_width();
    let font = theme::ui_font();
    let job = {
        let mut job = egui::text::LayoutJob::simple(text.to_owned(), font, INK_2, width - 32.0 - 16.0 - 8.0);
        job.wrap.max_rows = 2;
        job
    };
    let galley = ui.fonts_mut(|f| f.layout_job(job));
    let height = galley.size().y + 12.0;
    let sense = if pick { Sense::click() } else { Sense::hover() };
    let (rect, response) = ui.allocate_exact_size(Vec2::new(width, height), sense);
    let hovered = pick && response.hovered();
    let p = ui.painter();
    if hovered {
        p.rect_filled(rect, CornerRadius::same(2), BG_RAISED);
    }
    icons::paint(p, icon, Rect::from_min_size(egui::pos2(rect.min.x + 8.0, rect.min.y + 6.0), Vec2::splat(16.0)), icon_ink);
    p.galley(egui::pos2(rect.min.x + 8.0 + 32.0 + 8.0, rect.min.y + 6.0), galley, if hovered { INK } else { INK_2 });
    if pick {
        response.on_hover_text(text)
    } else {
        response
    }
}

/// Where egui's memory keeps the export panel's SVG text choice.
const SVG_TEXT_ID: &str = "c4rizer-svg-text";

/// The SVG format the export panel's SVG text choice names (P2-663),
/// plain `Svg` until one is made; File > Export uses it too.
pub(super) fn svg_choice(ctx: &egui::Context) -> ExportFormat {
    let choices = svg_text_choices();
    let pick = ctx.data(|d| d.get_temp::<usize>(egui::Id::unique(SVG_TEXT_ID))).unwrap_or(0).min(choices.len() - 1);
    choices[pick].1
}

/// The Export section, folded by default (EDIT-7, P2-663, P3-431).
fn export_section(ui: &mut egui::Ui, out: &mut Vec<Event>) {
    kit::section(ui, "export", "Export", None, false, |ui| {
        // P2-663: the SVG text choice, `name` by default; it lives in
        // egui's memory and never reaches the diagram file (P2-665).
        let id = egui::Id::unique(SVG_TEXT_ID);
        let choices = svg_text_choices();
        let mut pick = ui.ctx().data(|d| d.get_temp::<usize>(id)).unwrap_or(0).min(choices.len() - 1);
        let names: Vec<String> = choices.iter().map(|(l, _)| (*l).to_owned()).collect();
        kit::form_row(ui, "SVG text", |ui| {
            if let Some(i) = kit::select(ui, "svg-text", &names[pick], &names, Some(pick)) {
                pick = i;
            }
        });
        ui.ctx().data_mut(|d| d.insert_temp(id, pick));
        let svg = choices[pick].1;
        let pair = |ui: &mut egui::Ui, a: &str, b: &str| -> (bool, bool) {
            let mut clicked = (false, false);
            ui.columns(2, |cols| {
                clicked.0 = Button::new(a).block().show(&mut cols[0]).clicked();
                clicked.1 = Button::new(b).block().show(&mut cols[1]).clicked();
            });
            clicked
        };
        let (view_svg, view_png) = pair(ui, "View as SVG", "View as PNG");
        if view_svg {
            cmd(out, Command::ExportView(svg));
        }
        if view_png {
            cmd(out, Command::ExportView(ExportFormat::Png));
        }
        let (all_svg, all_png) = pair(ui, "All views as SVG", "All views as PNG");
        if all_svg {
            cmd(out, Command::ExportAll(svg));
        }
        if all_png {
            cmd(out, Command::ExportAll(ExportFormat::Png));
        }
        // P3-431: every format of `c4rizer export -f` (P3-400), the SVG
        // with the SVG text chosen above.
        let id = egui::Id::unique("c4rizer-export-format");
        let formats = export_choices();
        let mut format = ui.ctx().data(|d| d.get_temp::<usize>(id)).unwrap_or(0).min(formats.len() - 1);
        let names: Vec<String> = formats.iter().map(|(l, _)| (*l).to_owned()).collect();
        kit::form_row(ui, "Format", |ui| {
            if let Some(i) = kit::select(ui, "export-format", &names[format], &names, Some(format)) {
                format = i;
            }
        });
        ui.ctx().data_mut(|d| d.insert_temp(id, format));
        let chosen = if formats[format].1 == ExportFormat::Svg { svg } else { formats[format].1 };
        if Button::new("Export view").with_icon(Icon::Export).block().show(ui).clicked() {
            cmd(out, Command::ExportView(chosen));
        }
        if Button::new("Export all views").with_icon(Icon::Export).block().show(ui).clicked() {
            cmd(out, Command::ExportAll(chosen));
        }
    });
}

/// One row of the view list: the key (mono, `accent` when current), then
/// the title cut to the room left; the full title and kind as tooltip
/// (EDIT-8, P2-737, behaviour change 11). Indented `indent` px under its
/// folder's header (P5-602); key and title never pass the row's right
/// edge. Returns the row's text and response.
pub(super) fn view_row(ui: &mut egui::Ui, entry: &crate::state::ViewEntry, key_w: f32, indent: f32) -> (String, egui::Response) {
    let width = ui.available_width();
    let (rect, response) = ui.allocate_exact_size(Vec2::new(width, SIZE_ROW), Sense::click());
    let p = ui.painter();
    if entry.current {
        p.rect_filled(rect, CornerRadius::same(2), ACCENT_SOFT);
    } else if response.hovered() {
        p.rect_filled(rect, CornerRadius::same(2), BG_RAISED);
    }
    let key = entry.view.key();
    let weak = !entry.kind.drawable();
    let key_ink = if weak {
        INK_DISABLED
    } else if entry.current {
        ACCENT
    } else {
        INK
    };
    let title_ink = if weak {
        INK_DISABLED
    } else if entry.current || response.hovered() {
        INK
    } else {
        INK_2
    };
    let code = theme::code_font();
    let x0 = rect.min.x + 8.0 + indent;
    // P5-602: a panel narrower than the key column cuts the key, never
    // the panel's edge.
    let key_w = key_w.min((rect.max.x - 8.0 - x0).max(0.0));
    let key_text = fit(ui, &key, &code, key_w);
    let p = ui.painter();
    p.text(egui::pos2(x0, rect.center().y), Align2::LEFT_CENTER, &key_text, code, key_ink);
    let (_, title) = view_list_line(entry, entry.label.chars().count());
    let font = theme::ui_font();
    let room = (rect.max.x - 8.0 - (x0 + key_w + 8.0)).max(0.0);
    let shown = if title == key { String::new() } else { fit(ui, &title, &font, room) };
    p.text(egui::pos2(x0 + key_w + 8.0, rect.center().y), Align2::LEFT_CENTER, &shown, font, title_ink);
    let tip = format!("{title}\n{}", view_label(entry));
    (format!("{key_text}  {shown}"), response.on_hover_text(tip))
}

/// `kit::elide` of `text` to `room` px, or nothing when not even the
/// ellipsis fits (P5-602: no text past a row's edge).
fn fit(ui: &egui::Ui, text: &str, font: &egui::FontId, room: f32) -> String {
    let cut = kit::elide(ui, text, font, room);
    let width = ui.fonts_mut(|f| f.layout_no_wrap(cut.clone(), font.clone(), INK).size().x);
    if width <= room { cut } else { String::new() }
}

/// The width of the key column: the widest view or folder key of
/// `folders`, at most 104 px (P5-602).
pub(super) fn key_column(ui: &egui::Ui, folders: &[crate::view_folders::Folder]) -> f32 {
    let code = theme::code_font();
    let width = |text: String| ui.fonts_mut(|f| f.layout_no_wrap(text, code.clone(), INK).size().x);
    folders
        .iter()
        .flat_map(|f| std::iter::once(f.folder.key().to_owned()).chain(f.views.iter().map(|v| v.view.key())))
        .map(width)
        .fold(0.0_f32, f32::max)
        .min(104.0)
}

/// Room for the fold dot (P5-603) beside a header's count, px.
const DOT_ROOM: f32 = 10.0;
/// The recap card's widest line, px (P5-607).
const RECAP_W: f32 = 260.0;

/// One folder header of the view list (P5-602, P5-603, P5-607): a
/// chevron, the folder key in the key column's font and colour, the
/// label, the view count right-aligned in `ink-3`; folded and holding the
/// view on screen, an `accent` dot left of the count. Hovering shows the
/// recap card. Returns the row's text (`▾`/`▸`, key, label, count, `•`
/// for the dot) and response; the caller folds on a click (pointer, or
/// Enter or Space while focused).
pub(super) fn folder_header(ui: &mut egui::Ui, group: &crate::view_folders::Folder, folded: bool, key_w: f32) -> (String, egui::Response) {
    let width = ui.available_width();
    let (rect, response) = ui.allocate_exact_size(Vec2::new(width, SIZE_ROW), Sense::click());
    if response.hovered() {
        ui.painter().rect_filled(rect, CornerRadius::same(2), BG_RAISED);
    }
    let cy = rect.center().y;
    let x0 = rect.min.x + 8.0;
    let chevron = Rect::from_center_size(egui::pos2(x0 + theme::SIZE_ICON / 2.0, cy), Vec2::splat(theme::SIZE_ICON));
    icons::paint(ui.painter(), if folded { Icon::ChevronRight } else { Icon::ChevronDown }, chevron, INK_3);
    let code = theme::code_font();
    let count = format!("{:02}", group.views.len());
    let count_w = ui.fonts_mut(|f| f.layout_no_wrap(count.clone(), code.clone(), INK_3).size().x);
    let count_x = rect.max.x - 8.0;
    ui.painter().text(egui::pos2(count_x, cy), Align2::RIGHT_CENTER, &count, code.clone(), INK_3);
    let dot = folded && group.holds_current();
    let mut right = count_x - count_w - 6.0;
    if dot {
        ui.painter().circle_filled(egui::pos2(right - DOT_ROOM / 2.0, cy), 3.0, ACCENT);
        right -= DOT_ROOM;
    }
    let key_x = x0 + theme::SIZE_ICON;
    let key_w = key_w.min((right - key_x).max(0.0));
    let key = fit(ui, group.folder.key(), &code, key_w);
    let key_rect = ui.painter().text(egui::pos2(key_x, cy), Align2::LEFT_CENTER, &key, code, INK);
    let font = theme::ui_font();
    // DECISION: the label follows the short folder key rather than the
    // key column, so it keeps its whole word in a narrow panel.
    let label_x = key_rect.max.x + 8.0;
    let label = fit(ui, group.folder.label(), &font, (right - label_x).max(0.0));
    ui.painter().text(egui::pos2(label_x, cy), Align2::LEFT_CENTER, &label, font, INK_2);
    kit::focus_ring(ui, &response);
    let which = group.folder;
    let response = response.on_hover_ui(move |ui| {
        ui.set_max_width(RECAP_W);
        ui.spacing_mut().item_spacing.y = 4.0;
        ui.label(egui::RichText::new(which.recap_title()).font(theme::strong_font()).color(INK));
        ui.label(egui::RichText::new(which.recap()).color(INK_2));
    });
    let chev = if folded { '▸' } else { '▾' };
    let mark = if dot { " •" } else { "" };
    (format!("{chev} {key}  {label}  {count}{mark}"), response)
}

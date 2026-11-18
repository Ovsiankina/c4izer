//! The dynamic-view part of the editor state: step lines selected as
//! steps, their details and in-place text, the Steps list and step
//! reordering (SRS 4.7 P3-218..P3-221).
//!
//! Depends on: `state`, `c4rizer-core` (model, edit, scene, view).

// DECISION: a step is selected by its index into `ViewSpec::steps` of
// the open view (`Selected::Step`): steps have no key in the file, and
// the index survives a re-parse that changes no step line; a reorder
// moves the selection with the moved step.
// DECISION: "number order" of the Steps list is Structurizr 6.2.4's
// `DynamicView.getRelationships` order: by numeric value when every
// number is numeric, else by the number as text; ties keep file order.
// DECISION: a step moves past its neighbour in file order (the step
// before or after it); with no written order and inside one block that
// is also its neighbour in number order.

use std::cmp::Ordering;

use c4rizer_core::edit::TextTarget;
use c4rizer_core::model::{DynamicStep, Model, ViewKind, ViewSpec};
use c4rizer_core::scene::{ItemKey, Scene};
use c4rizer_core::view::EdgeId;

use super::{Action, Details, Effect, Selected, State, TextField};
use crate::worker::ViewRef;

/// The step part of the details panel (P3-218).
// APPROVER: new pub item
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StepDetails {
    /// The sequence number as drawn.
    pub order: String,
    /// `<source name> -> <destination name>`.
    pub ends: String,
    /// The relationship the step stands for, as `<source name> ->
    /// <destination name>: <description>`.
    pub relationship: String,
}

/// One row of the Steps list beside a dynamic view (P3-219).
// APPROVER: new pub item
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StepRow {
    /// Index into `ViewSpec::steps`.
    pub step: u32,
    /// `<number>: <description>`, as the line's label reads.
    pub label: String,
    /// True when the step's line is selected.
    pub selected: bool,
}

impl State {
    /// The Steps list of the open view, in number order; empty unless it
    /// is a dynamic view (P3-219).
    #[must_use]
    pub fn steps(&self) -> Vec<StepRow> {
        let (Some(model), Some(view)) = (self.model(), self.view.as_ref()) else {
            return Vec::new();
        };
        let Some(spec) = dynamic_spec(model, view) else {
            return Vec::new();
        };
        number_order(spec)
            .into_iter()
            .map(|i| {
                let step = u32::try_from(i).unwrap_or(u32::MAX);
                StepRow {
                    step,
                    label: label(model, &spec.steps[i]),
                    selected: self.selection.contains(&Selected::Step(step)),
                }
            })
            .collect()
    }

    /// `Command::SelectStep` (P3-219): the step's line becomes the
    /// selection.
    pub(super) fn select_step(&mut self, step: u32) -> Vec<Effect> {
        let exists = match (self.model(), self.view.as_ref()) {
            (Some(model), Some(view)) => {
                dynamic_spec(model, view).is_some_and(|spec| (step as usize) < spec.steps.len())
            }
            _ => false,
        };
        if exists {
            self.selection.clear();
            self.selection.insert(Selected::Step(step));
        }
        Vec::new()
    }

    /// `Command::MoveStep` (P3-220, P3-221): swaps the step with its
    /// neighbour, or refuses with a status notice and writes nothing.
    pub(super) fn move_step(&mut self, step: u32, up: bool) -> Vec<Effect> {
        let (Some(model), Some(view)) = (self.model(), self.view.clone()) else {
            return Vec::new();
        };
        let Some(spec) = dynamic_spec(model, &view) else {
            return Vec::new();
        };
        let other = if up { step.checked_sub(1) } else { step.checked_add(1) };
        let (Some(this), Some(other), Some(that)) = (
            spec.steps.get(step as usize),
            other,
            other.and_then(|o| spec.steps.get(o as usize)),
        ) else {
            self.notice = Some(format!(
                "step not moved: it is already the {} step",
                if up { "first" } else { "last" }
            ));
            return Vec::new();
        };
        // P3-221 (ruling Y2): written orders are never renumbered, and a
        // step stays in its block.
        if spec.steps.iter().any(|s| s.written_order.is_some()) {
            self.notice = Some(
                "step not moved: this view numbers its steps with written orders; change the numbers in the file instead"
                    .to_owned(),
            );
            return Vec::new();
        }
        if this.parallel != that.parallel {
            self.notice = Some("step not moved: a step cannot move across a parallel block's border".to_owned());
            return Vec::new();
        }
        let first = step.min(other);
        let effects = self.write(Action::SwapSteps { view, first });
        if !effects.is_empty() && self.selection.remove(&Selected::Step(step)) {
            self.selection.insert(Selected::Step(other));
        }
        effects
    }

    /// The details of step `step` of the open view (P3-218).
    pub(super) fn step_details(&self, step: u32, subject: Selected) -> Option<Details> {
        let model = self.model()?;
        let spec = dynamic_spec(model, self.view.as_ref()?)?;
        let s = spec.steps.get(step as usize)?;
        let r = model.relationship(s.relationship);
        let name = |e| model.element(e).name.value.clone();
        let description = s.description.as_ref().or(r.description.as_ref()).map(|d| d.value.clone());
        let technology = s.technology.as_ref().or(r.technology.as_ref()).map(|t| t.value.clone());
        let ends = format!("{} -> {}", name(s.source.value), name(s.destination.value));
        let relationship = format!(
            "{} -> {}: {}",
            name(r.source.value),
            name(r.destination.value),
            r.description.as_ref().map_or("", |d| d.value.as_str())
        );
        Some(Details {
            subject,
            name: label(model, s),
            kind: "Step".to_owned(),
            description,
            technology,
            tags: r.tags.clone(),
            lifted_from: None,
            merged: Vec::new(),
            instances: None,
            environment: None,
            step: Some(StepDetails {
                order: s.order.clone(),
                ends,
                relationship,
            }),
        })
    }
}

/// The in-place text field of step `step` (P3-218): the step's own
/// description, empty when it has none (the relationship's text is not
/// copied into the file by an untouched Enter).
pub(super) fn step_text_fields(model: &Model, view: &ViewRef, step: u32) -> Vec<TextField> {
    let Some(spec) = dynamic_spec(model, view) else {
        return Vec::new();
    };
    let Some(s) = spec.steps.get(step as usize) else {
        return Vec::new();
    };
    let original = s.description.as_ref().map(|d| d.value.clone());
    vec![TextField {
        target: TextTarget::Step {
            view: spec.key.clone(),
            step,
        },
        label: "description",
        buffer: original.clone().unwrap_or_default(),
        original,
    }]
}

/// The scene line drawing step `step` of the scene's view.
pub(super) fn step_items(scene: &Scene, model: &Model, step: u32) -> Vec<ItemKey> {
    if scene.kind != ViewKind::Dynamic {
        return Vec::new();
    }
    let Some(spec) = model.view(&scene.key) else {
        return Vec::new();
    };
    spec.drawn_steps()
        .iter()
        .position(|&i| i == step as usize)
        .map(|edge| ItemKey::Line(EdgeId(u32::try_from(edge).unwrap_or(u32::MAX))))
        .filter(|item| scene.lines.iter().any(|l| ItemKey::Line(l.edge) == *item))
        .into_iter()
        .collect()
}

/// The selection a hit line or label makes on a dynamic view: its step
/// (P3-219); `None` for any other item or view.
pub(super) fn step_selected(scene: &Scene, model: &Model, item: ItemKey) -> Option<Selected> {
    if scene.kind != ViewKind::Dynamic {
        return None;
    }
    let (ItemKey::Line(edge) | ItemKey::Label(edge)) = item else {
        return None;
    };
    let index = *model.view(&scene.key)?.drawn_steps().get(edge.0 as usize)?;
    Some(Selected::Step(u32::try_from(index).ok()?))
}

/// The declared dynamic view `view` names, if it is one.
fn dynamic_spec<'m>(model: &'m Model, view: &ViewRef) -> Option<&'m ViewSpec> {
    model.view(&view.key()).filter(|v| v.kind == ViewKind::Dynamic)
}

/// `<number>: <description>`, the step's line label (P3-211).
fn label(model: &Model, step: &DynamicStep) -> String {
    let text = step
        .description
        .as_ref()
        .or(model.relationship(step.relationship).description.as_ref())
        .map_or("", |d| d.value.as_str());
    format!("{}: {text}", step.order).trim_end().to_owned()
}

/// The indices of `spec.steps` in number order.
fn number_order(spec: &ViewSpec) -> Vec<usize> {
    let numeric: Option<Vec<f64>> = spec.steps.iter().map(|s| s.order.parse::<f64>().ok()).collect();
    let mut out: Vec<usize> = (0..spec.steps.len()).collect();
    match numeric {
        Some(values) => out.sort_by(|&a, &b| values[a].partial_cmp(&values[b]).unwrap_or(Ordering::Equal)),
        None => out.sort_by(|&a, &b| spec.steps[a].order.cmp(&spec.steps[b].order)),
    }
    out
}

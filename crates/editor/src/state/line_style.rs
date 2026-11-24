//! The Style section of a line selection (SRS 4.9 P5-357; owner request
//! 2026-11-23, OR-15 / issue #5): what it shows and its writes, the line
//! counterpart of `State::style_panel` and `Command::SetStyle`. Every
//! write is one `edit::Edit` batch, one undo step.
//!
//! Depends on: `state`, `c4rizer-core` (edit, model, scene).

use c4rizer_core::edit::{LineStyleProperty, LineStyleTarget, RelationshipRef};
use c4rizer_core::model::{Colour, RelationshipId};

use super::{Action, Effect, Selected, State};
use crate::worker::ViewRef;

/// The Style section of a line selection (P5-357).
// PENDING SIGN-OFF: new pub item (owner request 2026-11-23, OR-15 / issue #5)
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LineStylePanel {
    /// The colour every selected line is drawn in on the shown scene,
    /// `None` when they differ.
    pub color: Option<Colour>,
    /// Tags every selected line carries: the `all <tag>` choices.
    pub tags: Vec<String>,
    /// Number of lines the change applies to.
    pub lines: usize,
}

impl State {
    /// The selected lines' relationships, `None` unless the selection is
    /// lines only (no box, title or step) and not empty.
    fn selected_lines_only(&self) -> Option<Vec<RelationshipRef>> {
        let mut out = Vec::new();
        for s in &self.selection {
            match s {
                Selected::Relationship(r) => out.push(r.clone()),
                Selected::Element(_) | Selected::Title | Selected::Step(_) => return None,
            }
        }
        (!out.is_empty()).then_some(out)
    }

    /// The Style section of a line selection (P5-357): `None` unless only
    /// lines are selected.
    // DECISION: the colour is read from the scene on screen, as the box
    // section's swatches are (`shown_style`): it shows the colour the
    // line is drawn in, end-box colour included (P5-300); with no drawn
    // line for a relationship the style's colour, else none.
    #[must_use]
    pub fn line_style_panel(&self) -> Option<LineStylePanel> {
        let model = self.model()?;
        let ids: Vec<RelationshipId> = self
            .selected_lines_only()?
            .iter()
            .map(|r| super::resolve_relationship(model, r))
            .collect::<Option<_>>()?;
        let scene = self.scene();
        let colours: Vec<Option<Colour>> = ids
            .iter()
            .map(|&id| {
                scene
                    .and_then(|s| s.lines.iter().find(|l| l.relationship == id))
                    .map(|l| l.stroke.colour)
                    .or_else(|| model.relationship_style_of(id).color)
            })
            .collect();
        let color = colours.iter().all(|c| *c == colours[0]).then_some(colours[0]).flatten();
        let first = model.relationship(ids[0]);
        let tags = first
            .tags
            .iter()
            .filter(|t| ids.iter().all(|&id| model.relationship(id).tags.contains(t)))
            .cloned()
            .collect();
        Some(LineStylePanel {
            color,
            tags,
            lines: ids.len(),
        })
    }

    /// `Command::SetLineStyle`: every selected line, one undo step.
    pub(super) fn set_line_style(&mut self, target: LineStyleTarget, property: LineStyleProperty) -> Vec<Effect> {
        let (Some(relationships), Some(view)) = (self.selected_lines_only(), self.view.clone()) else {
            return Vec::new();
        };
        self.write(Action::SetLineStyle {
            view,
            relationships,
            target,
            property,
        })
    }

    /// `Command::ResetStyle` with lines and no box selected.
    pub(super) fn reset_line_style(&mut self, view: ViewRef) -> Vec<Effect> {
        let Some(relationships) = self.selected_lines_only() else {
            return Vec::new();
        };
        self.write(Action::ResetLineStyle { view, relationships })
    }
}

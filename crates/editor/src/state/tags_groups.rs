//! Tags and groups of the selection (SRS 4.9 P5-106..P5-109,
//! P5-204..P5-206; owner requests 2026-11-23, issues #1 and #2): the
//! Details section's Tags and Group rows, the add-tag and group-name
//! popups, the context menu's group entries and their keys. Every write
//! is one `edit::Edit` batch, one undo step.
//!
//! Depends on: `state`, `c4rizer-core` (edit, model).

use c4rizer_core::edit::TagTarget;
use c4rizer_core::model::{ElementId, ElementKey, ElementKind, Model};

use super::{Action, Effect, Key, MenuAction, MenuEntry, Overlay, Selected, State};

/// The Tags row of the selection (P5-204, P5-206): the tags every
/// selected item carries, and which of them every item declares.
// PENDING SIGN-OFF: new pub item (owner request 2026-11-23, OR-12 / issue #2)
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TagRow {
    /// Tags every selected item carries, in the first item's order.
    pub tags: Vec<String>,
    /// The tags of `tags` every selected item declares: they show the
    /// remove mark.
    pub removable: Vec<String>,
    /// True for a selection of lines, false for boxes.
    pub lines: bool,
}

/// The Group row of a box selection (P5-106, P5-109).
// PENDING SIGN-OFF: new pub item (owner request 2026-11-23, OR-11 / issue #1)
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GroupRow {
    /// The group every selected box is in (its name as written), `None`
    /// when none is in a group or they differ.
    pub current: Option<String>,
    /// True when the selected boxes are in different groups (or some in
    /// none).
    pub mixed: bool,
    /// The groups declared directly in the boxes' shared parent scope,
    /// in file order.
    pub choices: Vec<String>,
    /// False when the boxes do not share a parent scope (P5-109).
    pub enabled: bool,
    /// True when a selected box is in a group: "Remove from group" runs.
    pub grouped: bool,
}

/// The kinds a group holds (P5-100).
const fn groupable(kind: ElementKind) -> bool {
    matches!(
        kind,
        ElementKind::Person | ElementKind::SoftwareSystem | ElementKind::Container | ElementKind::Component
    )
}

/// What the selection names for tags: elements or relationships, never
/// both (P5-206).
enum Targets {
    Boxes(Vec<ElementId>),
    Lines(Vec<c4rizer_core::model::RelationshipId>),
}

impl State {
    /// The selection as tag targets, `None` when empty, mixed (boxes and
    /// lines) or holding anything else (title, step).
    fn tag_targets(&self, model: &Model) -> Option<Targets> {
        let mut boxes = Vec::new();
        let mut lines = Vec::new();
        for s in &self.selection {
            match s {
                Selected::Element(k) => boxes.push(model.find(k)?),
                Selected::Relationship(r) => lines.push(super::resolve_relationship(model, r)?),
                Selected::Title | Selected::Step(_) => return None,
            }
        }
        match (boxes.is_empty(), lines.is_empty()) {
            (false, true) => Some(Targets::Boxes(boxes)),
            (true, false) => Some(Targets::Lines(lines)),
            _ => None,
        }
    }

    /// The Tags row of the selection (P5-204, P5-206): `None` when the
    /// selection is empty, mixed, or the file is read-only (the panel
    /// then shows plain chips, as before).
    // DECISION: a tag shows the remove mark only when every selected
    // item declares it, so `RemoveTag` is never refused for one of them.
    #[must_use]
    pub fn tag_row(&self) -> Option<TagRow> {
        // Per item: its effective tags and its declared tags.
        type Each<'m> = Vec<(&'m [String], Vec<String>)>;
        if self.read_only() {
            return None;
        }
        let model = self.model()?;
        let (each, lines): (Each<'_>, bool) = match self.tag_targets(model)? {
            Targets::Boxes(ids) => (
                ids.iter()
                    .map(|&id| {
                        let e = model.element(id);
                        (e.tags.as_slice(), declared(&e.declared_tags))
                    })
                    .collect(),
                false,
            ),
            Targets::Lines(ids) => (
                ids.iter()
                    .map(|&id| {
                        let r = model.relationship(id);
                        (r.tags.as_slice(), declared(&r.declared_tags))
                    })
                    .collect(),
                true,
            ),
        };
        let (first, _) = each.first()?;
        let tags: Vec<String> = first
            .iter()
            .filter(|t| each.iter().all(|(tags, _)| tags.contains(t)))
            .cloned()
            .collect();
        let removable = tags
            .iter()
            .filter(|t| each.iter().all(|(_, declared)| declared.contains(t)))
            .cloned()
            .collect();
        Some(TagRow { tags, removable, lines })
    }

    /// The existing tags of the selection's kind (P5-205): declared on any
    /// element (resp. relationship) or named by an `element` (resp.
    /// `relationship`) style, minus the tags every selected item carries,
    /// sorted without case. Computed when the popup opens, not per frame.
    fn add_tag_choices(&self, lines: bool) -> Vec<String> {
        let Some(model) = self.model() else { return Vec::new() };
        let mut all: Vec<String> = if lines {
            model
                .relationships
                .iter()
                .flat_map(|r| declared(&r.declared_tags))
                .chain(model.relationship_styles.iter().map(|s| s.tag.value.clone()))
                .collect()
        } else {
            model
                .elements
                .iter()
                .flat_map(|e| declared(&e.declared_tags))
                .chain(model.element_styles.iter().map(|s| s.tag.value.clone()))
                .collect()
        };
        let carried = self.tag_row().map(|r| r.tags).unwrap_or_default();
        all.retain(|t| !t.is_empty() && !carried.contains(t));
        all.sort_by_key(|t| (t.to_lowercase(), t.clone()));
        all.dedup();
        all
    }

    /// The add-tag popup's rows: its choices containing the typed text,
    /// any case (P5-205); empty when the popup is closed.
    #[must_use]
    pub fn tag_popup_rows(&self) -> Vec<String> {
        let Some(Overlay::AddTag { query, choices }) = &self.overlay else {
            return Vec::new();
        };
        let q = query.trim().to_lowercase();
        choices.iter().filter(|c| c.to_lowercase().contains(&q)).cloned().collect()
    }

    /// Opens the add-tag popup, or closes it when open (P5-204).
    pub(super) fn toggle_tag_popup(&mut self) -> Vec<Effect> {
        if matches!(self.overlay, Some(Overlay::AddTag { .. })) {
            self.overlay = None;
            return Vec::new();
        }
        let Some(row) = self.tag_row() else { return Vec::new() };
        self.overlay = Some(Overlay::AddTag {
            query: String::new(),
            choices: self.add_tag_choices(row.lines),
        });
        Vec::new()
    }

    /// Adds (`add`) or removes `tag` on every selected item, one undo step
    /// (P5-204..P5-206). Closes the add-tag popup.
    pub(super) fn write_tag(&mut self, tag: &str, add: bool) -> Vec<Effect> {
        if matches!(self.overlay, Some(Overlay::AddTag { .. })) {
            self.overlay = None;
        }
        let tag = tag.trim();
        if tag.is_empty() {
            return Vec::new();
        }
        let targets: Vec<TagTarget> = self
            .selection
            .iter()
            .filter_map(|s| match s {
                Selected::Element(k) => Some(TagTarget::Element(k.clone())),
                Selected::Relationship(r) => Some(TagTarget::Relationship(r.clone())),
                _ => None,
            })
            .collect();
        if targets.is_empty() || self.tag_row().is_none() {
            return Vec::new();
        }
        self.write(Action::Tags {
            targets,
            tag: tag.to_owned(),
            add,
        })
    }

    /// The selected boxes, when every one is a groupable element.
    fn group_members(&self, model: &Model) -> Option<Vec<ElementId>> {
        let mut ids = Vec::new();
        for s in &self.selection {
            match s {
                Selected::Element(k) => {
                    let id = model.find(k)?;
                    if !groupable(model.element(id).kind) {
                        return None;
                    }
                    ids.push(id);
                }
                Selected::Relationship(_) => {}
                Selected::Title | Selected::Step(_) => return None,
            }
        }
        (!ids.is_empty()).then_some(ids)
    }

    /// The Group row of the selection (P5-106, P5-109): `None` unless the
    /// selection holds boxes of persons, software systems, containers or
    /// components, and the file can be written.
    #[must_use]
    pub fn group_row(&self) -> Option<GroupRow> {
        if self.read_only() {
            return None;
        }
        let model = self.model()?;
        let ids = self.group_members(model)?;
        let scope = model.element(ids[0]).parent;
        let enabled = ids.iter().all(|&id| model.element(id).parent == scope);
        let group_name = |id: ElementId| {
            model
                .element(id)
                .group
                .and_then(|g| model.groups.get(g.0 as usize))
                .map(|g| g.name.value.clone())
        };
        let first = group_name(ids[0]);
        let mixed = ids.iter().any(|&id| group_name(id) != first);
        let mut choices: Vec<String> = Vec::new();
        if enabled {
            for g in model.groups.iter().filter(|g| g.owner == scope && g.parent.is_none()) {
                if !choices.contains(&g.name.value) {
                    choices.push(g.name.value.clone());
                }
            }
        }
        Some(GroupRow {
            current: if mixed { None } else { first },
            mixed,
            choices,
            enabled,
            grouped: ids.iter().any(|&id| model.element(id).group.is_some()),
        })
    }

    /// The group entries of a box's context menu (P5-108): "Group…" and,
    /// when a selected box is in a group, "Remove from group"; none when
    /// the boxes do not share a parent (the menu greys them, P5-109).
    pub(super) fn group_entries(&self) -> Vec<MenuEntry> {
        let Some(row) = self.group_row().filter(|r| r.enabled) else {
            return Vec::new();
        };
        let mut out = vec![MenuEntry {
            label: "Group…".to_owned(),
            action: MenuAction::Group,
        }];
        if row.grouped {
            out.push(MenuEntry {
                label: "Remove from group".to_owned(),
                action: MenuAction::RemoveFromGroup,
            });
        }
        out
    }

    /// Opens the group-name popup prefilled "Group", or closes it when
    /// open (P5-107); nothing when the boxes do not share a parent.
    pub(super) fn toggle_group_popup(&mut self) -> Vec<Effect> {
        if matches!(self.overlay, Some(Overlay::GroupName { .. })) {
            self.overlay = None;
            return Vec::new();
        }
        if self.group_row().is_some_and(|r| r.enabled) {
            self.overlay = Some(Overlay::GroupName { name: "Group".to_owned() });
        }
        Vec::new()
    }

    /// Writes one `SetGroup` for the selected boxes, one undo step
    /// (P5-106..P5-108); an empty name is refused with a notice and keeps
    /// the popup open (P5-107).
    pub(super) fn write_group(&mut self, group: Option<String>) -> Vec<Effect> {
        let group = match group {
            Some(name) if name.trim().is_empty() => {
                self.notice = Some("a group needs a name".to_owned());
                return Vec::new();
            }
            Some(name) => Some(name.trim().to_owned()),
            None => None,
        };
        if matches!(self.overlay, Some(Overlay::GroupName { .. })) {
            self.overlay = None;
        }
        if !self.group_row().is_some_and(|r| r.enabled) {
            return Vec::new();
        }
        let elements: Vec<ElementKey> = self.selected_elements();
        self.write(Action::SetGroup { elements, group })
    }

    /// A key while the add-tag or group-name popup is open: Enter
    /// confirms, Escape closes; every other key is the text field's.
    /// `None` when neither is open.
    pub(super) fn popup_key(&mut self, key: Key) -> Option<Vec<Effect>> {
        match (&self.overlay, key) {
            (Some(Overlay::AddTag { .. } | Overlay::GroupName { .. }), Key::Escape) => {
                self.overlay = None;
                Some(Vec::new())
            }
            (Some(Overlay::AddTag { query, .. }), Key::Enter) => {
                let tag = query.clone();
                Some(self.write_tag(&tag, true))
            }
            (Some(Overlay::GroupName { name }), Key::Enter) => {
                let name = name.clone();
                Some(self.write_group(Some(name)))
            }
            (Some(Overlay::AddTag { .. } | Overlay::GroupName { .. }), _) => Some(Vec::new()),
            _ => None,
        }
    }

    /// The text field of an open popup took `text`; false when none is
    /// open.
    pub(super) fn popup_text(&mut self, text: &str) -> bool {
        match self.overlay.as_mut() {
            Some(Overlay::AddTag { query, .. }) => text.clone_into(query),
            Some(Overlay::GroupName { name }) => text.clone_into(name),
            _ => return false,
        }
        true
    }
}

/// The declared tags of an item, split and trimmed, in order.
fn declared(literals: &[c4rizer_core::model::Literal]) -> Vec<String> {
    literals
        .iter()
        .flat_map(|l| l.value.split(','))
        .map(str::trim)
        .filter(|t| !t.is_empty())
        .map(str::to_owned)
        .collect()
}

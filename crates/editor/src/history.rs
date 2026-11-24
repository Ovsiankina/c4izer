//! Undo and redo over the edits the app wrote (EDIT-19).
//!
//! Depends on: `c4rizer-core` (edit).

// Shape: two stacks of `Step`s. A step is the batch of semantic edits
// that performs it (`edit::Edit`, keyed by element/view keys, never byte
// patches), so it applies to whatever text is on disk then (EDIT-24).
// Applying a step through `edit::apply` gives its inverse, which goes on
// the other stack; `state` does the applying and the writing, this
// module only keeps the stacks.
//
// Flow: a user action writes edits E and `apply` returns inverse I:
// `record(Step { edits: I })`. Undo: `pop_undo` gives I, `state` applies
// it, gets inverse E', writes, then `push_undone(Step { edits: E' })`.
// Redo is the mirror (`pop_redo`, `push_redone`). A step whose target is
// gone (`EditErrorKind::TargetGone` or `Conflict`) is dropped, not pushed
// back, and the status bar says so (EDIT-19).
//
// DECISION: capacity `MAX_STEPS` = 1000 (EDIT-19: "at least 100"); the
// oldest step is dropped beyond it. Steps are a few edits each, so the
// memory is negligible next to one scene.

use c4rizer_core::edit::Edit;

/// One undo or redo step: the edits that perform it, one `edit::apply`
/// batch, so a multi-box operation is one step (EDIT-17, EDIT-19,
/// EDIT-20, EDIT-26).
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Step {
    /// The batch, in apply order (`edit::Applied::inverse`).
    pub edits: Vec<Edit>,
}

/// Steps kept per stack; the oldest goes beyond it (EDIT-19).
pub const MAX_STEPS: usize = 1000;

/// The undo and redo stacks of one session (EDIT-19). Survives external
/// reloads unchanged: steps are re-applied by key (EDIT-24). Cleared only
/// when another file is opened.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Default)]
pub struct History {
    /// Steps to undo, newest last.
    pub undo: Vec<Step>,
    /// Steps to redo, newest last.
    pub redo: Vec<Step>,
}

impl History {
    /// Records the inverse of a new user action: pushed on `undo` (the
    /// oldest dropped beyond `MAX_STEPS`), `redo` cleared. An empty step
    /// (no edit) is not recorded. Serves EDIT-19.
    pub fn record(&mut self, step: Step) {
        if step.edits.is_empty() {
            return;
        }
        self.undo.push(step);
        if self.undo.len() > MAX_STEPS {
            let extra = self.undo.len() - MAX_STEPS;
            self.undo.drain(..extra);
        }
        self.redo.clear();
    }

    /// Takes the newest undo step, `None` when there is none. Serves
    /// EDIT-19.
    pub fn pop_undo(&mut self) -> Option<Step> {
        self.undo.pop()
    }

    /// Takes the newest redo step, `None` when there is none. Serves
    /// EDIT-19.
    pub fn pop_redo(&mut self) -> Option<Step> {
        self.redo.pop()
    }

    /// Pushes the inverse of an applied undo step on `redo`. Serves
    /// EDIT-19.
    pub fn push_undone(&mut self, step: Step) {
        if step.edits.is_empty() {
            return;
        }
        self.redo.push(step);
        if self.redo.len() > MAX_STEPS {
            let extra = self.redo.len() - MAX_STEPS;
            self.redo.drain(..extra);
        }
    }

    /// Pushes the inverse of an applied redo step on `undo`, keeping
    /// `redo`. Serves EDIT-19.
    pub fn push_redone(&mut self, step: Step) {
        if step.edits.is_empty() {
            return;
        }
        self.undo.push(step);
        if self.undo.len() > MAX_STEPS {
            let extra = self.undo.len() - MAX_STEPS;
            self.undo.drain(..extra);
        }
    }

    /// True when `undo` is not empty (menu state). Serves EDIT-19.
    #[must_use]
    pub fn can_undo(&self) -> bool {
        !self.undo.is_empty()
    }

    /// True when `redo` is not empty (menu state). Serves EDIT-19.
    #[must_use]
    pub fn can_redo(&self) -> bool {
        !self.redo.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use c4rizer_core::model::ElementKey;
    use super::{History, MAX_STEPS, Step};
    use c4rizer_core::edit::Edit;

    fn step(i: usize) -> Step {
        Step {
            edits: vec![Edit::Move {
                view: "v".to_owned(),
                element: ElementKey::from_written("box"),
                centre: Some((i32::try_from(i).unwrap_or(0) * 10, 0)),
            }],
        }
    }

    /// EDIT-19: 100 recorded steps can all be undone, then all redone,
    /// in order; a new record clears redo.
    #[test]
    fn edit_19_stacks_hold_100_steps() {
        let mut h = History::default();
        for i in 0..100 {
            h.record(step(i));
        }
        assert!(h.can_undo() && !h.can_redo());
        for i in (0..100).rev() {
            let s = h.pop_undo().expect("undo step");
            assert_eq!(s, step(i));
            h.push_undone(s);
        }
        assert!(!h.can_undo() && h.can_redo());
        for i in 0..100 {
            let s = h.pop_redo().expect("redo step");
            assert_eq!(s, step(i));
            h.push_redone(s);
        }
        assert_eq!(h.undo.len(), 100);
        h.pop_undo();
        h.push_undone(step(7));
        h.record(step(200));
        assert!(!h.can_redo(), "a new record clears redo");
    }

    /// EDIT-19: beyond `MAX_STEPS` the oldest step is dropped; an empty
    /// step is not recorded.
    #[test]
    fn edit_19_cap_drops_oldest() {
        let mut h = History::default();
        for i in 0..=MAX_STEPS {
            h.record(step(i));
        }
        assert_eq!(h.undo.len(), MAX_STEPS);
        assert_eq!(h.undo[0], step(1), "the oldest went");
        h.record(Step { edits: Vec::new() });
        assert_eq!(h.undo.len(), MAX_STEPS);
        assert_eq!(h.undo.last(), Some(&step(MAX_STEPS)));
    }
}

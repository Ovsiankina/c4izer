//! Semantic edits (move box, resize, set text, set style) turned into
//! byte patches on the original text, touching nothing else (DSL-9, 10,
//! 13). Re-applies an edit to a newer text by id (EDIT-24).
//!
//! Depends on: `text`, `model`.

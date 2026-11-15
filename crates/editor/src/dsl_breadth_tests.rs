//! Step-4 contract tests of the dsl-breadth row, editor part (P2-123,
//! P2-126, P2-136, P2-141, P2-180). Bodies are `todo!()` until step 5.

/// The view list shows an unsupported view marked "not yet supported";
/// selecting it draws nothing and changes no file.
#[test]
fn p2_123_view_list_marks_unsupported() {
    todo!()
}

/// At startup and in the EDIT-27 fallback the editor opens the first
/// supported view, never an unsupported one.
#[test]
fn p2_123_never_opens_unsupported_view_by_itself() {
    todo!()
}

/// The findings panel (EDIT-6) shows every notice of the open file,
/// naming the construct.
#[test]
fn p2_126_notices_in_findings_panel() {
    todo!()
}

/// A change to any included file reloads as EDIT-1 does; a save by
/// rename is seen as EDIT-23 says.
#[test]
fn p2_141_included_file_change_reloads() {
    todo!()
}

/// The app's own write to an included file does not reload (EDIT-22,
/// per-file own-write hash).
#[test]
fn p2_141_own_write_to_included_file_ignored() {
    todo!()
}

/// "Export all views" (EDIT-7) skips unsupported views and writes no
/// file for them.
#[test]
fn p2_180_export_all_skips_unsupported() {
    todo!()
}

/// An error in an included file shows that file's path, line and column
/// (EDIT-11).
#[test]
fn p2_136_editor_error_names_included_file() {
    todo!()
}

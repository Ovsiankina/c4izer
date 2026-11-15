//! Step-4 contract tests of the landscape-filtered row, editor part
//! (P2-305, P2-313, P2-314, P2-338). Bodies are `todo!()` until step 5.

/// In a landscape view a click on an enterable box opens its view as
/// EDIT-15 says, and the breadcrumb is the workspace crumb alone.
// TODO(srs): P2-305, the workspace crumb. Reading taken (B
// landscape-filtered 8): it opens the first `systemLandscape` view when
// the file has one, else the first system context view (EDIT-16).
#[test]
fn p2_305_landscape_click_enters_breadcrumb_is_workspace() {
    todo!()
}

/// A move or resize in a filtered view.
// TODO(srs): P2-313. Reading taken (B landscape-filtered 1): it writes
// the base view's `at`/`size` line, in one undo step (shared layout).
#[test]
fn p2_313_filtered_move_writes_base_view_line() {
    todo!()
}

/// A view that is the base of a filtered view is missing from the view
/// list (`Model::filtered_bases`).
// TODO(srs): P2-314. Reading taken (B landscape-filtered 5): `c4rizer
// render` still writes it; EDIT-15/16 may open it, and it then shows in
// the list until another view is opened.
#[test]
fn p2_314_base_view_hidden_from_view_list() {
    todo!()
}

/// With no last-viewed view, the editor opens the view carrying
/// `default` (`Model::default_view_key`) instead of the first declared.
// TODO(srs): P2-338, several `default` views. Reading taken (B
// landscape-filtered 16): the last one declared wins.
#[test]
fn p2_338_editor_opens_default_view() {
    todo!()
}

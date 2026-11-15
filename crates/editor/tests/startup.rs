//! EDIT-14 smoke tests: launch the editor binary with `--startup-report`
//! and read the ms it prints (HLD §8: one smoke test launches the
//! binary). All need a display, so none is named `perf_*`: the release
//! perf oracle (`--ignored perf_`) runs headless. The QA startup script
//! runs them with `--ignored startup_` on a machine with a display.

// TODO(srs): EDIT-14 budgets (500 ms with `example/`, 300 ms with no
// file) are marked "confirm once the window backend is measured" in the
// SRS; the perf tests pin them as written.

/// EDIT-14: `c4rizer-editor --startup-report` prints one line of whole ms
/// and exits 0.
#[test]
#[ignore = "needs a display; run by the QA startup script"]
fn startup_edit_14_report_prints_ms_and_exits() {
    let _bin = env!("CARGO_BIN_EXE_c4rizer-editor");
    todo!()
}

/// EDIT-14 (plan editor-02 `edit_14_first_view_under_500ms`): with
/// `tests/data/example.dsl` the first frame shows the first view in under
/// 500 ms, release build.
#[test]
#[ignore = "needs a display; release build, run by the QA startup script"]
fn startup_edit_14_first_view_under_500ms() {
    todo!()
}

/// EDIT-14: with no file the start screen shows in under 300 ms, release
/// build.
#[test]
#[ignore = "needs a display; release build, run by the QA startup script"]
fn startup_edit_14_no_file_under_300ms() {
    todo!()
}

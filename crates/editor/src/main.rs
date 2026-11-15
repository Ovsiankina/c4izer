//! c4rizer editor: native desktop window showing the diagram file live,
//! with drill-down, drag, in-place text and style editing.
//!
//! Depends on: `app`, `c4rizer-core`.

// Step-4 contract of a bin crate: internal signatures and struct layouts,
// `todo!()` bodies. Items are `pub` inside private modules so they are
// visible across the crate and nothing outside it.
// DECISION: `dead_code` is expected while every body is `todo!()`; the
// `expect` turns into an "unfulfilled" warning once step 5 wires the
// items, which is the cue to delete it (clippy -D warnings at step 6).
#![expect(dead_code, reason = "step-4 contract: items are wired at step 5; delete this line then")]

mod app;
mod camera;
mod canvas;
mod files;
mod history;
mod state;
mod worker;

use std::path::PathBuf;
use std::process::ExitCode;

/// Command-line arguments: `c4rizer-editor [FILE] [--startup-report]`
/// (EDIT-13, EDIT-14).
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Args {
    /// The diagram file to open, if given (EDIT-13).
    pub file: Option<PathBuf>,
    /// Print ms from process start to the first presented frame and exit
    /// (EDIT-14).
    pub startup_report: bool,
}

/// Parses the arguments after the program name. Serves EDIT-13, EDIT-14.
///
/// # Errors
///
/// A usage sentence (SRS 5.1) for an unknown flag or a second file.
pub fn parse_args(args: impl IntoIterator<Item = String>) -> Result<Args, String> {
    todo!()
}

/// Starts the window (`eframe::run_native`) with `app::App`; exit code 0,
/// or 2 with the usage or window error on stderr. Serves EDIT-13,
/// EDIT-14.
fn main() -> ExitCode {
    todo!()
}

#[cfg(test)]
mod tests {
    /// EDIT-13 (plan editor-01): `c4rizer-editor FILE` gives `file:
    /// Some(FILE)`; no argument gives none (the start screen with Open
    /// and recent files).
    #[test]
    fn edit_13_open_from_argument() {
        todo!()
    }

    /// EDIT-14: `--startup-report` is accepted before or after FILE; an
    /// unknown flag or a second file is a usage error.
    #[test]
    fn edit_14_startup_report_flag() {
        todo!()
    }
}

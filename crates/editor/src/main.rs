//! c4rizer editor: native desktop window showing the diagram file live,
//! with drill-down, drag, in-place text and style editing.
//!
//! Depends on: `app`, `c4rizer-core`.

// Items are `pub` inside private modules so they are visible across the
// crate and nothing outside it.

mod app;
mod camera;
mod canvas;
#[cfg(test)]
mod deployment_view_tests;
#[cfg(test)]
mod dsl_breadth_tests;
#[cfg(test)]
mod dynamic_view_tests;
#[cfg(test)]
mod editor_breadth_tests;
mod explore;
#[cfg(test)]
mod explore_tests;
mod files;
mod history;
#[cfg(test)]
mod landscape_filtered_tests;
mod state;
#[cfg(test)]
mod styles_themes_tests;
#[cfg(test)]
mod testutil;
mod worker;

use std::path::PathBuf;
use std::process::ExitCode;
use std::time::Instant;

use eframe::egui;

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
    const USAGE: &str = "usage: c4rizer-editor [FILE] [--startup-report]";
    let mut out = Args::default();
    let mut only_files = false;
    for arg in args {
        if !only_files && arg == "--" {
            only_files = true;
        } else if !only_files && arg == "--startup-report" {
            out.startup_report = true;
        } else if !only_files && arg.starts_with('-') && arg != "-" {
            return Err(format!("unknown option `{arg}` — {USAGE}"));
        } else if out.file.is_some() {
            return Err(format!("only one file can be opened, `{arg}` is a second one — {USAGE}"));
        } else {
            out.file = Some(PathBuf::from(arg));
        }
    }
    Ok(out)
}

/// Starts the window (`eframe::run_native`) with `app::App`; exit code 0,
/// or 2 with the usage or window error on stderr. Serves EDIT-13,
/// EDIT-14.
fn main() -> ExitCode {
    let began = Instant::now();
    let args = match parse_args(std::env::args().skip(1)) {
        Ok(args) => args,
        Err(usage) => {
            eprintln!("c4rizer-editor: {usage}");
            return ExitCode::from(2);
        }
    };
    let startup = app::Startup {
        began,
        report: args.startup_report,
    };
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("c4rizer editor")
            .with_inner_size([1280.0, 800.0]),
        ..Default::default()
    };
    let file = args.file;
    let result = eframe::run_native(
        "c4rizer-editor",
        options,
        Box::new(move |cc| Ok(Box::new(app::App::new(cc, file, startup)))),
    );
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("c4rizer-editor: cannot open the window: {e}");
            ExitCode::from(2)
        }
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::{Args, parse_args};

    /// EDIT-13 (plan editor-01): `c4rizer-editor FILE` gives `file:
    /// Some(FILE)`; no argument gives none (the start screen with Open
    /// and recent files).
    #[test]
    fn edit_13_open_from_argument() {
        let args = parse_args(["shop.dsl".to_owned()]).expect("one file");
        assert_eq!(args.file, Some(PathBuf::from("shop.dsl")));
        assert!(!args.startup_report);
        assert_eq!(parse_args(Vec::new()), Ok(Args::default()));
    }

    /// EDIT-14: `--startup-report` is accepted before or after FILE; an
    /// unknown flag or a second file is a usage error.
    #[test]
    fn edit_14_startup_report_flag() {
        let s = |v: &[&str]| parse_args(v.iter().map(|a| (*a).to_owned()));
        let before = s(&["--startup-report", "a.dsl"]).expect("flag first");
        let after = s(&["a.dsl", "--startup-report"]).expect("flag last");
        assert_eq!(before, after);
        assert_eq!(
            before,
            Args {
                file: Some(PathBuf::from("a.dsl")),
                startup_report: true
            }
        );
        assert_eq!(s(&["--startup-report"]).map(|a| a.file), Ok(None));
        let unknown = s(&["--bogus"]).expect_err("unknown flag");
        assert!(unknown.contains("--bogus") && unknown.contains("usage"), "{unknown}");
        let second = s(&["a.dsl", "b.dsl"]).expect_err("second file");
        assert!(second.contains("b.dsl") && second.contains("usage"), "{second}");
    }
}

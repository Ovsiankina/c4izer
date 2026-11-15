//! c4rizer CLI: `c4rizer render FILE -o DIR [--png [--width N]]` and
//! `c4rizer lint FILE`, exit code 0 clean, 1 lint errors, 2 parse or I/O
//! error (CLI-1..5).
//!
//! Depends on: `report`, `c4rizer-core`.

// Shape of the bin (step-4 contract of a bin crate: internal signatures
// and layouts, `pub(crate)`):
//
//   main()                         -> ExitCode
//     parse_args(args)             -> Command | UsageError   (exit 2 + USAGE)
//     run(&command, stdin, stdout) -> Exit | CliError        (exit 2)
//       read FILE or stdin         -> String | CliError
//       model::parse(text)         -> Parsed | ModelError    (exit 2)
//       pipeline::build_all(model) -> AllViews
//       per view: report::view, then (render) export::svg [+ export::png]
//                 to `<out>/<view key>.svg|.png`, then report::wrote
//       report::coverage(all.coverage) after every view (CLI-1)
//       Exit::LintErrors if all.has_errors(), else Exit::Clean (CLI-2)
//
// Only the items below are contract; helpers (reading the input, mapping
// `Exit` to `ExitCode`) are step-5 implementation (contract skill: no
// private items at step 4).
//
// `run` writes the report to the `stdout` it is given and touches the file
// system only to read FILE and (render) to create the output directory
// and write the files, so unit tests drive it with in-memory readers and
// writers and a temporary directory. `main` is the only place that reads
// the real arguments, stdin and stdout, prints errors to stderr and turns
// `Exit` into the process exit code.
//
// DECISION: the PNG flags follow SRS CLI-4 and HLD §7, `--png [--width N]`,
// not the plan's `--png WIDTH` (the SRS wins over the plan).
// DECISION: `-o DIR` is required on `render` (HLD §7 and CLI-1 write
// `render FILE -o DIR`, only `[--png [--width N]]` is optional); a
// missing `-o` is `UsageError::MissingOutDir`.
// DECISION: flags may come before or after FILE, each at most once; the
// value of `-o` and `--width` is the next argument (no `--opt=value`
// form); FILE `-` means stdin (plan Review Focus, for agents piping a
// model). `--width` without `--png`, `-o`/`--png`/`--width` on `lint`,
// `render` without `-o`, a second FILE, an unknown flag or subcommand,
// and no subcommand are usage errors: usage text on stderr, exit 2 (plan
// Review Focus).
// DECISION: `-h`/`--help` and `help`/`version` are not phase 1 (P2-22,
// cli-breadth): until then they are an unknown flag or subcommand.
// DECISION: parse and I/O errors go to stderr as one line
// `c4rizer: <path>:<line>:<column>: <what> — <what to change>` (SRS 5.1;
// `ModelError`'s `Display` gives `line:column: ...` and asks the caller to
// prefix the path of `span.file`; in phase 1 that is always FILE); stdin
// is named `<stdin>`. The report goes to stdout.
// DECISION: a file with no views prints `no views` on stdout and exits 0
// (pipeline plan: "CLI reports \"no views\""; CLI-2: no view has a lint
// error).
// DECISION: render writes every view's files even when its lint has
// errors (the prototype writes the SVG whenever it has one); the exit
// code still says 1. A view's files are written after its findings are
// printed and before its `wrote` lines, so `wrote` is only printed for a
// file that exists. The first write failure stops the run with exit 2.
// DECISION: the output directory is created (with parents) when missing
// (plan Review Focus); a directory that cannot be created or written is
// exit 2 naming the path.
// DECISION: two view keys equal up to ASCII case (`C1`, `c1`; model's
// duplicate check is exact-match) would overwrite each other's files on
// a case-insensitive file system, so `render` stops with exit 2 naming
// both keys (`CliError::KeyCollision`) before creating the directory or
// writing anything. `lint` writes nothing and is unaffected.
// DECISION: no hot struct here (an argument list and one report per
// run), so no `#[repr]` and no size pin. `Exit` carries no discriminants:
// the variant-to-code mapping is a `match` in step 5, which leaves room
// for P2-22 `inspect` (exit code = violation count).
//
// TODO(deferred): `--png` needs the `png` feature of `c4rizer-core`
// forwarded by a `png` feature of this crate (a feature-flag decision,
// docs/deferred.md row "cli ... png feature"), and the bundled font bytes
// that `export::png` takes (docs/deferred.md row "scene/export"
// FONT_BYTES). Until then `--png` parses but `run` returns
// `CliError::PngUnavailable` (exit 2 with a clear message, plan Review
// Focus) and `cli_4_png_written` is `#[ignore = "deferred"]`.

mod report;

use std::ffi::OsString;
use std::fmt;
use std::io::{self, Read, Write};
use std::num::NonZeroU32;
use std::path::PathBuf;
use std::process::ExitCode;

use c4rizer_core::model::ModelError;

/// The usage text printed to stderr on every usage error (exit 2). Names
/// both phase-1 subcommands and their flags (CLI-1, CLI-3, CLI-4).
pub(crate) const USAGE: &str = "\
usage: c4rizer render FILE -o DIR [--png [--width N]]
       c4rizer lint FILE
FILE `-` reads the diagram from stdin.
exit code: 0 clean, 1 lint errors, 2 parse, I/O or usage error";

/// The outcome of a run that got as far as the report (CLI-2). `main`
/// turns it into the process exit code; a `UsageError` or `CliError` is
/// exit 2.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) enum Exit {
    /// No view has a lint error and coverage (L22) is clean: exit 0.
    Clean,
    /// Some view has a lint error, or coverage failed
    /// (`pipeline::AllViews::has_errors`): exit 1.
    LintErrors,
}

/// Where the diagram text comes from. Serves CLI-1, CLI-3.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub(crate) enum Input {
    /// A file path, as given (not canonicalized), named so in errors.
    Path(PathBuf),
    /// FILE given as `-`: read stdin to the end; named `<stdin>` in
    /// errors (plan Review Focus).
    Stdin,
}

/// The `--png` request of `render` (CLI-4).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) struct Png {
    /// `--width N` in px; `None` means twice the SVG width
    /// (`scene.bounds` width), per view. Zero is a usage error, hence
    /// `NonZeroU32`.
    pub(crate) width: Option<NonZeroU32>,
}

/// A parsed command line. Serves CLI-1, CLI-3, CLI-4.
// Phase 2 (P2-22, cli-breadth) adds `validate`, `inspect`, `list`,
// `version`, `help` as new variants.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub(crate) enum Command {
    /// `c4rizer render FILE -o DIR [--png [--width N]]`: report and
    /// write `<view key>.svg` (and `.png`) per view (CLI-1, CLI-4).
    Render {
        /// FILE.
        input: Input,
        /// `-o DIR`, required (header DECISION).
        out_dir: PathBuf,
        /// `--png [--width N]`, `None` without `--png`.
        png: Option<Png>,
    },
    /// `c4rizer lint FILE`: the same checks and report, no file written
    /// (CLI-3).
    Lint {
        /// FILE.
        input: Input,
    },
}

/// A command line that is not one of the `USAGE` forms. `Display` is one
/// line saying what is wrong; `main` prints it, then `USAGE`, to stderr
/// and exits 2 (plan Review Focus). Serves CLI-2.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub(crate) enum UsageError {
    /// No subcommand at all.
    NoCommand,
    /// A first argument that is not `render` or `lint`, as given (lossy
    /// UTF-8).
    UnknownCommand(String),
    /// An argument starting with `-` (other than FILE `-`) that the
    /// subcommand does not take, as given.
    UnknownFlag(String),
    /// The subcommand got no FILE.
    MissingFile,
    /// `render` without `-o DIR`.
    MissingOutDir,
    /// A second FILE, or any other extra argument, as given.
    ExtraArgument(String),
    /// A flag given twice (`-o`, `--png`, `--width`).
    Repeated(&'static str),
    /// `-o` or `--width` as the last argument, with no value.
    MissingValue(&'static str),
    /// `--width` with a value that is not a whole number from 1 to
    /// `u32::MAX`, as given.
    InvalidWidth(String),
    /// `--width` without `--png`.
    WidthWithoutPng,
}

impl fmt::Display for UsageError {
    /// One line, e.g. `unknown flag "--pdf"`. Serves CLI-2, SRS 5.1.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        todo!()
    }
}

impl std::error::Error for UsageError {}

/// A failure of `run` that ends it with exit 2 (CLI-2). `Display` is the
/// one line `main` prints to stderr after `c4rizer: `, naming the file and
/// saying what to change (SRS 5.1). Usage errors are `UsageError`, never
/// this type. Serves CLI-2, CLI-1, CLI-4.
#[derive(Debug)]
pub(crate) enum CliError {
    /// FILE (or stdin) could not be read, or is not UTF-8.
    Read {
        /// What was read.
        input: Input,
        /// Why.
        source: io::Error,
    },
    /// FILE is not a valid diagram (DSL-4, DSL-5): printed as
    /// `<path>:<line>:<column>: ...`, where `<path>` is the path of
    /// `error.span.file`; in phase 1 always FILE (`<stdin>` for `-`).
    Parse {
        /// What was parsed.
        input: Input,
        /// The model error, whose `Display` starts at `line:column:`.
        error: ModelError,
    },
    /// `render` on a file with two view keys equal up to ASCII case
    /// (header DECISION): nothing was written.
    KeyCollision {
        /// The key declared first.
        first: String,
        /// The key declared later.
        second: String,
    },
    /// The output directory is missing and could not be created (plan
    /// Review Focus).
    CreateDir {
        /// The directory.
        path: PathBuf,
        /// Why.
        source: io::Error,
    },
    /// An SVG or PNG file could not be written (plan Review Focus: names
    /// the path).
    Write {
        /// The file.
        path: PathBuf,
        /// Why.
        source: io::Error,
    },
    /// The report could not be written to stdout (e.g. a closed pipe).
    Report(io::Error),
    /// `--png` on a build without PNG support (no `png` feature, or no
    /// bundled font): says to rebuild with the feature (plan Review
    /// Focus).
    PngUnavailable,
    /// `export::png` refused a view (CLI-4, EXP-2).
    // DECISION: the `PngError` is kept as its `Display` text, so this type
    // compiles whether or not core's `png` feature is on.
    Png {
        /// The view key.
        view: String,
        /// `PngError`'s `Display` text.
        message: String,
    },
}

impl fmt::Display for CliError {
    /// One line naming the path or view and what to change, e.g.
    /// `out/c1.svg: permission denied — choose a writable -o directory`.
    /// Serves CLI-2, SRS 5.1.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        todo!()
    }
}

impl std::error::Error for CliError {}

/// Parses the arguments after the program name (`std::env::args_os()`
/// minus the first) into a `Command`. Paths keep their `OsString` bytes;
/// flags and subcommands must be UTF-8 to match. Serves CLI-1, CLI-3,
/// CLI-4.
///
/// # Errors
///
/// A `UsageError` for anything that is not one of the `USAGE` forms
/// (header DECISION lists them).
pub(crate) fn parse_args<I>(args: I) -> Result<Command, UsageError>
where
    I: IntoIterator<Item = OsString>,
{
    todo!()
}

/// Runs `command`: reads the input (the whole file, or `stdin` read to
/// the end for `Input::Stdin`; never written to), parses it
/// (`model::parse`), builds every view (`pipeline::build_all`), prints the
/// report to `stdout` (`report::view` per view in declaration order,
/// `report::no_views` when there is none, `report::coverage` after all
/// views) and, for `render`, creates the output directory and writes
/// `<view key>.svg` (`export::svg`) and, with `--png`, `<view key>.png`
/// (`export::png`) per view, each followed by its `report::wrote` line.
/// `lint` writes no file (CLI-3). Serves CLI-1, CLI-2, CLI-3, CLI-4.
///
/// # Errors
///
/// A `CliError` (exit 2) on a read (including non-UTF-8 bytes), parse,
/// key-collision, directory, write or report failure, or `PngUnavailable` for `--png` without PNG support (checked
/// before any file is read). Lint findings are not errors: they give
/// `Ok(Exit::LintErrors)`.
pub(crate) fn run(
    command: &Command,
    stdin: &mut impl Read,
    stdout: &mut impl Write,
) -> Result<Exit, CliError> {
    todo!()
}

/// Parses `std::env::args_os()`; on a `UsageError` prints
/// `c4rizer: <error>` and `USAGE` to stderr and returns exit 2. Otherwise
/// runs the command on the real stdin and stdout and returns 0 for
/// `Exit::Clean`, 1 for `Exit::LintErrors`; on a `CliError` prints
/// `c4rizer: <error>` to stderr and returns exit 2. Serves CLI-2.
fn main() -> ExitCode {
    todo!()
}

#[cfg(test)]
mod tests {
    /// CLI-2: `run` gives `Exit::Clean` on a file whose views lint clean,
    /// `Exit::LintErrors` when one view has an error or L22 fires, and a
    /// `CliError` on a parse error and on a missing file. The mapping to
    /// process codes 0/1/2 is checked end to end in
    /// `tests/cli.rs::cli_2_binary_exit_codes`.
    #[test]
    fn cli_2_exit_codes() {
        todo!()
    }

    /// Plan cli-01 / Review Focus: an unknown flag (`--pdf`), an unknown
    /// subcommand and no subcommand are `UsageError`s, which `main` turns
    /// into the usage text on stderr and exit 2.
    #[test]
    fn unknown_flag_prints_usage() {
        todo!()
    }

    /// Review Focus: `render FILE -o DIR --png --width 800` parses with
    /// flags in any order around FILE; `render FILE` without `-o`
    /// (`MissingOutDir`), `--width` without `--png`, a repeated flag,
    /// `--width 0`, and `-o` with no value are usage errors; `lint`
    /// rejects `-o` and `--png`.
    #[test]
    fn cli_4_png_and_width_flags_parse() {
        todo!()
    }

    /// Review Focus: FILE `-` parses to `Input::Stdin`, and `run` reads
    /// the model from the given stdin.
    #[test]
    fn stdin_dash_reads_model() {
        todo!()
    }

    /// Review Focus: a missing output directory is created (with
    /// parents) and the SVGs land in it.
    #[test]
    fn output_dir_created() {
        todo!()
    }

    /// Review Focus: an output path that cannot be written (a regular
    /// file given as `-o`) is `CliError::CreateDir` or `Write` naming the
    /// path, exit 2.
    #[test]
    fn output_dir_not_writable_exits_2() {
        todo!()
    }

    /// Review Focus: `--png` on a build without PNG support is
    /// `CliError::PngUnavailable` (exit 2) before any file is read or
    /// written. Becomes `#[cfg(not(feature = "png"))]` in the same change
    /// as docs/deferred.md row "cli png feature" (it cannot be gated now:
    /// a cfg on a feature that does not exist trips `unexpected_cfgs`).
    #[test]
    fn png_without_feature_exits_2() {
        todo!()
    }

    /// SRS 5.1: a parse error's `Display` is
    /// `<path>:<line>:<column>: <what> — <what to change>`, and
    /// `<stdin>:...` for FILE `-`.
    #[test]
    fn parse_error_names_file_line_column() {
        todo!()
    }

    /// Header DECISION: `render` on a file whose view keys `C1` and `c1`
    /// differ only in case is `CliError::KeyCollision` naming both keys,
    /// and the output directory is left absent (nothing written).
    #[test]
    fn render_case_colliding_keys_exits_2() {
        todo!()
    }
}

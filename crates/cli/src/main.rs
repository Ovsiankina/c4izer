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
// `--png` needs this crate's `png` feature (`png = ["c4rizer-core/png"]`),
// which turns on core's `export::png`; it renders with the bundled font
// (`scene::FONT_REGULAR`, `scene::FONT_BOLD`). Built without the feature,
// `--png` still parses and `run` returns `CliError::PngUnavailable` (exit
// 2 with a clear message, plan Review Focus).
// Every build passes the pipeline a stop flag that is never set: a CLI
// run is never stopped, and gives the same result as an unstopped run
// (CORE-25).

mod report;

use std::ffi::OsString;
use std::fmt;
use std::io::{self, Read, Write};
use std::num::NonZeroU32;
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::sync::atomic::AtomicBool;

use c4rizer_core::model::{self, ModelError};
use c4rizer_core::{export, pipeline};

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
        match self {
            UsageError::NoCommand => f.write_str("no subcommand — give `render` or `lint`"),
            UsageError::UnknownCommand(c) => {
                write!(f, "unknown subcommand \"{c}\" — use `render` or `lint`")
            }
            UsageError::UnknownFlag(flag) => write!(f, "unknown flag \"{flag}\""),
            UsageError::MissingFile => {
                f.write_str("no FILE given — name the diagram file, or `-` for stdin")
            }
            UsageError::MissingOutDir => {
                f.write_str("`render` needs `-o DIR` — name the output directory")
            }
            UsageError::ExtraArgument(a) => {
                write!(f, "unexpected argument \"{a}\" — give exactly one FILE")
            }
            UsageError::Repeated(flag) => write!(f, "`{flag}` given twice — give it once"),
            UsageError::MissingValue(flag) => write!(f, "`{flag}` needs a value after it"),
            UsageError::InvalidWidth(w) => write!(
                f,
                "invalid --width \"{w}\" — give a whole number of pixels from 1 to {}",
                u32::MAX
            ),
            UsageError::WidthWithoutPng => f.write_str("`--width` needs `--png`"),
        }
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
    /// `--png` on a build without the `png` feature: says to rebuild
    /// with `--features png` (plan Review Focus).
    PngUnavailable,
    /// `export::png` refused a view (CLI-4, EXP-2).
    // DECISION: the `PngError` is kept as its `Display` text, so this type
    // compiles whether or not core's `png` feature is on.
    // DECISION: only `png_bytes` with the `png` feature builds it, so it
    // is dead code in a build without the feature.
    #[cfg_attr(
        not(feature = "png"),
        expect(dead_code, reason = "built only with the png feature")
    )]
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
        match self {
            CliError::Read { input, source } => write!(
                f,
                "{}: cannot read ({source}) — check the path and that the file is UTF-8 text",
                InputName(input)
            ),
            CliError::Parse { input, error } => write!(f, "{}:{error}", InputName(input)),
            CliError::KeyCollision { first, second } => write!(
                f,
                "view keys \"{first}\" and \"{second}\" differ only in case, so their files would overwrite each other — rename one of the views"
            ),
            CliError::CreateDir { path, source } => write!(
                f,
                "{}: cannot create the output directory ({source}) — choose a writable -o directory",
                path.display()
            ),
            CliError::Write { path, source } => write!(
                f,
                "{}: cannot write ({source}) — choose a writable -o directory",
                path.display()
            ),
            CliError::Report(source) => write!(
                f,
                "cannot write the report to stdout ({source}) — keep stdout open until c4rizer exits"
            ),
            CliError::PngUnavailable => f.write_str(
                "--png needs PNG support, which this build lacks — rebuild with `cargo build -p c4rizer-cli --features png`",
            ),
            CliError::Png { view, message } => write!(
                f,
                "view \"{view}\": PNG export failed ({message}) — try a smaller --width"
            ),
        }
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
    let mut args = args.into_iter();
    let first = args.next().ok_or(UsageError::NoCommand)?;
    let render = match first.to_str() {
        Some("render") => true,
        Some("lint") => false,
        _ => return Err(UsageError::UnknownCommand(lossy(&first))),
    };
    let mut file: Option<OsString> = None;
    let mut out_dir: Option<PathBuf> = None;
    let mut png = false;
    let mut width: Option<NonZeroU32> = None;
    while let Some(arg) = args.next() {
        match arg.to_str() {
            Some("-o") if render => {
                if out_dir.is_some() {
                    return Err(UsageError::Repeated("-o"));
                }
                let value = args.next().ok_or(UsageError::MissingValue("-o"))?;
                out_dir = Some(PathBuf::from(value));
            }
            Some("--png") if render => {
                if png {
                    return Err(UsageError::Repeated("--png"));
                }
                png = true;
            }
            Some("--width") if render => {
                if width.is_some() {
                    return Err(UsageError::Repeated("--width"));
                }
                let value = args.next().ok_or(UsageError::MissingValue("--width"))?;
                let parsed = value
                    .to_str()
                    .and_then(|v| v.parse::<NonZeroU32>().ok())
                    .ok_or_else(|| UsageError::InvalidWidth(lossy(&value)))?;
                width = Some(parsed);
            }
            _ if is_flag(&arg) => return Err(UsageError::UnknownFlag(lossy(&arg))),
            _ if file.is_some() => return Err(UsageError::ExtraArgument(lossy(&arg))),
            _ => file = Some(arg),
        }
    }
    // DECISION: a missing FILE is reported before a missing `-o` and
    // before `--width` without `--png`: the first thing to fix.
    let file = file.ok_or(UsageError::MissingFile)?;
    let input = if file == "-" {
        Input::Stdin
    } else {
        Input::Path(PathBuf::from(file))
    };
    if !render {
        return Ok(Command::Lint { input });
    }
    let out_dir = out_dir.ok_or(UsageError::MissingOutDir)?;
    if width.is_some() && !png {
        return Err(UsageError::WidthWithoutPng);
    }
    Ok(Command::Render {
        input,
        out_dir,
        png: png.then_some(Png { width }),
    })
}

/// An argument that starts with `-` and is not FILE `-` itself.
// DECISION: checked on the raw bytes, so a non-UTF-8 argument starting
// with `-` is an unknown flag too, never a FILE.
fn is_flag(arg: &OsString) -> bool {
    let bytes = arg.as_encoded_bytes();
    bytes.len() > 1 && bytes[0] == b'-'
}

/// An argument as text for a `UsageError`.
fn lossy(arg: &OsString) -> String {
    arg.to_string_lossy().into_owned()
}

/// `Display` of an `Input` in messages: the path as given, or `<stdin>`.
struct InputName<'a>(&'a Input);

impl fmt::Display for InputName<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.0 {
            Input::Path(path) => write!(f, "{}", path.display()),
            Input::Stdin => f.write_str("<stdin>"),
        }
    }
}

/// Reads the whole input as UTF-8 text.
fn read_input(input: &Input, stdin: &mut impl Read) -> Result<String, CliError> {
    let bytes = match input {
        Input::Path(path) => std::fs::read(path),
        Input::Stdin => {
            let mut bytes = Vec::new();
            stdin.read_to_end(&mut bytes).map(|_| bytes)
        }
    };
    bytes
        .and_then(|b| {
            String::from_utf8(b).map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))
        })
        .map_err(|source| CliError::Read {
            input: input.clone(),
            source,
        })
}

/// The first pair of view keys equal up to ASCII case, in declaration
/// order (header DECISION).
fn key_collision(keys: &[&str]) -> Option<CliError> {
    keys.iter().enumerate().find_map(|(i, first)| {
        keys[i + 1..]
            .iter()
            .find(|second| first.eq_ignore_ascii_case(second))
            .map(|second| CliError::KeyCollision {
                first: (*first).to_owned(),
                second: (*second).to_owned(),
            })
    })
}

/// Writes `bytes` to `path`, then its `wrote` line.
fn write_file(path: &Path, bytes: &[u8], stdout: &mut impl Write) -> Result<(), CliError> {
    std::fs::write(path, bytes).map_err(|source| CliError::Write {
        path: path.to_path_buf(),
        source,
    })?;
    report::wrote(stdout, path).map_err(CliError::Report)
}

/// The PNG of one view at the requested width, or twice the SVG width.
#[cfg(feature = "png")]
fn png_bytes(view: &pipeline::ViewBuild, png: Png) -> Result<Vec<u8>, CliError> {
    let width = png.width.map_or_else(
        || {
            u32::try_from(view.scene.bounds.width())
                .unwrap_or(0)
                .saturating_mul(2)
        },
        NonZeroU32::get,
    );
    c4rizer_core::export::png(&view.scene, width).map_err(|e| CliError::Png {
        view: view.plan.key.clone(),
        message: e.to_string(),
    })
}

/// Without the `png` feature `run` refuses `--png` before this is reached.
#[cfg(not(feature = "png"))]
fn png_bytes(_view: &pipeline::ViewBuild, _png: Png) -> Result<Vec<u8>, CliError> {
    Err(CliError::PngUnavailable)
}

/// Runs `command`: reads the input (the whole file, or `stdin` read to
/// the end for `Input::Stdin`; never written to), parses it
/// (`model::parse`), builds every view (`pipeline::build_all`, with a
/// stop flag that is never set, so it never returns `Stopped`), prints the
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
    let (input, render) = match command {
        Command::Render {
            input,
            out_dir,
            png,
        } => (input, Some((out_dir, *png))),
        Command::Lint { input } => (input, None),
    };
    if !cfg!(feature = "png") && matches!(render, Some((_, Some(_)))) {
        return Err(CliError::PngUnavailable);
    }
    let text = read_input(input, stdin)?;
    let parsed = model::parse(&text).map_err(|error| CliError::Parse {
        input: input.clone(),
        error,
    })?;
    // A flag that is never set: `build_all` cannot return `Stopped`.
    let Ok(all) = pipeline::build_all(&parsed.model, &AtomicBool::new(false)) else {
        unreachable!("the stop flag is never set");
    };
    if let Some((out_dir, _)) = render {
        let keys: Vec<&str> = all.views.iter().map(|v| v.plan.key.as_str()).collect();
        if let Some(collision) = key_collision(&keys) {
            return Err(collision);
        }
        // DECISION: `render` creates DIR even for a file with no views, so
        // a script can rely on DIR existing after exit 0 or 1.
        std::fs::create_dir_all(out_dir).map_err(|source| CliError::CreateDir {
            path: out_dir.clone(),
            source,
        })?;
    }
    if all.views.is_empty() {
        report::no_views(stdout).map_err(CliError::Report)?;
    }
    for view in &all.views {
        report::view(stdout, &view.report, view.plan.title.as_deref()).map_err(CliError::Report)?;
        if let Some((out_dir, png)) = render {
            let key = &view.plan.key;
            let svg = export::svg(&view.scene);
            write_file(&out_dir.join(format!("{key}.svg")), svg.as_bytes(), stdout)?;
            if let Some(png) = png {
                let bytes = png_bytes(view, png)?;
                write_file(&out_dir.join(format!("{key}.png")), &bytes, stdout)?;
            }
        }
    }
    report::coverage(stdout, &all.coverage).map_err(CliError::Report)?;
    stdout.flush().map_err(CliError::Report)?;
    Ok(if all.has_errors() {
        Exit::LintErrors
    } else {
        Exit::Clean
    })
}

/// Parses `std::env::args_os()`; on a `UsageError` prints
/// `c4rizer: <error>` and `USAGE` to stderr and returns exit 2. Otherwise
/// runs the command on the real stdin and stdout and returns 0 for
/// `Exit::Clean`, 1 for `Exit::LintErrors`; on a `CliError` prints
/// `c4rizer: <error>` to stderr and returns exit 2. Serves CLI-2.
fn main() -> ExitCode {
    let command = match parse_args(std::env::args_os().skip(1)) {
        Ok(command) => command,
        Err(error) => {
            eprintln!("c4rizer: {error}\n{USAGE}");
            return ExitCode::from(2);
        }
    };
    match run(&command, &mut io::stdin().lock(), &mut io::stdout().lock()) {
        Ok(exit) => exit_code(exit),
        Err(error) => {
            eprintln!("c4rizer: {error}");
            ExitCode::from(2)
        }
    }
}

/// The process exit code of a finished run (CLI-2).
fn exit_code(exit: Exit) -> ExitCode {
    match exit {
        Exit::Clean => ExitCode::SUCCESS,
        Exit::LintErrors => ExitCode::from(1),
    }
}

#[cfg(test)]
mod tests {
    use std::ffi::OsString;
    use std::io::{self, Cursor};
    use std::num::NonZeroU32;
    use std::path::{Path, PathBuf};
    use std::sync::atomic::{AtomicU32, Ordering};

    use super::{CliError, Command, Exit, Input, Png, USAGE, UsageError, parse_args, run};

    /// Two boxes on the grid, far apart, one labelled line: lints clean.
    pub(crate) const CLEAN: &str = r#"workspace {
  model {
    operator = person "Operator" "Places orders."
    shop = softwareSystem "Shop" "Sells goods."
    operator -> shop "Places orders in"
  }
  views {
    systemContext shop "ctx" "Shop context" {
      include *
      at operator 200 150
      at shop 200 450
    }
  }
}
"#;

    /// `CLEAN` with a stop-word label: L13, an error.
    fn vague() -> String {
        CLEAN.replace("\"Places orders in\"", "\"uses\"")
    }

    /// A component view hiding one relationship of its container: L22.
    const UNCOVERED: &str = r#"workspace {
  model {
    shop = softwareSystem "Shop" "Sells goods." {
      front = container "Front" "Shows pages." "Rust" {
        pages = component "Pages" "Renders pages." "Rust"
        cart = component "Cart" "Holds the cart." "Rust"
      }
    }
    pages -> cart "Fills"
  }
  views {
    component front "front" "Front components" {
      include *
      hide pages -> cart
    }
  }
}
"#;

    /// An unknown construct on line 2.
    const BROKEN: &str = "workspace {\n  bogus\n}\n";

    /// A fresh, empty directory under the system temp dir, removed on drop.
    // DECISION: a hand-made temp dir (pid, counter, test name), since the
    // crate takes no dev-dependency.
    pub(crate) struct Scratch(PathBuf);

    impl Scratch {
        pub(crate) fn new(name: &str) -> Scratch {
            static NEXT: AtomicU32 = AtomicU32::new(0);
            let n = NEXT.fetch_add(1, Ordering::Relaxed);
            let dir = std::env::temp_dir().join(format!(
                "c4rizer-cli-unit-{}-{n}-{name}",
                std::process::id()
            ));
            let _ = std::fs::remove_dir_all(&dir);
            std::fs::create_dir_all(&dir).expect("temp dir");
            Scratch(dir)
        }

        pub(crate) fn file(&self, name: &str, text: &str) -> PathBuf {
            let path = self.0.join(name);
            std::fs::write(&path, text).expect("fixture written");
            path
        }

        pub(crate) fn path(&self) -> &Path {
            &self.0
        }
    }

    impl Drop for Scratch {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn args(list: &[&str]) -> Vec<OsString> {
        list.iter().map(OsString::from).collect()
    }

    fn parse(list: &[&str]) -> Result<Command, UsageError> {
        parse_args(args(list))
    }

    fn lint(path: &Path) -> Command {
        Command::Lint {
            input: Input::Path(path.to_path_buf()),
        }
    }

    fn render(path: &Path, out: &Path) -> Command {
        Command::Render {
            input: Input::Path(path.to_path_buf()),
            out_dir: out.to_path_buf(),
            png: None,
        }
    }

    /// Runs `command` with an empty stdin, returning the result and stdout.
    fn go(command: &Command) -> (Result<Exit, CliError>, String) {
        let mut out = Vec::new();
        let result = run(command, &mut io::empty(), &mut out);
        (result, String::from_utf8(out).expect("UTF-8 report"))
    }

    /// CLI-2: `run` gives `Exit::Clean` on a file whose views lint clean,
    /// `Exit::LintErrors` when one view has an error or L22 fires, and a
    /// `CliError` on a parse error and on a missing file. The mapping to
    /// process codes 0/1/2 is checked end to end in
    /// `tests/cli.rs::cli_2_binary_exit_codes`.
    #[test]
    fn cli_2_exit_codes() {
        let dir = Scratch::new("exit");
        let (clean, report) = go(&lint(&dir.file("clean.dsl", CLEAN)));
        assert_eq!(clean.unwrap(), Exit::Clean, "{report}");

        let (bad, report) = go(&lint(&dir.file("vague.dsl", &vague())));
        assert_eq!(bad.unwrap(), Exit::LintErrors, "{report}");
        assert!(report.contains("   ERROR: "), "{report}");

        let (uncovered, report) = go(&lint(&dir.file("uncovered.dsl", UNCOVERED)));
        assert_eq!(uncovered.unwrap(), Exit::LintErrors, "{report}");
        assert!(report.lines().any(|l| l.starts_with("ERROR: ")), "{report}");

        let (broken, _) = go(&lint(&dir.file("broken.dsl", BROKEN)));
        assert!(matches!(broken, Err(CliError::Parse { .. })), "{broken:?}");

        let (missing, _) = go(&lint(&dir.path().join("absent.dsl")));
        assert!(matches!(missing, Err(CliError::Read { .. })), "{missing:?}");

        let (empty, report) = go(&lint(&dir.file("empty.dsl", "workspace {\n}\n")));
        assert_eq!(empty.unwrap(), Exit::Clean);
        assert_eq!(report, "no views\n");
    }

    /// Plan cli-01 / Review Focus: an unknown flag (`--pdf`), an unknown
    /// subcommand and no subcommand are `UsageError`s, which `main` turns
    /// into the usage text on stderr and exit 2.
    #[test]
    fn unknown_flag_prints_usage() {
        let flag = parse(&["render", "a.dsl", "-o", "out", "--pdf"]).unwrap_err();
        assert_eq!(flag, UsageError::UnknownFlag("--pdf".into()));
        assert!(flag.to_string().contains("--pdf"), "{flag}");
        let command = parse(&["draw", "a.dsl"]).unwrap_err();
        assert_eq!(command, UsageError::UnknownCommand("draw".into()));
        assert!(command.to_string().contains("draw"), "{command}");
        assert_eq!(parse(&[]).unwrap_err(), UsageError::NoCommand);
        assert_eq!(
            parse(&["lint", "--help"]).unwrap_err(),
            UsageError::UnknownFlag("--help".into())
        );
        for e in [flag, command, UsageError::NoCommand] {
            assert!(!e.to_string().contains('\n'), "one line: {e}");
        }
        assert!(USAGE.contains("c4rizer render FILE -o DIR"));
        assert!(USAGE.contains("c4rizer lint FILE"));
    }

    /// Review Focus: `render FILE -o DIR --png --width 800` parses with
    /// flags in any order around FILE; `render FILE` without `-o`
    /// (`MissingOutDir`), `--width` without `--png`, a repeated flag,
    /// `--width 0`, and `-o` with no value are usage errors; `lint`
    /// rejects `-o` and `--png`.
    #[test]
    fn cli_4_png_and_width_flags_parse() {
        let want = Command::Render {
            input: Input::Path("a.dsl".into()),
            out_dir: "out".into(),
            png: Some(Png {
                width: NonZeroU32::new(800),
            }),
        };
        for order in [
            &["render", "a.dsl", "-o", "out", "--png", "--width", "800"][..],
            &["render", "--width", "800", "-o", "out", "--png", "a.dsl"][..],
            &["render", "--png", "a.dsl", "--width", "800", "-o", "out"][..],
        ] {
            assert_eq!(parse(order).unwrap(), want, "{order:?}");
        }
        assert_eq!(
            parse(&["render", "a.dsl", "-o", "out", "--png"]).unwrap(),
            Command::Render {
                input: Input::Path("a.dsl".into()),
                out_dir: "out".into(),
                png: Some(Png { width: None }),
            }
        );
        assert_eq!(
            parse(&["render", "a.dsl", "-o", "out"]).unwrap(),
            render(Path::new("a.dsl"), Path::new("out"))
        );
        assert_eq!(parse(&["lint", "a.dsl"]).unwrap(), lint(Path::new("a.dsl")));

        let err = |list: &[&str]| parse(list).unwrap_err();
        assert_eq!(err(&["render", "a.dsl"]), UsageError::MissingOutDir);
        assert_eq!(err(&["render", "-o", "out"]), UsageError::MissingFile);
        assert_eq!(err(&["lint"]), UsageError::MissingFile);
        assert_eq!(
            err(&["render", "a.dsl", "-o", "out", "--width", "800"]),
            UsageError::WidthWithoutPng
        );
        assert_eq!(
            err(&["render", "a.dsl", "-o", "out", "-o", "two"]),
            UsageError::Repeated("-o")
        );
        assert_eq!(
            err(&["render", "a.dsl", "-o", "out", "--png", "--png"]),
            UsageError::Repeated("--png")
        );
        assert_eq!(
            err(&[
                "render", "a.dsl", "-o", "o", "--png", "--width", "1", "--width", "2"
            ]),
            UsageError::Repeated("--width")
        );
        assert_eq!(
            err(&["render", "a.dsl", "-o", "out", "--png", "--width", "0"]),
            UsageError::InvalidWidth("0".into())
        );
        assert_eq!(
            err(&["render", "a.dsl", "-o", "out", "--png", "--width", "wide"]),
            UsageError::InvalidWidth("wide".into())
        );
        assert_eq!(
            err(&["render", "a.dsl", "-o"]),
            UsageError::MissingValue("-o")
        );
        assert_eq!(
            err(&["render", "a.dsl", "-o", "out", "--png", "--width"]),
            UsageError::MissingValue("--width")
        );
        assert_eq!(
            err(&["lint", "a.dsl", "-o", "out"]),
            UsageError::UnknownFlag("-o".into())
        );
        assert_eq!(
            err(&["lint", "a.dsl", "--png"]),
            UsageError::UnknownFlag("--png".into())
        );
        assert_eq!(
            err(&["lint", "a.dsl", "b.dsl"]),
            UsageError::ExtraArgument("b.dsl".into())
        );
    }

    /// Review Focus: FILE `-` parses to `Input::Stdin`, and `run` reads
    /// the model from the given stdin.
    #[test]
    fn stdin_dash_reads_model() {
        assert_eq!(
            parse(&["lint", "-"]).unwrap(),
            Command::Lint {
                input: Input::Stdin
            }
        );
        let mut stdin = Cursor::new(CLEAN.as_bytes().to_vec());
        let mut out = Vec::new();
        let exit = run(
            &Command::Lint {
                input: Input::Stdin,
            },
            &mut stdin,
            &mut out,
        )
        .unwrap();
        assert_eq!(exit, Exit::Clean);
        let report = String::from_utf8(out).unwrap();
        assert!(report.starts_with("== ctx: Shop context\n"), "{report}");
    }

    /// Review Focus: a missing output directory is created (with
    /// parents) and the SVGs land in it.
    #[test]
    fn output_dir_created() {
        let dir = Scratch::new("created");
        let file = dir.file("clean.dsl", CLEAN);
        let out = dir.path().join("a").join("b");
        let (result, report) = go(&render(&file, &out));
        assert_eq!(result.unwrap(), Exit::Clean);
        let svg = std::fs::read_to_string(out.join("ctx.svg")).expect("SVG written");
        assert!(svg.starts_with("<svg"), "{svg}");
        assert!(
            report.ends_with(&format!("   wrote {}\n", out.join("ctx.svg").display())),
            "{report}"
        );
    }

    /// Review Focus: an output path that cannot be written (a regular
    /// file given as `-o`) is `CliError::CreateDir` or `Write` naming the
    /// path, exit 2.
    #[test]
    fn output_dir_not_writable_exits_2() {
        let dir = Scratch::new("unwritable");
        let file = dir.file("clean.dsl", CLEAN);
        let blocker = dir.file("taken", "a regular file");
        let (result, _) = go(&render(&file, &blocker));
        let error = result.unwrap_err();
        match &error {
            CliError::CreateDir { path, .. } => assert_eq!(path, &blocker),
            CliError::Write { path, .. } => assert!(path.starts_with(&blocker)),
            other => panic!("expected CreateDir or Write, got {other:?}"),
        }
        assert!(
            error.to_string().contains(&blocker.display().to_string()),
            "{error}"
        );
    }

    /// Review Focus: `--png` on a build without PNG support is
    /// `CliError::PngUnavailable` (exit 2) before any file is read or
    /// written. Runs on builds without the `png` feature only.
    #[cfg(not(feature = "png"))]
    #[test]
    fn png_without_feature_exits_2() {
        let dir = Scratch::new("nopng");
        let out = dir.path().join("out");
        let command = Command::Render {
            input: Input::Path(dir.path().join("absent.dsl")),
            out_dir: out.clone(),
            png: Some(Png { width: None }),
        };
        let (result, report) = go(&command);
        let error = result.unwrap_err();
        assert!(matches!(error, CliError::PngUnavailable), "{error:?}");
        assert!(error.to_string().contains("--features png"), "{error}");
        assert!(report.is_empty());
        assert!(!out.exists());
    }

    /// SRS 5.1: a parse error's `Display` is
    /// `<path>:<line>:<column>: <what> — <what to change>`, and
    /// `<stdin>:...` for FILE `-`.
    #[test]
    fn parse_error_names_file_line_column() {
        let dir = Scratch::new("parse");
        let file = dir.file("broken.dsl", BROKEN);
        let (result, _) = go(&lint(&file));
        let text = result.unwrap_err().to_string();
        assert!(
            text.starts_with(&format!("{}:2:3: ", file.display())),
            "{text}"
        );
        assert!(text.contains(" — "), "{text}");
        assert!(!text.contains('\n'), "{text}");

        let mut stdin = Cursor::new(BROKEN.as_bytes().to_vec());
        let text = run(
            &Command::Lint {
                input: Input::Stdin,
            },
            &mut stdin,
            &mut Vec::new(),
        )
        .unwrap_err()
        .to_string();
        assert!(text.starts_with("<stdin>:2:3: "), "{text}");
    }

    /// Header DECISION: `render` on a file whose view keys `C1` and `c1`
    /// differ only in case is `CliError::KeyCollision` naming both keys,
    /// and the output directory is left absent (nothing written).
    #[test]
    fn render_case_colliding_keys_exits_2() {
        let dir = Scratch::new("collide");
        let two = CLEAN.replace(
            "    systemContext shop \"ctx\"",
            "    systemContext shop \"C1\" \"Shop context\" {\n      include *\n    }\n    systemContext shop \"c1\"",
        );
        let file = dir.file("collide.dsl", &two);
        let out = dir.path().join("out");
        let (result, report) = go(&render(&file, &out));
        let error = result.unwrap_err();
        match &error {
            CliError::KeyCollision { first, second } => {
                assert_eq!((first.as_str(), second.as_str()), ("C1", "c1"));
            }
            other => panic!("expected KeyCollision, got {other:?}"),
        }
        let text = error.to_string();
        assert!(text.contains("C1") && text.contains("c1"), "{text}");
        assert!(report.is_empty(), "{report}");
        assert!(!out.exists());
        // `lint` writes nothing and is unaffected.
        assert!(go(&lint(&file)).0.is_ok());
    }
}

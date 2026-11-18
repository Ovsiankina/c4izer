//! c4rizer CLI: `c4rizer render FILE -o DIR [--png [--width N]] [--mode light|dark]
//! [--svg-text name|embed|outline]` and `c4rizer lint FILE`, exit code 0
//! clean, 1 lint errors, 2 parse or I/O error (CLI-1..5); `validate`,
//! `inspect`, `list`, `version` and `help` as Structurizr's commands of
//! the same name (P2-22, P2-600..P2-650).
//!
//! Depends on: `report`, `c4rizer-core`.

// Shape of the bin (step-4 contract of a bin crate: internal signatures
// and layouts, `pub(crate)`):
//
//   main()                         -> ExitCode
//     parse_args(args)             -> Command | UsageError   (exit 2 + USAGE)
//     run(&command, stdin, stdout) -> Exit | CliError        (exit 2)
//       read FILE or stdin         -> String | CliError
//       model::parse_with(text, path, FsLoader)
//                                  -> Parsed | ModelError    (exit 2)
//       pipeline::build_all(model) -> AllViews
//       report::notice per model notice, before any view (P2-125)
//       per view, declaration order: an unsupported view prints
//                 report::unsupported (P2-122) and writes nothing;
//                 else report::view, then (render) export::svg [+ export::png]
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
// DECISION: `help`, `-h` and `--help` are only recognised in place of the
// subcommand (B cli-breadth 9); after a subcommand `--help` stays an
// unknown flag, so `lint --help` is a usage error as in phase 1.
// DECISION: parse and I/O errors go to stderr as one line
// `c4rizer: <path>:<line>:<column>: <what> — <what to change>` (SRS 5.1;
// `ModelError`'s `Display` gives `line:column: ...` and asks the caller to
// prefix the path of `span.file`: FILE, or the included file the error
// is in, P2-136); stdin is named `<stdin>`. The report goes to stdout.
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

mod export_files;
mod report;

use std::ffi::OsString;
use std::fmt;
use std::io::{self, Read, Write};
use std::num::NonZeroU32;
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::sync::atomic::AtomicBool;

use c4rizer_core::import::{self, ImportError};
use c4rizer_core::model::{self, Mode, ModelError, ViewKind};
use c4rizer_core::text::FileId;
use c4rizer_core::{edit, export, inspect, pipeline};

/// The usage text printed to stderr on every usage error (exit 2) and to
/// stdout by `help`. Names all seven subcommands and every argument
/// (CLI-1, CLI-3, CLI-4, P2-648).
pub(crate) const USAGE: &str = "\
usage: c4rizer render FILE -o DIR [--png [--width N]] [--mode light|dark] [--svg-text name|embed|outline]
       c4rizer lint FILE
       c4rizer validate -workspace FILE
       c4rizer inspect -workspace FILE [-severity error,warning,info,ignore]
       c4rizer list -workspace FILE
       c4rizer import-layout JSON FILE
       c4rizer export -f FORMAT [-o DIR] [-workspace] FILE [--positions]
       c4rizer merge -w FILE -l LAYOUT.json [-v KEY] -o OUT
       c4rizer generate system-landscape -i DIR -o OUT.dsl
       c4rizer version
       c4rizer help [COMMAND]
FILE `-` reads the diagram from stdin; `-workspace` may be left out.
export FORMAT: plantuml, plantuml/structurizr, plantuml/c4plantuml, mermaid,
  websequencediagrams, dot, ilograph, json, theme, static, svg, png
exit code: 0 clean, 1 lint errors, 2 parse, I/O or usage error;
merge, generate: 1 on a parse error of any input;
inspect: the number of error violations shown (at most 254), 255 on an error";

/// Each subcommand with its usage line, in `USAGE` order (P2-647,
/// P2-648): `help <command>` prints that line alone.
const COMMAND_LINES: [(&str, &str); 11] = [
    (
        "render",
        "c4rizer render FILE -o DIR [--png [--width N]] [--mode light|dark] [--svg-text name|embed|outline]",
    ),
    ("lint", "c4rizer lint FILE"),
    ("validate", "c4rizer validate -workspace FILE"),
    (
        "inspect",
        "c4rizer inspect -workspace FILE [-severity error,warning,info,ignore]",
    ),
    ("list", "c4rizer list -workspace FILE"),
    ("import-layout", "c4rizer import-layout JSON FILE"),
    (
        "export",
        "c4rizer export -f FORMAT [-o DIR] [-workspace] FILE [--positions]",
    ),
    ("merge", "c4rizer merge -w FILE -l LAYOUT.json [-v KEY] -o OUT"),
    (
        "generate",
        "c4rizer generate system-landscape -i DIR -o OUT.dsl",
    ),
    ("version", "c4rizer version"),
    ("help", "c4rizer help [COMMAND]"),
];

/// The seven subcommands as the no-subcommand and unknown-subcommand
/// messages name them (P2-650).
const SUBCOMMANDS: &str =
    "`render`, `lint`, `validate`, `inspect`, `list`, `import-layout`, `export`, `merge`, `generate`, `version`, `help`";

/// The formats of `export -f`, as the usage errors list them (P3-401).
const FORMATS: &str = "plantuml, plantuml/structurizr, plantuml/c4plantuml, mermaid, websequencediagrams, dot, ilograph, json, theme, static, svg, png";

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
    /// `inspect` showed this many error-severity violations, at most 254
    /// (P2-642, B cli-breadth 5): exit with that number.
    Violations(u8),
}

/// How `render` writes text into its SVG files (P2-651).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub(crate) enum SvgText {
    /// `name` (the default): `export::svg`, the font named (EXP-1).
    #[default]
    Name,
    /// `embed`: `export::svg_embedded`, the bundled faces embedded.
    Embed,
    /// `outline`: `export::svg_outlined`, text as paths (feature
    /// `outline`).
    Outline,
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

/// A parsed command line. Serves CLI-1, CLI-3, CLI-4, P2-22.
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
        /// `--mode light|dark`; light when not given (P2-432).
        mode: Mode,
        /// `--svg-text name|embed|outline`; `name` when not given (P2-651).
        svg_text: SvgText,
    },
    /// `c4rizer lint FILE`: the same checks and report, no file written
    /// (CLI-3).
    Lint {
        /// FILE.
        input: Input,
    },
    /// `c4rizer validate -workspace FILE`: parse only (P2-604..P2-607).
    Validate {
        /// FILE.
        input: Input,
    },
    /// `c4rizer inspect -workspace FILE [-severity LIST]` (P2-611..P2-644).
    Inspect {
        /// FILE.
        input: Input,
        /// The severities shown and counted, sorted, no repeats; all four
        /// when `-severity` is not given (P2-640).
        severities: Vec<inspect::Severity>,
        /// `-inspector <class>`, refused by `run` (P2-643).
        inspector: Option<String>,
    },
    /// `c4rizer list -workspace FILE`: every element (P2-608..P2-610).
    List {
        /// FILE.
        input: Input,
    },
    /// `c4rizer import-layout JSON FILE`: `at` lines from a Structurizr
    /// workspace JSON into FILE, in place (P2-15, P2-504).
    ImportLayout {
        /// JSON, the Structurizr workspace.
        json: PathBuf,
        /// FILE, rewritten in place.
        file: PathBuf,
    },
    /// `c4rizer export -f <format> [-o DIR] [-w] FILE [--positions]`:
    /// the format's files into DIR (P3-400..P3-405).
    Export {
        /// FILE (`-w`/`-workspace` may stand before it).
        input: Input,
        /// `-f`/`--format`.
        format: ExportFormat,
        /// `-o`/`--output`; FILE's folder when not given (P3-400).
        out_dir: Option<PathBuf>,
        /// `--positions`: DOT nodes carry `pos` (P3-411).
        positions: bool,
    },
    /// `c4rizer merge -w FILE -l LAYOUT.json [-v KEY] -o OUT`: a copy of
    /// FILE with the `at` lines `import-layout` would add from LAYOUT
    /// (P3-425..P3-427).
    Merge {
        /// FILE, read only (P3-426).
        workspace: PathBuf,
        /// LAYOUT.json, a Structurizr workspace JSON.
        layout: PathBuf,
        /// `-v KEY`: only that view changes.
        view: Option<String>,
        /// OUT.
        out: PathBuf,
    },
    /// `c4rizer generate system-landscape -i DIR -o OUT.dsl`: one
    /// landscape from every workspace in DIR (P3-428, P3-429).
    Generate {
        /// DIR.
        input_dir: PathBuf,
        /// OUT.dsl.
        out: PathBuf,
    },
    /// `c4rizer version` (P2-645).
    Version,
    /// `c4rizer help [COMMAND]`, `-h`, `--help` (P2-647).
    Help {
        /// The subcommand asked about, `None` for all of them.
        command: Option<&'static str>,
    },
}

/// A format of `export -f` (P3-400), as Structurizr spells it plus
/// c4rizer's `dot`, `ilograph`, `svg` and `png`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) enum ExportFormat {
    /// `plantuml`, `plantuml/structurizr`, `plantuml/c4plantuml`.
    PlantUml(export::Flavour),
    /// `mermaid`.
    Mermaid,
    /// `websequencediagrams`.
    WebSequenceDiagrams,
    /// `dot`.
    Dot,
    /// `ilograph`.
    Ilograph,
    /// `json`.
    Json,
    /// `theme`.
    Theme,
    /// `static` (part 2 of the row, P3-423).
    Static,
    /// `svg`, as `render` writes it (P3-402).
    Svg,
    /// `png`, as `render --png` writes it (P3-402).
    Png,
}

/// A command line that is not one of the `USAGE` forms. `Display` is one
/// line saying what is wrong; `main` prints it, then `USAGE`, to stderr
/// and exits 2 (plan Review Focus). Serves CLI-2.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub(crate) enum UsageError {
    /// No subcommand at all.
    NoCommand,
    /// A first argument that is not one of the seven subcommands (or
    /// `-h`, `--help`), as given (lossy UTF-8).
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
    /// `--mode` with a value other than `light` or `dark`, as given
    /// (P2-432).
    InvalidMode(String),
    /// `validate`, `inspect` or `list` without a workspace (P2-602).
    MissingWorkspace,
    /// `-workspace` naming a URL, as given (P2-603, B cli-breadth 3).
    WorkspaceUrl(String),
    /// `-severity` with a value outside `error`, `warning`, `info`,
    /// `ignore`, as given (P2-641).
    InvalidSeverity(String),
    /// `--svg-text` with a value other than `name`, `embed` or `outline`,
    /// as given (P2-653).
    UnknownSvgText(String),
    /// `import-layout` without both JSON and FILE (P2-504).
    MissingImportFiles,
    /// `export` without `-f` (P3-401).
    MissingFormat,
    /// `export -f` with a format c4rizer does not write, `fqcn` or a Java
    /// class name included, as given (P3-401).
    UnknownFormat(String),
    /// `merge` or `generate` without a required option, named as the
    /// usage line spells it (P3-430).
    MissingOption(&'static str),
    /// `generate` without a generator, or with one other than
    /// `system-landscape`, as given (empty when missing) (P3-430).
    UnknownGenerator(String),
}

impl fmt::Display for UsageError {
    /// One line, e.g. `unknown flag "--pdf"`. Serves CLI-2, SRS 5.1.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            UsageError::NoCommand => write!(f, "no subcommand — give one of {SUBCOMMANDS}"),
            UsageError::UnknownCommand(c) => {
                write!(f, "unknown subcommand \"{c}\" — use one of {SUBCOMMANDS}")
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
            UsageError::InvalidMode(m) => {
                write!(f, "invalid --mode \"{m}\" — give `light` or `dark`")
            }
            UsageError::MissingWorkspace => {
                f.write_str("no workspace given — add `-workspace FILE`, or `-` for stdin")
            }
            UsageError::WorkspaceUrl(url) => write!(
                f,
                "workspace URL \"{url}\" is not supported (no network) — download it and give the local path"
            ),
            UsageError::InvalidSeverity(v) => write!(
                f,
                "invalid -severity \"{v}\" — give a comma-separated list of error, warning, info and ignore"
            ),
            UsageError::UnknownSvgText(v) => write!(
                f,
                "invalid --svg-text \"{v}\" — give `name`, `embed` or `outline`"
            ),
            UsageError::MissingImportFiles => f.write_str(
                "`import-layout` needs the workspace JSON and the diagram file — give `import-layout JSON FILE`",
            ),
            UsageError::MissingFormat => write!(f, "`export` needs `-f FORMAT` — give one of {FORMATS}"),
            UsageError::UnknownFormat(format) => {
                write!(f, "unknown export format \"{format}\" — give one of {FORMATS}")
            }
            UsageError::MissingOption(option) => write!(f, "`{option}` is required — add it"),
            UsageError::UnknownGenerator(g) if g.is_empty() => {
                f.write_str("`generate` needs a generator — give `generate system-landscape`")
            }
            UsageError::UnknownGenerator(g) => {
                write!(f, "unknown generator \"{g}\" — give `system-landscape`")
            }
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
    /// `error.span.file`: FILE (`<stdin>` for `-`), or the included file
    /// the error is in (P2-136).
    Parse {
        /// The file holding the error as messages name it: FILE as given
        /// (`<stdin>`), or the included file's path.
        // DECISION: the name, not the `Input`, so the variant stays as small
        // as in phase 1 (clippy `result_large_err`).
        file: String,
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
    /// `inspect -inspector <class>`: custom Java inspectors need the JVM
    /// (parity table: never, P2-643).
    InspectorUnsupported(String),
    /// `--svg-text outline` on a build without the `outline` feature:
    /// says to rebuild with `--features outline` (P2-662).
    OutlineUnavailable,
    /// `export::svg_outlined` refused a view (P2-658).
    // DECISION: kept as text, so this type compiles with or without
    // core's `outline` feature, like `Png`.
    #[cfg_attr(
        not(feature = "outline"),
        expect(dead_code, reason = "built only with the outline feature")
    )]
    Outline {
        /// The view key.
        view: String,
        /// `OutlineError`'s `Display` text.
        message: String,
    },
    /// `import-layout`: the JSON is not RFC 8259 JSON or not a
    /// Structurizr workspace (P2-504).
    Json {
        /// The JSON file as given.
        path: PathBuf,
        /// What `import::import_layout` refused.
        error: ImportError,
    },
    /// `import-layout`: `edit::apply` refused the batch (P2-504); FILE is
    /// untouched.
    Edit {
        /// FILE as given.
        path: PathBuf,
        /// `EditError`'s `Display` text.
        message: String,
    },
    /// `import-layout`: FILE could not be rewritten; it is untouched
    /// (EDIT-22 atomic write).
    Rewrite {
        /// The file.
        path: PathBuf,
        /// Why.
        source: io::Error,
    },
    /// `merge`: a request refused before anything is written (OUT names
    /// FILE, FILE is JSON, `-v` names no view of FILE).
    Refused {
        /// The file the refusal is about.
        path: PathBuf,
        /// What is wrong and what to change.
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
            CliError::Parse { file, error } => write!(f, "{file}:{error}"),
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
            CliError::InspectorUnsupported(class) => write!(
                f,
                "-inspector \"{class}\": custom inspectors are Java classes and need the JVM, which c4rizer does not run — leave out -inspector to run the built-in inspections"
            ),
            CliError::OutlineUnavailable => f.write_str(
                "--svg-text outline needs outline support, which this build lacks — rebuild with `cargo build -p c4rizer-cli --features outline`",
            ),
            CliError::Outline { view, message } => write!(
                f,
                "view \"{view}\": outlining the SVG failed ({message}) — use --svg-text name or embed"
            ),
            // P2-504, SRS 5.1: `path:line:column: <what> — <what to change>`.
            CliError::Json { path, error } => match error.position {
                Some(_) => write!(f, "{}:{error}", path.display()),
                None => write!(f, "{}: {error}", path.display()),
            },
            CliError::Edit { path, message } => write!(
                f,
                "{}: positions not written ({message}) — the file is unchanged; check that it parses and try again",
                path.display()
            ),
            CliError::Rewrite { path, source } => write!(
                f,
                "{}: cannot rewrite ({source}) — the file is unchanged; check that its folder is writable",
                path.display()
            ),
            CliError::Refused { path, message } => write!(f, "{}: {message}", path.display()),
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
        Some("version") => {
            return match args.next() {
                None => Ok(Command::Version),
                Some(extra) => Err(UsageError::ExtraArgument(lossy(&extra))),
            };
        }
        Some(flag @ ("help" | "-h" | "--help")) => return parse_help(flag, args),
        Some(name @ ("validate" | "inspect" | "list")) => {
            return parse_workspace_command(name, args);
        }
        Some("import-layout") => return parse_import_layout(args),
        Some("export") => return parse_export(args),
        Some(name @ ("merge" | "generate")) => return if name == "merge" { parse_merge(args) } else { parse_generate(args) },
        _ => return Err(UsageError::UnknownCommand(lossy(&first))),
    };
    let mut file: Option<OsString> = None;
    let mut out_dir: Option<PathBuf> = None;
    let mut png = false;
    let mut width: Option<NonZeroU32> = None;
    let mut mode: Option<Mode> = None;
    let mut svg_text: Option<SvgText> = None;
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
            // DECISION: `--mode` takes exactly `light` or `dark` in lower
            // case, as Structurizr's `-mode` values are written (P2-432).
            Some("--mode") if render => {
                if mode.is_some() {
                    return Err(UsageError::Repeated("--mode"));
                }
                let value = args.next().ok_or(UsageError::MissingValue("--mode"))?;
                mode = Some(match value.to_str() {
                    Some("light") => Mode::Light,
                    Some("dark") => Mode::Dark,
                    _ => return Err(UsageError::InvalidMode(lossy(&value))),
                });
            }
            // DECISION: `--svg-text` takes exactly `name`, `embed` or
            // `outline` in lower case, like `--mode` (B cli-breadth 10).
            Some("--svg-text") if render => {
                if svg_text.is_some() {
                    return Err(UsageError::Repeated("--svg-text"));
                }
                let value = args.next().ok_or(UsageError::MissingValue("--svg-text"))?;
                svg_text = Some(match value.to_str() {
                    Some("name") => SvgText::Name,
                    Some("embed") => SvgText::Embed,
                    Some("outline") => SvgText::Outline,
                    _ => return Err(UsageError::UnknownSvgText(lossy(&value))),
                });
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
        mode: mode.unwrap_or_default(),
        svg_text: svg_text.unwrap_or_default(),
    })
}

/// `validate`, `inspect` or `list` and their arguments (P2-600..P2-603).
// DECISION: the workspace is `-workspace FILE`, Structurizr's short `-w`
// and long `--workspace` spellings of the same option, or a bare FILE
// (B cli-breadth 2). A second FILE in any form is a usage error:
// `Repeated("-workspace")` when given with a workspace flag,
// `ExtraArgument` when bare.
// DECISION: a value starting with `http://` or `https://` (any case) is
// a URL, refused as a usage error before any read (B cli-breadth 3;
// Structurizr's own test of the value).
fn parse_workspace_command(
    name: &str,
    mut args: impl Iterator<Item = OsString>,
) -> Result<Command, UsageError> {
    let inspect = name == "inspect";
    let mut file: Option<OsString> = None;
    let mut severities: Option<Vec<inspect::Severity>> = None;
    let mut inspector: Option<String> = None;
    while let Some(arg) = args.next() {
        match arg.to_str() {
            Some("-workspace" | "-w" | "--workspace") => {
                let value = args.next().ok_or(UsageError::MissingValue("-workspace"))?;
                if file.is_some() {
                    return Err(UsageError::Repeated("-workspace"));
                }
                file = Some(value);
            }
            // Structurizr's `-s`/`-severity`/`--severity` and
            // `-i`/`-inspector`/`--inspector`, `inspect` only (P2-641).
            Some("-severity" | "-s" | "--severity") if inspect => {
                if severities.is_some() {
                    return Err(UsageError::Repeated("-severity"));
                }
                let value = args.next().ok_or(UsageError::MissingValue("-severity"))?;
                severities = Some(parse_severities(&value)?);
            }
            Some("-inspector" | "-i" | "--inspector") if inspect => {
                if inspector.is_some() {
                    return Err(UsageError::Repeated("-inspector"));
                }
                let value = args.next().ok_or(UsageError::MissingValue("-inspector"))?;
                inspector = Some(lossy(&value));
            }
            _ if is_flag(&arg) => return Err(UsageError::UnknownFlag(lossy(&arg))),
            _ if file.is_some() => return Err(UsageError::ExtraArgument(lossy(&arg))),
            _ => file = Some(arg),
        }
    }
    let file = file.ok_or(UsageError::MissingWorkspace)?;
    if is_url(&file) {
        return Err(UsageError::WorkspaceUrl(lossy(&file)));
    }
    let input = if file == "-" {
        Input::Stdin
    } else {
        Input::Path(PathBuf::from(file))
    };
    Ok(match name {
        "validate" => Command::Validate { input },
        "inspect" => Command::Inspect {
            input,
            severities: severities.unwrap_or_else(|| ALL_SEVERITIES.to_vec()),
            inspector,
        },
        _ => Command::List { input },
    })
}

/// Every inspection severity, lowest first: what `inspect` shows without
/// `-severity` (P2-640).
const ALL_SEVERITIES: [inspect::Severity; 4] = [
    inspect::Severity::Ignore,
    inspect::Severity::Info,
    inspect::Severity::Warning,
    inspect::Severity::Error,
];

/// `-severity LIST`: comma-separated severity keywords, any case, spaces
/// around each trimmed (Structurizr trims and upper-cases each), sorted
/// and without repeats (P2-640, P2-641). An empty item is invalid.
fn parse_severities(value: &OsString) -> Result<Vec<inspect::Severity>, UsageError> {
    let invalid = || UsageError::InvalidSeverity(lossy(value));
    let text = value.to_str().ok_or_else(invalid)?;
    let mut out = Vec::new();
    for item in text.split(',') {
        let item = item.trim();
        let severity = ALL_SEVERITIES
            .into_iter()
            .find(|s| s.keyword().eq_ignore_ascii_case(item))
            .ok_or_else(invalid)?;
        out.push(severity);
    }
    out.sort();
    out.dedup();
    Ok(out)
}

/// `export` arguments (P3-400, P3-401): `-f`/`--format` (also
/// Structurizr's `-format`), `-o`/`--output` (`-output`), an optional
/// `-w`/`-workspace` before FILE, `--positions`.
// DECISION: format names compare without regard to case, as Structurizr's
// do; a Java class name (any name with a `.`, `fqcn` included) is an
// unknown format, as c4rizer runs no JVM.
fn parse_export(mut args: impl Iterator<Item = OsString>) -> Result<Command, UsageError> {
    let mut file: Option<OsString> = None;
    let mut format: Option<ExportFormat> = None;
    let mut out_dir: Option<PathBuf> = None;
    let mut positions = false;
    while let Some(arg) = args.next() {
        match arg.to_str() {
            Some("-f" | "--format" | "-format") => {
                let value = args.next().ok_or(UsageError::MissingValue("-f"))?;
                if format.is_some() {
                    return Err(UsageError::Repeated("-f"));
                }
                format = Some(export_format(&lossy(&value))?);
            }
            Some("-o" | "--output" | "-output") => {
                let value = args.next().ok_or(UsageError::MissingValue("-o"))?;
                if out_dir.is_some() {
                    return Err(UsageError::Repeated("-o"));
                }
                out_dir = Some(PathBuf::from(value));
            }
            Some("-w" | "-workspace" | "--workspace") => {
                let value = args.next().ok_or(UsageError::MissingValue("-workspace"))?;
                if file.is_some() {
                    return Err(UsageError::Repeated("-workspace"));
                }
                file = Some(value);
            }
            Some("--positions") => {
                if positions {
                    return Err(UsageError::Repeated("--positions"));
                }
                positions = true;
            }
            _ if is_flag(&arg) => return Err(UsageError::UnknownFlag(lossy(&arg))),
            _ if file.is_some() => return Err(UsageError::ExtraArgument(lossy(&arg))),
            _ => file = Some(arg),
        }
    }
    let format = format.ok_or(UsageError::MissingFormat)?;
    let file = file.ok_or(UsageError::MissingFile)?;
    if is_url(&file) {
        return Err(UsageError::WorkspaceUrl(lossy(&file)));
    }
    let input = if file == "-" {
        Input::Stdin
    } else {
        Input::Path(PathBuf::from(file))
    };
    Ok(Command::Export {
        input,
        format,
        out_dir,
        positions,
    })
}

/// One `-f` value (P3-400, P3-401).
fn export_format(value: &str) -> Result<ExportFormat, UsageError> {
    let format = match value.to_ascii_lowercase().as_str() {
        "plantuml" | "plantuml/structurizr" => ExportFormat::PlantUml(export::Flavour::Structurizr),
        "plantuml/c4plantuml" => ExportFormat::PlantUml(export::Flavour::C4PlantUml),
        "mermaid" => ExportFormat::Mermaid,
        "websequencediagrams" => ExportFormat::WebSequenceDiagrams,
        "dot" => ExportFormat::Dot,
        "ilograph" => ExportFormat::Ilograph,
        "json" => ExportFormat::Json,
        "theme" => ExportFormat::Theme,
        "static" => ExportFormat::Static,
        "svg" => ExportFormat::Svg,
        "png" => ExportFormat::Png,
        _ => return Err(UsageError::UnknownFormat(value.to_owned())),
    };
    Ok(format)
}

/// `import-layout JSON FILE`: exactly two paths, no flags (P2-504).
fn parse_import_layout(args: impl Iterator<Item = OsString>) -> Result<Command, UsageError> {
    let mut paths = Vec::new();
    for arg in args {
        if is_flag(&arg) {
            return Err(UsageError::UnknownFlag(lossy(&arg)));
        }
        if paths.len() == 2 {
            return Err(UsageError::ExtraArgument(lossy(&arg)));
        }
        paths.push(PathBuf::from(arg));
    }
    let mut paths = paths.into_iter();
    match (paths.next(), paths.next()) {
        (Some(json), Some(file)) => Ok(Command::ImportLayout { json, file }),
        _ => Err(UsageError::MissingImportFiles),
    }
}

/// `merge` arguments (P3-425, P3-430): `-w`/`-workspace`/`--workspace`,
/// `-l`/`-layout`/`--layout`, `-v`/`-view`/`--view`,
/// `-o`/`-output`/`--output`, each once, in any order; all but `-v`
/// required.
// DECISION: Structurizr's `merge` spells its options `-workspace`,
// `-layout`, `-view`, `-output` with the short forms `-w`, `-l`, `-v`,
// `-o`; the `--` long forms are accepted too, as `export` accepts them.
fn parse_merge(mut args: impl Iterator<Item = OsString>) -> Result<Command, UsageError> {
    let (mut workspace, mut layout, mut view, mut out) = (None, None, None, None);
    while let Some(arg) = args.next() {
        let (slot, name): (&mut Option<OsString>, &'static str) = match arg.to_str() {
            Some("-w" | "-workspace" | "--workspace") => (&mut workspace, "-w"),
            Some("-l" | "-layout" | "--layout") => (&mut layout, "-l"),
            Some("-v" | "-view" | "--view") => (&mut view, "-v"),
            Some("-o" | "-output" | "--output") => (&mut out, "-o"),
            _ if is_flag(&arg) => return Err(UsageError::UnknownFlag(lossy(&arg))),
            _ => return Err(UsageError::ExtraArgument(lossy(&arg))),
        };
        let value = args.next().ok_or(UsageError::MissingValue(name))?;
        if slot.is_some() {
            return Err(UsageError::Repeated(name));
        }
        *slot = Some(value);
    }
    let workspace = workspace.ok_or(UsageError::MissingOption("-w FILE"))?;
    if is_url(&workspace) {
        return Err(UsageError::WorkspaceUrl(lossy(&workspace)));
    }
    Ok(Command::Merge {
        workspace: PathBuf::from(workspace),
        layout: PathBuf::from(layout.ok_or(UsageError::MissingOption("-l LAYOUT.json"))?),
        view: view.map(|v| lossy(&v)),
        out: PathBuf::from(out.ok_or(UsageError::MissingOption("-o OUT"))?),
    })
}

/// `generate system-landscape -i DIR -o OUT.dsl` (P3-428, P3-430):
/// `-i`/`-input`/`--input` and `-o`/`-output`/`--output`, each once.
fn parse_generate(mut args: impl Iterator<Item = OsString>) -> Result<Command, UsageError> {
    match args.next() {
        Some(g) if g == "system-landscape" => {}
        Some(g) => return Err(UsageError::UnknownGenerator(lossy(&g))),
        None => return Err(UsageError::UnknownGenerator(String::new())),
    }
    let (mut input_dir, mut out) = (None, None);
    while let Some(arg) = args.next() {
        let (slot, name): (&mut Option<OsString>, &'static str) = match arg.to_str() {
            Some("-i" | "-input" | "--input") => (&mut input_dir, "-i"),
            Some("-o" | "-output" | "--output") => (&mut out, "-o"),
            _ if is_flag(&arg) => return Err(UsageError::UnknownFlag(lossy(&arg))),
            _ => return Err(UsageError::ExtraArgument(lossy(&arg))),
        };
        let value = args.next().ok_or(UsageError::MissingValue(name))?;
        if slot.is_some() {
            return Err(UsageError::Repeated(name));
        }
        *slot = Some(value);
    }
    Ok(Command::Generate {
        input_dir: PathBuf::from(input_dir.ok_or(UsageError::MissingOption("-i DIR"))?),
        out: PathBuf::from(out.ok_or(UsageError::MissingOption("-o OUT.dsl"))?),
    })
}

/// A workspace value naming an `http://` or `https://` URL.
fn is_url(value: &OsString) -> bool {
    let bytes = value.as_encoded_bytes();
    ["http://", "https://"].iter().any(|scheme| {
        bytes.len() >= scheme.len() && bytes[..scheme.len()].eq_ignore_ascii_case(scheme.as_bytes())
    })
}

/// `help [COMMAND]`, `-h` or `--help` (B cli-breadth 9): `help` takes one
/// optional subcommand name, `-h` and `--help` take nothing.
fn parse_help(
    flag: &str,
    mut args: impl Iterator<Item = OsString>,
) -> Result<Command, UsageError> {
    let topic = match args.next() {
        None => None,
        Some(arg) if flag == "help" => {
            let name = COMMAND_LINES
                .iter()
                .map(|(name, _)| *name)
                .find(|name| arg.to_str() == Some(name))
                .ok_or_else(|| UsageError::UnknownCommand(lossy(&arg)))?;
            Some(name)
        }
        Some(arg) => return Err(UsageError::ExtraArgument(lossy(&arg))),
    };
    match args.next() {
        None => Ok(Command::Help { command: topic }),
        Some(extra) => Err(UsageError::ExtraArgument(lossy(&extra))),
    }
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

/// `FsLoader` that remembers every file it read, in order (P2-136): core
/// gives each file read the next `FileId` from 1 in first-read order and
/// reads each file once, so `read[i]` is the path of `FileId(i + 1)`,
/// also when the parse fails and no `Parsed::paths` comes back.
// DECISION: the CLI recovers the path of an error's file this way because
// `ModelError` names only the `FileId`, and a parse error returns no
// `Parsed`; adding a path to `ModelError` would be a pub change.
#[derive(Debug, Default)]
struct Recording {
    /// Every file read, in read order.
    read: Vec<PathBuf>,
}

impl model::Loader for Recording {
    fn read(&mut self, path: &Path) -> io::Result<Vec<u8>> {
        let bytes = model::FsLoader.read(path)?;
        self.read.push(path.to_path_buf());
        Ok(bytes)
    }

    fn read_dir(&mut self, path: &Path) -> io::Result<Vec<PathBuf>> {
        model::FsLoader.read_dir(path)
    }

    fn canonicalize(&mut self, path: &Path) -> io::Result<PathBuf> {
        model::FsLoader.canonicalize(path)
    }

    fn env(&self, name: &str) -> Option<String> {
        model::FsLoader.env(name)
    }
}

impl Recording {
    /// The included file `file` (`FileId(i)`, `i >= 1`), `None` for the
    /// root file or an id never read.
    fn path(&self, file: FileId) -> Option<&Path> {
        let i = usize::try_from(file.0).ok()?.checked_sub(1)?;
        self.read.get(i).map(PathBuf::as_path)
    }

    /// `file` as the report names it: FILE as given (`<stdin>`), or the
    /// included file's path.
    fn name(&self, input: &Input, file: FileId) -> String {
        self.path(file).map_or_else(
            || InputName(input).to_string(),
            |p| p.display().to_string(),
        )
    }
}

/// Parses `text`, read from `input`: FILE through `FsLoader` with its
/// path, so `!include` and `workspace extends` resolve beside it; stdin
/// with no path, so an include from stdin is `NeedsFile` (A.2 cli).
fn parse(input: &Input, text: &str, loader: &mut Recording) -> Result<model::Parsed, CliError> {
    let path = match input {
        Input::Path(path) => Some(path.as_path()),
        Input::Stdin => None,
    };
    model::parse_with(text, path, loader).map_err(|error| CliError::Parse {
        file: loader.name(input, error.span.file),
        error,
    })
}

/// What `render`, `lint`, `export`, `validate`, `inspect` and `list`
/// read: the model only.
struct Loaded {
    model: model::Model,
}

/// Parses `text` as `parse` does, or as Structurizr workspace JSON when
/// its first non-whitespace byte is `{` (P3-416).
fn parse_model(input: &Input, text: &str, loader: &mut Recording) -> Result<Loaded, CliError> {
    if text.trim_start().starts_with('{') {
        return model::parse_json(text.as_bytes(), loader)
            .map(|model| Loaded { model })
            .map_err(|error| CliError::Parse {
                file: InputName(input).to_string(),
                error,
            });
    }
    parse(input, text, loader).map(|p| Loaded { model: p.model })
}

/// The first pair of view keys equal up to ASCII case, in declaration
/// order (header DECISION).
fn key_collision(keys: &[&str]) -> Option<CliError> {
    export_files::case_collision(keys).map(|(first, second)| CliError::KeyCollision {
        first: first.to_owned(),
        second: second.to_owned(),
    })
}

/// Writes `bytes` to `path` atomically (approver ruling R1: a temporary
/// file in the target folder, then renamed over `path`, never written in
/// place), then its `wrote` line.
fn write_file(path: &Path, bytes: &[u8], stdout: &mut impl Write) -> Result<(), CliError> {
    export_files::replace_file(path, bytes).map_err(|source| CliError::Write {
        path: path.to_path_buf(),
        source,
    })?;
    report::wrote(stdout, path).map_err(CliError::Report)
}


/// The files `render` writes: `<key>.svg`, and `<key>.png` with `png`.
fn view_files(out_dir: &Path, keys: &[&str], png: bool) -> Vec<PathBuf> {
    keys.iter()
        .flat_map(|key| {
            let png = png.then(|| out_dir.join(format!("{key}.png")));
            std::iter::once(out_dir.join(format!("{key}.svg"))).chain(png)
        })
        .collect()
}

/// The files a run read: FILE (not stdin) and every file the loader
/// read for it (`!include`, `workspace extends`, themes), in read order.
fn inputs_of(input: Option<&Input>, loader: &Recording) -> Vec<PathBuf> {
    let file = match input {
        Some(Input::Path(p)) => Some(p.clone()),
        _ => None,
    };
    file.into_iter().chain(loader.read.iter().cloned()).collect()
}

/// R1: refuses (exit 2, nothing written) when an output is one of the
/// inputs: the same file by device and inode, so hard links and
/// symlinks count. Outputs that do not exist yet are no input.
fn refuse_inputs(outputs: &[PathBuf], inputs: &[PathBuf]) -> Result<(), CliError> {
    match export_files::input_hit(outputs, inputs) {
        Some((out, input)) => Err(CliError::Refused {
            path: out.to_path_buf(),
            message: format!(
                "it is an input ({}), which c4rizer never overwrites — choose another output path",
                input.display()
            ),
        }),
        None => Ok(()),
    }
}

/// The SVG of one view as `--svg-text` asks (P2-651): named, embedded or
/// outlined. The PNG never depends on it (P2-661).
fn svg_of(view: &pipeline::ViewBuild, svg_text: SvgText) -> Result<String, CliError> {
    match svg_text {
        SvgText::Name => Ok(export::svg(&view.scene)),
        SvgText::Embed => Ok(export::svg_embedded(&view.scene)),
        SvgText::Outline => outlined(view),
    }
}

/// The outlined SVG of one view (feature `outline`).
#[cfg(feature = "outline")]
fn outlined(view: &pipeline::ViewBuild) -> Result<String, CliError> {
    export::svg_outlined(&view.scene).map_err(|e| CliError::Outline {
        view: view.plan.key.clone(),
        message: e.to_string(),
    })
}

/// Without the `outline` feature `run` refuses `--svg-text outline` before
/// this is reached.
#[cfg(not(feature = "outline"))]
fn outlined(_view: &pipeline::ViewBuild) -> Result<String, CliError> {
    Err(CliError::OutlineUnavailable)
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
            mode: _,
            svg_text: _,
        } => (input, Some((out_dir, *png))),
        Command::Lint { input } => (input, None),
        Command::Version => {
            // DECISION: the line B cli-breadth 4 approves, `c4rizer
            // <version>`; Structurizr's version command prints its own
            // and its libraries' versions through its logger.
            writeln!(stdout, "c4rizer {}", env!("CARGO_PKG_VERSION")).map_err(CliError::Report)?;
            stdout.flush().map_err(CliError::Report)?;
            return Ok(Exit::Clean);
        }
        Command::Help { command } => {
            // DECISION: help goes to stdout and exits 0 (B cli-breadth 4
            // and 9); a usage error prints the same text to stderr.
            let text = match command {
                None => USAGE.to_owned(),
                Some(name) => COMMAND_LINES
                    .iter()
                    .find(|(n, _)| n == name)
                    .map(|(_, line)| format!("usage: {line}"))
                    .unwrap_or_default(),
            };
            writeln!(stdout, "{text}").map_err(CliError::Report)?;
            stdout.flush().map_err(CliError::Report)?;
            return Ok(Exit::Clean);
        }
        Command::Validate { input } | Command::Inspect { input, .. } | Command::List { input } => {
            return run_model_command(command, input, stdin, stdout);
        }
        Command::ImportLayout { json, file } => return run_import_layout(json, file, stdout),
        Command::Export { .. } => return run_export(command, stdin, stdout),
        Command::Merge { workspace, layout, view, out } => return run_merge(workspace, layout, view.as_deref(), out, stdout),
        Command::Generate { input_dir, out } => return run_generate(input_dir, out, stdout),
    };
    let mode = match command {
        Command::Render { mode, .. } => *mode,
        _ => Mode::Light,
    };
    if !cfg!(feature = "png") && matches!(render, Some((_, Some(_)))) {
        return Err(CliError::PngUnavailable);
    }
    let svg_text = match command {
        Command::Render { svg_text, .. } => *svg_text,
        _ => SvgText::Name,
    };
    // P2-662: refused before any file is read, like `--png`.
    if !cfg!(feature = "outline") && svg_text == SvgText::Outline {
        return Err(CliError::OutlineUnavailable);
    }
    let text = read_input(input, stdin)?;
    let mut loader = Recording::default();
    let parsed = parse_model(input, &text, &mut loader)?;
    // A flag that is never set: `build_all` cannot return `Stopped`.
    let Ok(all) = pipeline::build_all_in(&parsed.model, mode, &AtomicBool::new(false)) else {
        unreachable!("the stop flag is never set");
    };
    if let Some((out_dir, _)) = render {
        let keys: Vec<&str> = all.views.iter().map(|v| v.plan.key.as_str()).collect();
        if let Some(collision) = key_collision(&keys) {
            return Err(collision);
        }
        // R1: no view file may be one of the files read.
        let png = render.is_some_and(|(_, png)| png.is_some());
        refuse_inputs(&view_files(out_dir, &keys, png), &inputs_of(Some(input), &loader))?;
        // DECISION: `render` creates DIR even for a file with no views, so
        // a script can rely on DIR existing after exit 0 or 1.
        std::fs::create_dir_all(out_dir).map_err(|source| CliError::CreateDir {
            path: out_dir.clone(),
            source,
        })?;
    }
    for notice in &parsed.model.notices {
        let file = notice.span.map(|s| loader.name(input, s.file));
        report::notice(stdout, notice, file.as_deref()).map_err(CliError::Report)?;
    }
    if all.views.is_empty() && all.unsupported.is_empty() {
        report::no_views(stdout).map_err(CliError::Report)?;
    }
    // Every view in declaration order: drawn ones with their report and
    // files, unsupported ones with their line (P2-122).
    for spec in &parsed.model.views {
        if let Some(u) = all.unsupported.iter().find(|u| u.key == spec.key) {
            report::unsupported(stdout, u).map_err(CliError::Report)?;
            continue;
        }
        let Some(view) = all.view(&spec.key) else {
            continue;
        };
        report::view(stdout, &view.report, view.plan.title.as_deref()).map_err(CliError::Report)?;
        if let Some((out_dir, png)) = render {
            let key = &view.plan.key;
            let svg = svg_of(view, svg_text)?;
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

/// `import-layout JSON FILE` (P2-15, P2-504): reads both files, parses
/// FILE, matches the JSON's positions (`import::import_layout`), applies
/// them with one `edit::apply` and rewrites FILE atomically, then reports.
/// Nothing is written on any error, nor when there is no edit.
// DECISION: every file whose bytes the batch changed is rewritten: the
// root file, and an included file only if it holds a matched view's block
// (P2-140; P2-509 says the root file, which is where views live in every
// corpus file). Each write is atomic on its own.
// DECISION (report format, plan task 08): per matched view `== <key>` and
// `   wrote <n> positions`, then `   warning: <notice>` per notice of
// that view; `skipped <key>: <why>` per skipped view; then `warning:
// <notice>` for the notices of no matched view (themes, unknown views).
fn run_import_layout(json: &Path, file: &Path, stdout: &mut impl Write) -> Result<Exit, CliError> {
    let json_input = Input::Path(json.to_path_buf());
    let json_text = read_input(&json_input, &mut io::empty())?;
    let input = Input::Path(file.to_path_buf());
    let text = read_input(&input, &mut io::empty())?;
    let mut loader = Recording::default();
    let parsed = parse(&input, &text, &mut loader)?;
    let imported = import::import_layout(&parsed, &json_text).map_err(|error| CliError::Json {
        path: json.to_path_buf(),
        error,
    })?;
    if !imported.edits.is_empty() {
        let applied = edit::apply(&parsed, &imported.edits).map_err(|e| CliError::Edit {
            path: file.to_path_buf(),
            message: e.to_string(),
        })?;
        let mut writes: Vec<(PathBuf, &str)> = Vec::new();
        if applied.text != text {
            writes.push((file.to_path_buf(), applied.text.as_str()));
        }
        for (id, patched) in &applied.files {
            if let Some(path) = loader.path(*id) {
                // DECISION: a `workspace extends <file>.json` base is read
                // as DSL text written from the JSON (P3-419); writing that
                // text over the JSON would destroy it, so the edit is
                // refused (JSON is read-only, P3-420).
                if path.extension().is_some_and(|e| e.eq_ignore_ascii_case("json")) {
                    return Err(CliError::Edit {
                        path: path.to_path_buf(),
                        message: "it is a workspace JSON, which c4rizer never writes — move the view into the DSL file"
                            .into(),
                    });
                }
                writes.push((path.to_path_buf(), patched.as_str()));
            }
        }
        for (path, patched) in &writes {
            write_atomic(path, patched).map_err(|source| CliError::Rewrite {
                path: path.clone(),
                source,
            })?;
        }
    }
    report::imported(stdout, &imported).map_err(CliError::Report)?;
    stdout.flush().map_err(CliError::Report)?;
    Ok(Exit::Clean)
}

/// Writes `text` to `path` atomically (EDIT-22): a temporary file beside
/// the target, then renamed over it. A symlink is resolved first, so the
/// link's target is written and the link stays; the target's permissions
/// are copied. On any error the temporary file is removed and `path` is
/// untouched.
// DECISION: the editor's `files::write_atomic` does the same; the CLI
// cannot depend on the editor crate, so the few lines are repeated here.
fn write_atomic(path: &Path, text: &str) -> io::Result<()> {
    let target = std::fs::canonicalize(path)?;
    let dir = target.parent().map_or_else(|| PathBuf::from("."), Path::to_path_buf);
    let name = target
        .file_name()
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "the path names no file"))?;
    let mut tmp_name = OsString::from(".");
    tmp_name.push(name);
    tmp_name.push(format!(".c4rizer-{}.tmp", std::process::id()));
    let tmp = dir.join(tmp_name);
    let result = (|| {
        let permissions = std::fs::metadata(&target)?.permissions();
        let mut out = std::fs::File::create(&tmp)?;
        out.write_all(text.as_bytes())?;
        out.sync_all()?;
        std::fs::set_permissions(&tmp, permissions)?;
        std::fs::rename(&tmp, &target)
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(&tmp);
    }
    result
}

/// `merge -w FILE -l LAYOUT.json [-v KEY] -o OUT` (P3-425..P3-427):
/// the `at` edits `import-layout` would apply to FILE (matched by
/// canonical name, `import::import_layout`), applied in memory and
/// written to OUT. FILE is only read (P3-426). Reports as
/// `import-layout` does, then the `wrote` line.
// DECISION: OUT holds the root file's text only, so edits for views
// declared in an included file are left out (and not reported): OUT is
// "a copy of FILE" (P3-425), and an included file is not FILE.
// DECISION: OUT naming FILE, LAYOUT or a file FILE includes or extends
// (same device and inode, approver ruling R1) is refused before anything
// is written, as P3-426 says FILE never changes; a FILE that is
// workspace JSON is refused too, as c4rizer never writes JSON (P3-420).
// DECISION: `-v KEY` naming no view of FILE is refused (exit 2), so a
// typo never gives an unchanged copy that looks merged.
// DECISION: `via` lines: `import-layout` writes none yet (vertices row,
// P3-317), so neither does `merge`; the copy gains what `import-layout`
// would add, as P3-425 defines it.
fn run_merge(
    workspace: &Path,
    layout: &Path,
    view: Option<&str>,
    out: &Path,
    stdout: &mut impl Write,
) -> Result<Exit, CliError> {
    let input = Input::Path(workspace.to_path_buf());
    let text = read_input(&input, &mut io::empty())?;
    let layout_text = read_input(&Input::Path(layout.to_path_buf()), &mut io::empty())?;
    refuse_inputs(&[out.to_path_buf()], &[workspace.to_path_buf(), layout.to_path_buf()])?;
    if text.trim_start().starts_with('{') {
        return Err(CliError::Refused {
            path: workspace.to_path_buf(),
            message: "it is a workspace JSON, which c4rizer never writes — merge into the DSL file instead".into(),
        });
    }
    let mut loader = Recording::default();
    let parsed = parse(&input, &text, &mut loader)?;
    // R1: OUT is none of the files FILE includes or extends either.
    refuse_inputs(&[out.to_path_buf()], &inputs_of(None, &loader))?;
    if let Some(key) = view
        && parsed.model.view(key).is_none()
    {
        let keys: Vec<&str> = parsed.model.views.iter().map(|v| v.key.as_str()).collect();
        return Err(CliError::Refused {
            path: workspace.to_path_buf(),
            message: format!("no view \"{key}\" — give one of: {}", keys.join(", ")),
        });
    }
    let mut imported = import::import_layout(&parsed, &layout_text).map_err(|error| CliError::Json {
        path: layout.to_path_buf(),
        error,
    })?;
    let keep = |key: &str| {
        view.is_none_or(|v| v == key)
            && parsed.model.view(key).is_some_and(|s| s.span.file == FileId::ROOT)
    };
    imported.edits.retain(|e| matches!(e, edit::Edit::Move { view, .. } if keep(view)));
    imported.views.retain(|v| keep(&v.key));
    imported.notices.retain(|n| n.view.as_deref().is_none_or(keep));
    let merged = if imported.edits.is_empty() {
        text
    } else {
        edit::apply(&parsed, &imported.edits)
            .map_err(|e| CliError::Edit {
                path: workspace.to_path_buf(),
                message: e.to_string(),
            })?
            .text
    };
    report::imported(stdout, &imported).map_err(CliError::Report)?;
    write_file(out, merged.as_bytes(), stdout)?;
    stdout.flush().map_err(CliError::Report)?;
    Ok(Exit::Clean)
}


/// `generate system-landscape -i DIR -o OUT.dsl` (P3-428, P3-429): reads
/// every `*.dsl` and `*.json` file of DIR (not its subfolders) sorted by
/// path, as `render` reads FILE (`parse_model`), and writes one DSL file
/// with every person and software system once and the relationships
/// between them, and one `systemLandscape` view with `include *`.
// DECISION: people and software systems match by name across both kinds
// (Structurizr requires those names to be unique in a model); the first
// workspace that names one decides its kind, and the description is the
// first non-empty one (P3-429, for people too).
// DECISION: "the relationships between them" are every written
// relationship (not copied instance ones) whose ends, each lifted to its
// top-level person or software system, are two different ones; one
// relationship per ordered pair, with the first non-empty description
// and technology, as Structurizr's implied relationships keep one per
// pair.
// DECISION: the generated file carries names, descriptions and
// relationships only: tags, styles and views of the inputs are not
// copied. Identifiers are `person_<name>` / `system_<name>` in snake
// case, with a number added on a clash, so no identifier is a keyword.
// DECISION (approver ruling R2): OUT inside DIR (or a folder below it)
// is refused with exit 2 before anything is read, and OUT naming any
// file an input read (R1) is refused before anything is written.
fn run_generate(input_dir: &Path, out: &Path, stdout: &mut impl Write) -> Result<Exit, CliError> {
    let dir_error = |source| CliError::Read {
        input: Input::Path(input_dir.to_path_buf()),
        source,
    };
    let mut paths: Vec<PathBuf> = std::fs::read_dir(input_dir)
        .map_err(dir_error)?
        .map(|e| e.map(|e| e.path()))
        .collect::<io::Result<_>>()
        .map_err(dir_error)?;
    paths.retain(|p| {
        p.is_file() && p.extension().is_some_and(|e| e.eq_ignore_ascii_case("dsl") || e.eq_ignore_ascii_case("json"))
    });
    paths.sort();
    if inside(out, input_dir) {
        return Err(CliError::Refused {
            path: out.to_path_buf(),
            message: format!(
                "it lies inside the input folder {}, whose files generate reads — choose an output outside it",
                input_dir.display()
            ),
        });
    }
    let mut landscape = Landscape::default();
    let mut read = Vec::new();
    for path in &paths {
        let input = Input::Path(path.clone());
        let text = read_input(&input, &mut io::empty())?;
        let mut loader = Recording::default();
        let workspace = parse_model(&input, &text, &mut loader)?;
        landscape.add(&workspace.model);
        read.extend(inputs_of(Some(&input), &loader));
        writeln!(stdout, "   read {}", path.display()).map_err(CliError::Report)?;
    }
    refuse_inputs(&[out.to_path_buf()], &read)?;
    write_file(out, landscape.dsl().as_bytes(), stdout)?;
    stdout.flush().map_err(CliError::Report)?;
    Ok(Exit::Clean)
}

/// True when `path` (which need not exist) lies inside folder `dir`, at
/// any depth, once both are resolved.
fn inside(path: &Path, dir: &Path) -> bool {
    let Ok(dir) = std::fs::canonicalize(dir) else {
        return false;
    };
    // The deepest existing ancestor of `path`, resolved.
    let mut probe = path.parent().filter(|p| !p.as_os_str().is_empty()).unwrap_or(Path::new("."));
    loop {
        if let Ok(resolved) = std::fs::canonicalize(probe) {
            return resolved.starts_with(&dir);
        }
        match probe.parent() {
            Some(p) if !p.as_os_str().is_empty() => probe = p,
            _ => return false,
        }
    }
}

/// The landscape `generate` builds, in first-seen order.
#[derive(Debug, Default)]
struct Landscape {
    /// People and software systems: is a person, name, description.
    elements: Vec<(bool, String, String)>,
    /// Source and destination (indices into `elements`), description,
    /// technology.
    relationships: Vec<(usize, usize, String, String)>,
}

impl Landscape {
    /// Adds the people, software systems and relationships of `model`.
    fn add(&mut self, model: &model::Model) {
        use model::ElementKind;
        let mut index = vec![None; model.elements.len()];
        for e in &model.elements {
            let person = match e.kind {
                ElementKind::Person => true,
                ElementKind::SoftwareSystem => false,
                _ => continue,
            };
            let description = e.description.as_ref().map_or("", |d| d.value.as_str());
            let i = self.elements.iter().position(|(_, n, _)| *n == e.name.value).unwrap_or_else(|| {
                self.elements.push((person, e.name.value.clone(), String::new()));
                self.elements.len() - 1
            });
            if self.elements[i].2.is_empty() {
                description.clone_into(&mut self.elements[i].2);
            }
            index[e.id.0 as usize] = Some(i);
        }
        let top = |mut id: model::ElementId| {
            while let Some(parent) = model.element(id).parent {
                id = parent;
            }
            index[id.0 as usize]
        };
        for r in model.relationships.iter().filter(|r| r.copied_from.is_none()) {
            let (Some(s), Some(d)) = (top(r.source.value), top(r.destination.value)) else {
                continue;
            };
            if s == d {
                continue;
            }
            let text = |l: &Option<model::Literal>| l.as_ref().map_or(String::new(), |l| l.value.clone());
            let i = self.relationships.iter().position(|(a, b, _, _)| (*a, *b) == (s, d)).unwrap_or_else(|| {
                self.relationships.push((s, d, String::new(), String::new()));
                self.relationships.len() - 1
            });
            let slot = &mut self.relationships[i];
            if slot.2.is_empty() {
                slot.2 = text(&r.description);
            }
            if slot.3.is_empty() {
                slot.3 = text(&r.technology);
            }
        }
    }

    /// The landscape as a DSL file (P3-428).
    fn dsl(&self) -> String {
        use std::fmt::Write as _;
        let mut ids: Vec<String> = Vec::new();
        for (person, name, _) in &self.elements {
            let mut slug = String::from(if *person { "person_" } else { "system_" });
            let mut gap = false;
            for c in name.chars() {
                if c.is_ascii_alphanumeric() {
                    if gap && !slug.ends_with('_') {
                        slug.push('_');
                    }
                    slug.push(c.to_ascii_lowercase());
                    gap = false;
                } else {
                    gap = true;
                }
            }
            let mut id = slug.clone();
            let mut n = 2;
            while ids.contains(&id) {
                id = format!("{slug}_{n}");
                n += 1;
            }
            ids.push(id);
        }
        let mut out = String::from("workspace \"System Landscape\" {\n    model {\n");
        for ((person, name, description), id) in self.elements.iter().zip(&ids) {
            let keyword = if *person { "person" } else { "softwareSystem" };
            let _ = write!(out, "        {id} = {keyword} {}", quote(name));
            if !description.is_empty() {
                let _ = write!(out, " {}", quote(description));
            }
            out.push('\n');
        }
        if !self.relationships.is_empty() {
            out.push('\n');
        }
        for (s, d, description, technology) in &self.relationships {
            let _ = write!(out, "        {} -> {}", ids[*s], ids[*d]);
            if !description.is_empty() || !technology.is_empty() {
                let _ = write!(out, " {}", quote(description));
            }
            if !technology.is_empty() {
                let _ = write!(out, " {}", quote(technology));
            }
            out.push('\n');
        }
        out.push_str("    }\n    views {\n        systemLandscape \"SystemLandscape\" \"System Landscape\" {\n            include *\n        }\n    }\n}\n");
        out
    }
}

/// `text` as a DSL string literal: `"` and `\` escaped, line breaks and
/// other control characters as spaces (a DSL string is one line).
fn quote(text: &str) -> String {
    let mut out = String::with_capacity(text.len() + 2);
    out.push('"');
    for c in text.chars() {
        match c {
            '"' | '\\' => {
                out.push('\\');
                out.push(c);
            }
            c if c.is_control() => out.push(' '),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

/// `export -f <format>` (P3-400..P3-405): reads and builds FILE, writes
/// the format's files into the output folder, prints one `wrote` line
/// per file and one skip line per view the format cannot express.
// DECISION: `export` exits 0 once its files are written (P3-400), lint
// errors included: `-f svg|png` writes and reports as `render` does but
// does not turn its lint errors into exit 1.
// DECISION: without `-o`, a FILE read from stdin writes into the current
// folder; `json` of stdin is `workspace.json`.
// DECISION: a view a format cannot express gets the P3-405 line: image
// and filtered views for every text format (Structurizr's exporters
// write neither), every non-dynamic view for `websequencediagrams`.
fn run_export(
    command: &Command,
    stdin: &mut impl Read,
    stdout: &mut impl Write,
) -> Result<Exit, CliError> {
    let Command::Export {
        input,
        format,
        out_dir,
        positions,
    } = command
    else {
        unreachable!("run_export is called with Command::Export");
    };
    let out_dir = out_dir.clone().unwrap_or_else(|| match input {
        Input::Path(p) => p.parent().map_or_else(PathBuf::new, Path::to_path_buf),
        Input::Stdin => PathBuf::new(),
    });
    if matches!(format, ExportFormat::Svg | ExportFormat::Png) {
        let render = Command::Render {
            input: input.clone(),
            out_dir,
            png: (*format == ExportFormat::Png).then_some(Png { width: None }),
            mode: Mode::Light,
            svg_text: SvgText::Name,
        };
        return run(&render, stdin, stdout).map(|_| Exit::Clean);
    }
    let text = read_input(input, stdin)?;
    let mut loader = Recording::default();
    let parsed = parse_model(input, &text, &mut loader)?;
    let model = &parsed.model;
    let Ok(all) = pipeline::build_all(model, &AtomicBool::new(false)) else {
        unreachable!("the stop flag is never set");
    };
    let name = match format {
        ExportFormat::PlantUml(export::Flavour::Structurizr) => "plantuml/structurizr",
        ExportFormat::PlantUml(export::Flavour::C4PlantUml) => "plantuml/c4plantuml",
        ExportFormat::Mermaid => "mermaid",
        ExportFormat::WebSequenceDiagrams => "websequencediagrams",
        ExportFormat::Dot => "dot",
        ExportFormat::Ilograph => "ilograph",
        ExportFormat::Json => "json",
        ExportFormat::Theme => "theme",
        ExportFormat::Static => "static",
        ExportFormat::Svg | ExportFormat::Png => unreachable!("handled above"),
    };
    let source = export_files::Source {
        model,
        all: &all,
        path: match input {
            Input::Path(p) => Some(p.as_path()),
            Input::Stdin => None,
        },
        dsl: (!text.trim_start().starts_with('{')).then_some(text.as_str()),
    };
    let Some(files) = export_files::files(name, &source, *positions) else {
        unreachable!("export_files::files writes every format but svg and png");
    };
    // CLI-1's guard: keys equal up to case would overwrite each other's
    // files on a case-insensitive file system (per-view formats, static).
    if !matches!(format, ExportFormat::Json | ExportFormat::Theme | ExportFormat::Ilograph) {
        let keys: Vec<&str> = all.views.iter().map(|v| v.plan.key.as_str()).collect();
        if let Some(collision) = key_collision(&keys) {
            return Err(collision);
        }
    }
    // R1: no file written is one of the files read.
    let outputs: Vec<PathBuf> = files.iter().map(|(p, _)| out_dir.join(p)).collect();
    refuse_inputs(&outputs, &inputs_of(Some(input), &loader))?;
    std::fs::create_dir_all(&out_dir).map_err(|source| CliError::CreateDir {
        path: out_dir.clone(),
        source,
    })?;
    for (path, bytes) in &files {
        let path = out_dir.join(path);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(|source| CliError::CreateDir {
                path: parent.to_path_buf(),
                source,
            })?;
        }
        write_file(&path, bytes, stdout)?;
    }
    let per_view = matches!(
        format,
        ExportFormat::PlantUml(_)
            | ExportFormat::Mermaid
            | ExportFormat::WebSequenceDiagrams
            | ExportFormat::Dot
            | ExportFormat::Ilograph
    );
    if per_view {
        for spec in &model.views {
            let skipped = matches!(spec.kind, ViewKind::Image | ViewKind::Filtered)
                || (*format == ExportFormat::WebSequenceDiagrams && spec.kind != ViewKind::Dynamic);
            if skipped {
                report::skipped(stdout, &spec.key, name, spec.kind.keyword()).map_err(CliError::Report)?;
            }
        }
    }
    stdout.flush().map_err(CliError::Report)?;
    Ok(Exit::Clean)
}


/// `validate`, `inspect` or `list`: read and parse the workspace, never
/// place, route or lint (P2-605, P2-609, P2-613), never write a file.
fn run_model_command(
    command: &Command,
    input: &Input,
    stdin: &mut impl Read,
    stdout: &mut impl Write,
) -> Result<Exit, CliError> {
    if let Command::Inspect {
        inspector: Some(class),
        ..
    } = command
    {
        return Err(CliError::InspectorUnsupported(class.clone()));
    }
    let text = read_input(input, stdin)?;
    let mut loader = Recording::default();
    let parsed = parse_model(input, &text, &mut loader)?;
    match command {
        // DECISION: `validate` prints nothing on success (B cli-breadth 4;
        // Structurizr's validate prints nothing either). Notices are not
        // printed: they are no error and change nothing.
        Command::Validate { .. } => {}
        Command::List { .. } => {
            report::elements(stdout, &parsed.model).map_err(CliError::Report)?;
        }
        Command::Inspect { severities, .. } => {
            let shown: Vec<inspect::Violation> = inspect::inspect(&parsed.model)
                .into_iter()
                .filter(|v| severities.contains(&v.severity))
                .collect();
            report::violations(stdout, &parsed.model, &shown).map_err(CliError::Report)?;
            stdout.flush().map_err(CliError::Report)?;
            let errors = shown
                .iter()
                .filter(|v| v.severity == inspect::Severity::Error)
                .count();
            // DECISION: B cli-breadth 5: the exit code counts the
            // error-severity violations shown, capped at 254.
            return Ok(Exit::Violations(
                u8::try_from(errors.min(254)).unwrap_or(254),
            ));
        }
        _ => unreachable!("only validate, inspect and list come here"),
    }
    stdout.flush().map_err(CliError::Report)?;
    Ok(Exit::Clean)
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
            let inspect = std::env::args_os().nth(1).is_some_and(|a| a == "inspect");
            return ExitCode::from(if inspect { INSPECT_ERROR } else { 2 });
        }
    };
    match run(&command, &mut io::stdin().lock(), &mut io::stdout().lock()) {
        Ok(exit) => exit_code(exit),
        Err(error) => {
            eprintln!("c4rizer: {error}");
            // P3-430: `merge` and `generate` exit 1 on a parse error of
            // any input (FILE, LAYOUT or a workspace of DIR).
            if matches!(command, Command::Merge { .. } | Command::Generate { .. })
                && matches!(error, CliError::Parse { .. } | CliError::Json { .. })
            {
                return ExitCode::from(1);
            }
            // DECISION: P2-643 pins exit 2 for a refused `-inspector`; B
            // cli-breadth 5's 255 covers inspect's parse, I/O and usage
            // errors, and a refused inspector is none of those.
            let inspect = matches!(command, Command::Inspect { .. })
                && !matches!(error, CliError::InspectorUnsupported(_));
            ExitCode::from(if inspect { INSPECT_ERROR } else { 2 })
        }
    }
}

/// `inspect`'s exit code on a parse, I/O or usage error (B cli-breadth 5):
/// above every violation count, which stops at 254.
// DECISION: B cli-breadth 5 rules the P2-642 TODOs: counts cap at 254 and
// every `inspect` error exits 255, so no count is ever mistaken for an
// error. Structurizr's own `inspect` exits 1 for any shown violation or
// error and 0 otherwise (deferred.md, differences).
const INSPECT_ERROR: u8 = 255;

/// The process exit code of a finished run (CLI-2).
fn exit_code(exit: Exit) -> ExitCode {
    match exit {
        Exit::Clean => ExitCode::SUCCESS,
        Exit::LintErrors => ExitCode::from(1),
        Exit::Violations(n) => ExitCode::from(n),
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
            mode: c4rizer_core::model::Mode::Light,
            svg_text: super::SvgText::Name,
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
            mode: c4rizer_core::model::Mode::Light,
            svg_text: super::SvgText::Name,
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
                mode: c4rizer_core::model::Mode::Light,
            svg_text: super::SvgText::Name,
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
            mode: c4rizer_core::model::Mode::Light,
            svg_text: super::SvgText::Name,
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

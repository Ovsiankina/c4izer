//! Tests of the compat-fixtures corpus (SRS 4.6: P2-14, P2-28,
//! P2-500..P2-503) over `tests/data/structurizr/`: the Structurizr DSL
//! test resources at commit `ad34036c5a1227535fc9adea9a0f97de5962189d`,
//! copied unchanged (Apache-2.0). One `#[test]` per requirement, named
//! after it.
//!
//! The corpus is third-party data kept unchanged under its licence, so
//! SRS 5.4 (roles, not personal names) does not apply to it (Part B
//! compat-fixtures 5).

use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Write as _;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

use c4rizer_core::export;
use c4rizer_core::lint::{Rule, Severity};
use c4rizer_core::model::{self, FsLoader, Parsed, ViewKind};
use c4rizer_core::pipeline::{self, BuildError};
use c4rizer_core::place::Keep;

/// `tests/data/structurizr/` of the workspace.
fn corpus() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/data/structurizr")
}

// ---- P2-500: SHA-1 of git blobs (test-only helper) -------------------

/// SHA-1 (FIPS 180-4) of `data`, as 40 lowercase hex digits.
#[allow(clippy::many_single_char_names)] // FIPS 180-4's own names: h, w, a..e, f, k, t
fn sha1_hex(data: &[u8]) -> String {
    let mut h: [u32; 5] = [0x6745_2301, 0xEFCD_AB89, 0x98BA_DCFE, 0x1032_5476, 0xC3D2_E1F0];
    let mut msg = data.to_vec();
    let bit_len = (data.len() as u64).wrapping_mul(8);
    msg.push(0x80);
    while msg.len() % 64 != 56 {
        msg.push(0);
    }
    msg.extend_from_slice(&bit_len.to_be_bytes());
    for chunk in msg.chunks_exact(64) {
        let mut w = [0u32; 80];
        for (i, word) in chunk.chunks_exact(4).enumerate() {
            w[i] = u32::from_be_bytes([word[0], word[1], word[2], word[3]]);
        }
        for i in 16..80 {
            w[i] = (w[i - 3] ^ w[i - 8] ^ w[i - 14] ^ w[i - 16]).rotate_left(1);
        }
        let [mut a, mut b, mut c, mut d, mut e] = h;
        for (i, &wi) in w.iter().enumerate() {
            let (f, k) = match i {
                0..=19 => ((b & c) | (!b & d), 0x5A82_7999),
                20..=39 => (b ^ c ^ d, 0x6ED9_EBA1),
                40..=59 => ((b & c) | (b & d) | (c & d), 0x8F1B_BCDC),
                _ => (b ^ c ^ d, 0xCA62_C1D6),
            };
            let t = a.rotate_left(5).wrapping_add(f).wrapping_add(e).wrapping_add(k).wrapping_add(wi);
            e = d;
            d = c;
            c = b.rotate_left(30);
            b = a;
            a = t;
        }
        for (x, y) in h.iter_mut().zip([a, b, c, d, e]) {
            *x = x.wrapping_add(y);
        }
    }
    h.iter().fold(String::new(), |mut s, x| {
        let _ = write!(s, "{x:08x}");
        s
    })
}

/// The git blob id of a file's bytes: SHA-1 of `blob <len>\0<bytes>`.
fn git_blob_id(bytes: &[u8]) -> String {
    let mut data = format!("blob {}\0", bytes.len()).into_bytes();
    data.extend_from_slice(bytes);
    sha1_hex(&data)
}

/// Every regular file under `dir`, as `/`-separated paths relative to
/// it, sorted byte-wise.
fn files_under(dir: &Path) -> Vec<String> {
    fn walk(root: &Path, dir: &Path, out: &mut Vec<String>) {
        let mut entries: Vec<PathBuf> = std::fs::read_dir(dir)
            .unwrap_or_else(|e| panic!("cannot list {}: {e}", dir.display()))
            .map(|e| e.expect("directory entry").path())
            .collect();
        entries.sort();
        for path in entries {
            if path.is_dir() {
                walk(root, &path, out);
            } else {
                let rel = path.strip_prefix(root).expect("under root");
                let parts: Vec<String> =
                    rel.components().map(|c| c.as_os_str().to_string_lossy().into_owned()).collect();
                out.push(parts.join("/"));
            }
        }
    }
    let mut out = Vec::new();
    walk(dir, dir, &mut out);
    out.sort();
    out
}

/// Lines of `text` that are neither blank nor `#` comments.
fn data_lines(text: &str) -> impl Iterator<Item = &str> {
    text.lines().map(str::trim_end).filter(|l| !l.trim().is_empty() && !l.starts_with('#'))
}

/// `SOURCE` as path → blob id.
fn source_list() -> BTreeMap<String, String> {
    let text = std::fs::read_to_string(corpus().join("SOURCE")).expect("SOURCE present");
    let mut out = BTreeMap::new();
    for line in data_lines(&text) {
        let (id, path) = line
            .split_once(' ')
            .unwrap_or_else(|| panic!("SOURCE line `{line}` is not `<blob id> <path>`"));
        assert!(out.insert(path.to_owned(), id.to_owned()).is_none(), "SOURCE lists {path} twice");
    }
    out
}

#[test]
fn p2_500_corpus_pinned_source() {
    // The helper itself, against git's id of the empty blob.
    assert_eq!(git_blob_id(b""), "e69de29bb2d1d6434b8b29ae775ad8c2e48c5391");
    let listed = source_list();
    let on_disk: BTreeSet<String> =
        files_under(&corpus()).into_iter().filter(|p| p != "SOURCE" && p != "MANIFEST").collect();
    let mut problems = Vec::new();
    for (path, id) in &listed {
        match std::fs::read(corpus().join(path)) {
            Ok(bytes) => {
                let got = git_blob_id(&bytes);
                if &got != id {
                    problems.push(format!("{path}: blob {got}, SOURCE says {id}"));
                }
            }
            Err(e) => problems.push(format!("{path}: listed but missing ({e})")),
        }
    }
    for path in &on_disk {
        if !listed.contains_key(path) {
            problems.push(format!("{path}: present but not in SOURCE"));
        }
    }
    assert!(problems.is_empty(), "corpus differs from SOURCE:\n{}", problems.join("\n"));
}

// ---- P2-501: MANIFEST ------------------------------------------------

/// How MANIFEST classifies a `.dsl` file.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Class {
    Root,
    Fragment,
    Excluded,
}

/// The reasons P2-501 allows for `excluded`, in its order.
const REASONS: [&str; 5] =
    ["rejected-by-structurizr", "needs-network", "needs-jvm", "needs-env", "needs-encoding"];

/// MANIFEST as path → class; panics on a malformed or duplicate line.
fn manifest() -> BTreeMap<String, Class> {
    let text = std::fs::read_to_string(corpus().join("MANIFEST")).expect("MANIFEST present");
    let mut out = BTreeMap::new();
    for line in data_lines(&text) {
        let mut words = line.split_whitespace();
        let path = words.next().expect("path").to_owned();
        let class = match (words.next(), words.next(), words.next()) {
            (Some("root"), None, None) => Class::Root,
            (Some("fragment"), None, None) => Class::Fragment,
            (Some("excluded"), Some(reason), None) => {
                assert!(
                    REASONS.contains(&reason),
                    "MANIFEST: {path} has reason `{reason}`, not one of {REASONS:?}"
                );
                Class::Excluded
            }
            _ => panic!("MANIFEST line `{line}` is not `<path> root|fragment|excluded <reason>`"),
        };
        assert!(out.insert(path.clone(), class).is_none(), "MANIFEST lists {path} twice");
    }
    out
}

#[test]
fn p2_501_manifest_covers_every_dsl() {
    let m = manifest();
    let dsl: BTreeSet<String> =
        files_under(&corpus()).into_iter().filter(|p| Path::new(p).extension().is_some_and(|e| e == "dsl")).collect();
    let missing: Vec<&String> = dsl.iter().filter(|p| !m.contains_key(*p)).collect();
    let stale: Vec<&String> = m.keys().filter(|p| !dsl.contains(*p)).collect();
    assert!(missing.is_empty(), "no MANIFEST line for {missing:?}");
    assert!(stale.is_empty(), "MANIFEST names missing files {stale:?}");
    let text = std::fs::read_to_string(corpus().join("MANIFEST")).expect("MANIFEST");
    let order: Vec<&str> = data_lines(&text).map(|l| l.split_whitespace().next().unwrap_or("")).collect();
    let mut sorted = order.clone();
    sorted.sort_unstable();
    assert_eq!(order, sorted, "MANIFEST is sorted by path");
    assert!(m.values().any(|c| *c == Class::Fragment) && m.values().any(|c| *c == Class::Excluded));
    // The reasons are exactly the ones P2-501 lists.
    let srs = std::fs::read_to_string(Path::new(env!("CARGO_MANIFEST_DIR")).join("../../docs/srs.md")).expect("srs");
    let start = srs.find("- **P2-501**").expect("P2-501");
    let end = srs[start..].find("- **P2-502**").map_or(srs.len(), |e| start + e);
    let listed: Vec<&str> = srs[start..end]
        .lines()
        .filter_map(|l| l.trim_start().strip_prefix("- `")?.split_once("`:").map(|(r, _)| r))
        .collect();
    assert_eq!(listed, REASONS, "P2-501's reason list");
}

// ---- P2-502: licence and source ----------------------------------------

#[test]
fn p2_502_corpus_licence_present() {
    let licence = std::fs::read_to_string(corpus().join("LICENSE")).expect("LICENSE present");
    assert!(
        licence.contains("Apache License") && licence.contains("Version 2.0"),
        "LICENSE is the Apache-2.0 text"
    );
    let source = std::fs::read_to_string(corpus().join("SOURCE")).expect("SOURCE present");
    for needle in [
        "https://github.com/structurizr/structurizr",
        "ad34036c5a1227535fc9adea9a0f97de5962189d",
        "structurizr-dsl/src/test/resources/dsl/",
        "structurizr-export/src/test/resources/big-bank-plc.json",
        "unmodified",
    ] {
        assert!(source.contains(needle), "SOURCE names `{needle}`");
    }
}

// ---- P2-14: every root opens and renders --------------------------------

/// Roots c4rizer cannot open yet, each with a `docs/deferred.md` row
/// naming what blocks it (never an `excluded` MANIFEST line).
/// `p2_14_corpus_parses` checks every other root;
/// `p2_14_corpus_parses_deferred_roots` checks these and is ignored until
/// the rows are settled. After the fix rounds one is left: a phase-3
/// feature (JSON in, P3-416..P3-420) whose span contract the owner has to
/// rule.
// DECISION: the oracle test stays strict on the 82 roots that open, and
// the gap root moves to one ignored test, so this row is green without
// hiding a gap: the list below is the gap list of the report.
const KNOWN_GAPS: [(&str, &str); 1] = [(
    "extend/extend-workspace-from-json-file.dsl",
    "phase 3 (P3-416..P3-420, JSON in): `workspace extends <file>.json`; span contract for JSON-sourced elements to rule",
)];

/// The MANIFEST roots, in path order.
fn roots() -> Vec<String> {
    manifest().into_iter().filter(|(_, c)| *c == Class::Root).map(|(p, _)| p).collect()
}

/// The repository's own model, read where it lives (P2-14).
fn own_workspace() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../docs/workspace.dsl")
}

/// Parses the file at `path` through the P2-5 entry point; the error is
/// the first one, as the CLI prints it.
fn open(path: &Path) -> Result<Parsed, String> {
    let bytes = std::fs::read(path).map_err(|e| format!("cannot read: {e}"))?;
    let text = String::from_utf8(bytes).map_err(|e| format!("not UTF-8: {e}"))?;
    model::parse_with(&text, Some(path), &mut FsLoader).map_err(|e| e.to_string())
}

/// The roots `p2_14_corpus_parses` must open, plus `docs/workspace.dsl`.
fn openable() -> Vec<(String, PathBuf)> {
    let gaps: BTreeSet<&str> = KNOWN_GAPS.iter().map(|(p, _)| *p).collect();
    let mut out: Vec<(String, PathBuf)> = roots()
        .into_iter()
        .filter(|p| !gaps.contains(p.as_str()))
        .map(|p| {
            let path = corpus().join(&p);
            (p, path)
        })
        .collect();
    out.push(("docs/workspace.dsl (c4rizer)".to_owned(), own_workspace()));
    out
}

/// The repository's own model shows that `import` builds on `text` (its
/// JSON reader reports `text::Position`s), and draws that edge.
#[test]
fn p2_14_own_workspace_import_uses_text() {
    let parsed = open(&own_workspace()).expect("docs/workspace.dsl parses");
    let m = &parsed.model;
    let (import, text) = (m.find("cImport").expect("cImport"), m.find("cText").expect("cText"));
    let edge = m
        .relationships
        .iter()
        .find(|r| r.source.value == import && r.destination.value == text)
        .expect("cImport -> cText");
    // Both ends in one view: build it and find the line.
    let drawn = m.views.iter().filter(|v| m.included(v).contains(&import) && m.included(v).contains(&text)).any(|v| {
        pipeline::build_view(m, &v.key, &Keep::default(), &AtomicBool::new(false))
            .is_ok_and(|b| b.scene.lines.iter().any(|l| l.relationships.contains(&edge.id)))
    });
    assert!(drawn, "cImport -> cText is drawn in a view");
}

#[test]
fn p2_14_corpus_parses() {
    let roots: BTreeSet<String> = roots().into_iter().collect();
    for (path, _) in KNOWN_GAPS {
        assert!(roots.contains(path), "KNOWN_GAPS names {path}, which is not a MANIFEST root");
    }
    // Approver ruling: JSON in (exporters row, P3-416..P3-420) is the only
    // gap left.
    assert_eq!(KNOWN_GAPS.map(|(p, _)| p), ["extend/extend-workspace-from-json-file.dsl"]);
    let failures: Vec<String> = openable()
        .into_iter()
        .filter_map(|(name, path)| open(&path).err().map(|e| format!("{name}: {e}")))
        .collect();
    assert!(failures.is_empty(), "{} root(s) do not parse:\n{}", failures.len(), failures.join("\n"));
}

#[test]
#[ignore = "deferred"]
fn p2_14_corpus_parses_deferred_roots() {
    let failures: Vec<String> = KNOWN_GAPS
        .iter()
        .filter_map(|(p, owner)| open(&corpus().join(p)).err().map(|e| format!("{p} ({owner}): {e}")))
        .collect();
    assert!(failures.is_empty(), "{} root(s) do not parse:\n{}", failures.len(), failures.join("\n"));
}

/// `f` over every item on all cores, results in item order.
// DECISION: views are independent (`build_all` is a loop of
// `build_spec` with a fresh `Keep`), so the debug-build corpus test
// spreads (file, view) pairs over threads; results come back in item
// order, so the output does not depend on scheduling.
fn parallel<I: Sync, T: Send>(items: &[I], f: impl Fn(&I) -> T + Sync) -> Vec<T> {
    let threads = std::thread::available_parallelism().map_or(4, std::num::NonZero::get);
    let next = AtomicUsize::new(0);
    let mut out: Vec<(usize, T)> = std::thread::scope(|scope| {
        let handles: Vec<_> = (0..threads)
            .map(|_| {
                scope.spawn(|| {
                    let mut mine = Vec::new();
                    loop {
                        let i = next.fetch_add(1, Ordering::Relaxed);
                        let Some(item) = items.get(i) else { break };
                        mine.push((i, f(item)));
                    }
                    mine
                })
            })
            .collect();
        handles.into_iter().flat_map(|h| h.join().expect("worker")).collect()
    });
    out.sort_by_key(|(i, _)| *i);
    out.into_iter().map(|(_, t)| t).collect()
}

/// Every view of every openable root builds and gives an SVG document;
/// only a view kind c4rizer does not draw yet (dynamic) may be refused as
/// not supported. Deployment views draw since the deployment-view row:
/// a refused one is a failure, and the corpus must draw some.
#[test]
fn p2_14_corpus_drawable_views_render() {
    let files: Vec<(String, Parsed)> =
        openable().into_iter().filter_map(|(name, path)| open(&path).ok().map(|p| (name, p))).collect();
    let tasks: Vec<(usize, usize)> = files
        .iter()
        .enumerate()
        .flat_map(|(f, (_, parsed))| (0..parsed.model.views.len()).map(move |v| (f, v)))
        .collect();
    let results = parallel(&tasks, |&(f, v)| {
        let (name, parsed) = &files[f];
        let view = &parsed.model.views[v];
        let built = pipeline::build_view(&parsed.model, &view.key, &Keep::default(), &AtomicBool::new(false));
        let must_draw = view.kind.drawable() || view.kind == ViewKind::Deployment;
        match (must_draw, built) {
            (true, Ok(view_build)) => {
                let svg = export::svg(&view_build.scene);
                if svg.starts_with("<svg") && svg.trim_end().ends_with("</svg>") {
                    Ok(Some(view.kind))
                } else {
                    Err(format!("{name}: view {} gives no SVG document", view.key))
                }
            }
            (false, Err(BuildError::Unsupported(_))) => Ok(None),
            (true, Err(e)) => Err(format!("{name}: view {} does not build: {e}", view.key)),
            (false, _) => Err(format!("{name}: view {} is not marked not yet supported", view.key)),
        }
    });
    let failures: Vec<&String> = results.iter().filter_map(|r| r.as_ref().err()).collect();
    let drawn = results.iter().filter(|r| matches!(r, Ok(Some(_)))).count();
    let deployment = results.iter().filter(|r| matches!(r, Ok(Some(ViewKind::Deployment)))).count();
    assert!(failures.is_empty(), "{failures:#?}");
    assert!(drawn > 40, "the corpus draws views ({drawn})");
    assert!(deployment >= 10, "the corpus draws deployment views ({deployment})");
}

// ---- P2-28, P2-503: house-style lint report -----------------------------

/// The house-style rules: c4rizer rules a plain Structurizr file cannot
/// be expected to satisfy.
// TODO(srs): P2-28, whether other c4rizer-only error rules (L8, L9, L10,
// L12, L13, L18, L21, L22) count as house style. Reading taken (Part B
// compat-fixtures 4): {L4, L15} until the owner reads the first report.
const HOUSE_STYLE: [Rule; 2] = [Rule::Crossings, Rule::C1Technology];

/// Where one root lands in the P2-28 totals.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Verdict {
    /// No lint error.
    Clean,
    /// Every lint error is a house-style rule.
    HouseStyleOnly,
    /// Some lint error is not house style.
    Other,
}

/// The P2-28 verdict for the rules one file breaks; warnings do not
/// count.
// DECISION: the classifier takes rules, not `lint::Finding`s:
// `Finding` is `#[non_exhaustive]`, so a test outside core cannot make
// one by hand, and the verdict reads nothing but the rule.
fn classify(rules: &[Rule]) -> Verdict {
    let errors: Vec<Rule> = rules.iter().copied().filter(|r| r.severity() == Severity::Error).collect();
    if errors.is_empty() {
        Verdict::Clean
    } else if errors.iter().all(|r| HOUSE_STYLE.contains(r)) {
        Verdict::HouseStyleOnly
    } else {
        Verdict::Other
    }
}

/// For each error rule, how many files break it at least once (P2-503).
fn per_rule_file_counts(files: &[Vec<Rule>]) -> BTreeMap<Rule, usize> {
    let mut counts = BTreeMap::new();
    for rules in files {
        let distinct: BTreeSet<Rule> = rules.iter().copied().filter(|r| r.severity() == Severity::Error).collect();
        for rule in distinct {
            *counts.entry(rule).or_insert(0) += 1;
        }
    }
    counts
}

#[test]
fn p2_28_classify_house_style_only() {
    assert_eq!(classify(&[]), Verdict::Clean);
    assert_eq!(classify(&[Rule::Crossings]), Verdict::HouseStyleOnly);
    assert_eq!(classify(&[Rule::C1Technology, Rule::Crossings, Rule::C1Technology]), Verdict::HouseStyleOnly);
    assert_eq!(classify(&[Rule::Crossings, Rule::ThroughBox]), Verdict::Other);
    assert_eq!(classify(&[Rule::Unroutable]), Verdict::Other);
    let warning = Rule::TextFit;
    assert_eq!(warning.severity(), Severity::Warning);
    assert_eq!(classify(&[warning]), Verdict::Clean, "warnings are not counted");
    assert_eq!(classify(&[warning, Rule::Crossings]), Verdict::HouseStyleOnly);
}

#[test]
fn p2_503_per_rule_file_counts() {
    let files = vec![
        vec![Rule::Crossings, Rule::Crossings, Rule::ThroughBox],
        vec![Rule::Crossings, Rule::C1Technology],
        vec![],
    ];
    let counts = per_rule_file_counts(&files);
    assert_eq!(counts.get(&Rule::Crossings), Some(&2), "twice in one file counts once");
    assert_eq!(counts.get(&Rule::ThroughBox), Some(&1));
    assert_eq!(counts.get(&Rule::C1Technology), Some(&1));
    assert_eq!(counts.len(), 3);
    assert!(per_rule_file_counts(&[vec![Rule::TextFit]]).is_empty(), "warnings are not counted");
}

/// The error and warning rules one root breaks over all its built views
/// plus the file-level L22 findings, or why it does not open.
fn lint_rules(path: &Path) -> Result<Vec<Rule>, String> {
    let parsed = open(path)?;
    let all = pipeline::build_all(&parsed.model, &AtomicBool::new(false)).expect("never stopped");
    let mut rules: Vec<Rule> = all
        .views
        .iter()
        .flat_map(|v| v.report.findings.iter().map(|f| f.rule))
        .chain(all.coverage.iter().map(|f| f.rule))
        .collect();
    rules.sort_unstable();
    rules.dedup();
    Ok(rules)
}

#[test]
#[ignore = "run by hand: prints the house-style report (P2-28)"]
fn p2_28_house_style_report() {
    let roots = roots();
    let results = parallel(&roots, |p| lint_rules(&corpus().join(p)));
    let mut out = String::new();
    let (mut clean, mut house, mut other, mut closed) = (0, 0, 0, 0);
    let mut opened = Vec::new();
    for (path, result) in roots.iter().zip(&results) {
        match result {
            Ok(rules) => {
                let errors: Vec<&str> =
                    rules.iter().filter(|r| r.severity() == Severity::Error).map(|r| r.code()).collect();
                let verdict = classify(rules);
                match verdict {
                    Verdict::Clean => clean += 1,
                    Verdict::HouseStyleOnly => house += 1,
                    Verdict::Other => other += 1,
                }
                let _ = writeln!(out, "{path}: {verdict:?} [{}]", errors.join(" "));
                opened.push(rules.clone());
            }
            Err(e) => {
                closed += 1;
                let _ = writeln!(out, "{path}: does not open: {e}");
            }
        }
    }
    let _ = writeln!(out, "files with no lint error: {clean}");
    let _ = writeln!(out, "files whose lint errors are all house style ({}): {house}",
        HOUSE_STYLE.map(Rule::code).join(", "));
    let _ = writeln!(out, "files with any other lint error: {other}");
    let _ = writeln!(out, "roots that do not open (KNOWN_GAPS): {closed}");
    for (rule, n) in per_rule_file_counts(&opened) {
        let _ = writeln!(out, "{}: {n} file(s)", rule.code());
    }
    println!("{out}");
    // DECISION: P2-28's three totals sum to the roots that open; the roots
    // that do not open yet (KNOWN_GAPS) are a fourth line, so the sum
    // still covers every root.
    assert_eq!(clean + house + other + closed, roots.len());
    assert_eq!(closed, KNOWN_GAPS.len());
}

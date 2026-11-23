//! Test helpers: temp directories without a new dependency.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

/// A fresh directory under the system temp dir, removed on drop.
pub struct TempDir(PathBuf);

impl TempDir {
    /// Creates `<tmp>/c4rizer-editor-<pid>-<n>-<tag>`.
    pub fn new(tag: &str) -> TempDir {
        static N: AtomicU64 = AtomicU64::new(0);
        let n = N.fetch_add(1, Ordering::Relaxed);
        let dir = std::env::temp_dir().join(format!("c4rizer-editor-{}-{n}-{tag}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("create temp dir");
        TempDir(dir)
    }

    /// The directory.
    pub fn path(&self) -> &Path {
        &self.0
    }

    /// `name` inside the directory.
    pub fn join(&self, name: &str) -> PathBuf {
        self.0.join(name)
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// A small shop, roles only: a person, a system with two containers (one
/// with two components, no component view: EDIT-15 default view), an
/// external system; a lifted line `shop -> bank` merging two
/// relationships; a context and a container view.
pub const SHOP: &str = r#"workspace "Shop" "An online shop." {
    model {
        customer = person "Customer" "Buys things."
        shop = softwareSystem "Shop" "Sells things." {
            web = container "Web app" "Serves pages." "Rust" {
                cart = component "Cart" "Holds items." "Rust module"
                pay = component "Payments" "Charges cards." "Rust module"
            }
            db = container "Database" "Stores orders." "SQLite" "Database"
        }
        bank = softwareSystem "Bank" "Moves money." "External"
        customer -> shop "Visits" "HTTPS"
        customer -> web "Orders through" "HTTPS"
        web -> db "Reads and writes" "SQL"
        pay -> bank "Charges through" "HTTPS"
        cart -> bank "Refunds through"
        cart -> pay "Checks out with"
    }
    views {
        systemContext shop "context" "The shop and its users" {
            include *
        }
        container shop "containers" "Inside the shop" {
            include *
        }
        styles {
            element "Database" {
                shape Cylinder
            }
        }
    }
}
"#;

#[test]
fn shop_fixture_parses_and_builds() {
    use std::sync::atomic::AtomicBool;
    let parsed = c4rizer_core::model::parse(SHOP).expect("fixture parses");
    let stop = AtomicBool::new(false);
    for key in ["context", "containers"] {
        let build = c4rizer_core::pipeline::build_view(&parsed.model, key, &c4rizer_core::place::Keep::default(), &stop).expect("builds");
        assert!(!build.scene.boxes.is_empty());
    }
}

/// Which identifiers a fixture is written with (P2-6, P2-142): a test
/// body taking it runs once per mode, so every gesture that names an
/// element is exercised on nested elements whose bare id does not resolve
/// under `!identifiers hierarchical`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Ids {
    /// The default: an element is named by its bare id.
    Flat,
    /// `!identifiers hierarchical`: a nested element is named by its
    /// dotted full id (`shop.web`).
    Hierarchical,
}

/// The full ids of `SHOP`'s nested elements, by bare id.
pub const SHOP_NESTED: &[(&str, &str)] = &[
    ("web", "shop.web"),
    ("db", "shop.db"),
    ("api", "shop.api"),
    ("cart", "shop.web.cart"),
    ("pay", "shop.web.pay"),
];

/// The full ids of the nested containers of the `s` system that the bus
/// and bridge fixtures declare, by bare id.
pub const S_NESTED: &[(&str, &str)] = &[
    ("a", "s.a"),
    ("b", "s.b"),
    ("c", "s.c"),
    ("d", "s.d"),
    ("x", "s.x"),
    ("y", "s.y"),
    ("z", "s.z"),
];

impl Ids {
    /// `text`, a workspace written with flat ids, in this mode, for the
    /// nested elements of `SHOP_NESTED`.
    pub fn workspace(self, text: &str) -> String {
        self.workspace_in(text, SHOP_NESTED)
    }

    /// `text`, a workspace written with flat ids, in this mode: under
    /// `Hierarchical` the `!identifiers hierarchical` line opens the
    /// workspace body and every reference to a nested element of `nested`
    /// (not its declaration) is its full id.
    pub fn workspace_in(self, text: &str, nested: &[(&str, &str)]) -> String {
        match self {
            Ids::Flat => text.to_owned(),
            Ids::Hierarchical => {
                let (head, body) = text.split_once('\n').expect("a workspace line");
                format!("{head}\n    !identifiers hierarchical\n{}", self.refs_in(body, nested))
            }
        }
    }

    /// `text` as this mode names the nested elements of `SHOP_NESTED`.
    pub fn refs(self, text: &str) -> String {
        self.refs_in(text, SHOP_NESTED)
    }

    /// `text` with every whole-word bare id of `nested` that is not
    /// followed by `=` written as this mode names it: unchanged under
    /// `Flat`, the full id under `Hierarchical`. For expected file lines
    /// (`at web 1 2`, `via web -> db 0 600`) and keys (`web`).
    pub fn refs_in(self, text: &str, nested: &[(&str, &str)]) -> String {
        if self == Ids::Flat {
            return text.to_owned();
        }
        let words: Vec<&str> = text.split(' ').collect();
        let mut out = Vec::with_capacity(words.len());
        for (i, w) in words.iter().enumerate() {
            // A word may end the line (`db\n`); match the part before it.
            let (word, rest) = w.find('\n').map_or((*w, ""), |n| w.split_at(n));
            let declared = words.get(i + 1).is_some_and(|next| *next == "=");
            match nested.iter().find(|(bare, _)| *bare == word) {
                Some((_, full)) if !declared => out.push(format!("{full}{rest}")),
                _ => out.push((*w).to_owned()),
            }
        }
        out.join(" ")
    }
}

#[test]
fn ids_rewrite_references_not_declarations() {
    let text = "workspace {\n    model {\n        web = container \"Web\"\n        web -> db \"Uses web\"\n    }\n    at web 1 2\n    via web -> db 0 600\n}\n";
    assert_eq!(Ids::Flat.workspace(text), text);
    assert_eq!(
        Ids::Hierarchical.workspace(text),
        "workspace {\n    !identifiers hierarchical\n    model {\n        web = container \"Web\"\n        shop.web -> shop.db \"Uses web\"\n    }\n    at shop.web 1 2\n    via shop.web -> shop.db 0 600\n}\n"
    );
    let parsed = c4rizer_core::model::parse(&Ids::Hierarchical.workspace(SHOP)).expect("hierarchical SHOP parses");
    assert!(parsed.model.find_written("shop.web.cart").is_some());
    assert!(parsed.model.find_written("cart").is_none(), "a bare nested id does not resolve");
}

/// A container view with `n` components on a grid, every one with an
/// `at` line, and dense relationships: slow to route in a debug build,
/// instant to place (EDIT-25 stop tests).
pub fn heavy(n: usize) -> String {
    use std::fmt::Write as _;
    let mut s = String::from("workspace {\n    model {\n        big = softwareSystem \"Big\" {\n            core = container \"Core\" {\n");
    for i in 0..n {
        let _ = writeln!(s, "                c{i} = component \"Part {i}\" \"Does part {i}.\"");
    }
    s.push_str("            }\n        }\n");
    for i in 0..n {
        for d in [1, 3, 7] {
            let j = (i + d) % n;
            let _ = writeln!(s, "        c{i} -> c{j} \"Calls\"");
        }
    }
    s.push_str("    }\n    views {\n        component core \"heavy\" \"Heavy\" {\n            include *\n");
    let cols = 6;
    for i in 0..n {
        let _ = writeln!(s, "            at c{i} {} {}", 200 + (i % cols) * 400, 200 + (i / cols) * 340);
    }
    s.push_str("        }\n    }\n}\n");
    s
}

/// Timed runs per phase 5 benchmark (SRS P5-900: at least 11).
pub const BENCH_RUNS: usize = 11;

/// True (after printing why) when this is not a release build: a timing
/// in a debug build means nothing (P5-900).
pub fn debug_build(test: &str) -> bool {
    if cfg!(debug_assertions) {
        println!("{test}: skipped, timing needs `cargo test --release`");
        return true;
    }
    false
}

/// A phase 5 benchmark (P5-900), as the core's `bench_util/bench.rs`:
/// one warm-up call, then `BENCH_RUNS` runs, each looping `body` enough
/// times to take over 1 ms and divided by that count; prints
/// `BENCH\t<id>\t<feature>\t<what>\t<input>\t<median_us>\t<min_us>\t<max_us>\t<runs>\t<budget_us>`
/// and asserts `median <= budget_us`.
// DECISION: times in microseconds with three decimals, as the core's
// helper: a snap query is well under one microsecond.
#[expect(clippy::too_many_arguments, reason = "the six fields of the BENCH line, then the body")]
pub fn bench(id: &str, feature: &str, what: &str, input: &str, budget_us: u64, mut body: impl FnMut()) {
    use std::time::{Duration, Instant};
    let t = Instant::now();
    body();
    let once = t.elapsed().max(Duration::from_nanos(1));
    let reps = u32::try_from((Duration::from_millis(1).as_nanos() / once.as_nanos()) + 1).unwrap_or(u32::MAX);
    let mut times: Vec<f64> = (0..BENCH_RUNS)
        .map(|_| {
            let t = Instant::now();
            for _ in 0..reps {
                body();
            }
            t.elapsed().as_secs_f64() * 1e6 / f64::from(reps)
        })
        .collect();
    times.sort_by(f64::total_cmp);
    let (median, min, max) = (times[BENCH_RUNS / 2], times[0], times[BENCH_RUNS - 1]);
    println!("BENCH\t{id}\t{feature}\t{what}\t{input}\t{median:.3}\t{min:.3}\t{max:.3}\t{BENCH_RUNS}\t{budget_us}");
    #[expect(clippy::cast_precision_loss, reason = "budgets are far below 2^52 us")]
    let budget = budget_us as f64;
    assert!(median <= budget, "{id}: median {median:.3} us over the budget {budget_us} us");
}

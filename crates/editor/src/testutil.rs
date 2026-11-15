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

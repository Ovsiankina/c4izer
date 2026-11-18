//! Step-4 contract tests of the network row, CLI part, end to end on
//! `env!("CARGO_BIN_EXE_c4rizer")`. No run here fetches: fixtures name
//! URLs only with the network off, and `XDG_CACHE_HOME` points into a
//! scratch folder (P3-616).

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::sync::atomic::{AtomicU32, Ordering};

use c4rizer_core::model::{NetCache, utc_text};

/// A fresh, empty directory under the system temp dir, removed on drop.
struct Scratch(PathBuf);

impl Scratch {
    fn new(name: &str) -> Scratch {
        static NEXT: AtomicU32 = AtomicU32::new(0);
        let n = NEXT.fetch_add(1, Ordering::Relaxed);
        let dir = std::env::temp_dir().join(format!("c4rizer-cli-net-{}-{n}-{name}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).expect("temp dir");
        Scratch(dir)
    }

    fn path(&self) -> &Path {
        &self.0
    }

    /// The cache the binary uses with `XDG_CACHE_HOME` set to this folder.
    fn cache(&self) -> NetCache {
        NetCache::new(self.0.join("xdg").join("c4rizer").join("net"))
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

/// Runs the binary in `dir` with its cache under `dir/xdg`.
fn c4rizer(dir: &Scratch, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_c4rizer"))
        .current_dir(dir.path())
        .env("XDG_CACHE_HOME", dir.path().join("xdg"))
        .args(args)
        .output()
        .expect("run c4rizer")
}

fn text(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes).into_owned()
}

/// P3-601: without `--network` nothing is fetched and no cache entry is
/// read, even when the cache holds the URL: an HTTPS include stays
/// `UrlNotSupported`, a URL theme gives the `NotFetched` notice.
#[test]
fn p3_601_no_cache_read_without_flag() {
    let dir = Scratch::new("601");
    let cache = dir.cache();
    cache.put("https://h.test/people.dsl", b"operator = person \"Operator\"\n", 1_700_000_000).unwrap();
    cache
        .put("https://h.test/theme.json", br##"{"elements": [{"tag": "Element", "background": "#123456"}]}"##, 1_700_000_000)
        .unwrap();
    fs::write(
        dir.path().join("inc.dsl"),
        "workspace {\n  model {\n    !include https://h.test/people.dsl\n  }\n}\n",
    )
    .unwrap();
    let out = c4rizer(&dir, &["lint", "inc.dsl"]);
    assert_eq!(out.status.code(), Some(2), "{}", text(&out.stderr));
    let err = text(&out.stderr);
    assert!(err.contains("https://h.test/people.dsl") && err.contains("reads local files only"), "{err}");
    assert!(!err.contains("fetched") && !err.contains("cached"), "{err}");
    fs::write(
        dir.path().join("theme.dsl"),
        "workspace {\n  model {\n    shop = softwareSystem \"Shop\"\n  }\n  views {\n    theme https://h.test/theme.json\n  }\n}\n",
    )
    .unwrap();
    let out = c4rizer(&dir, &["validate", "theme.dsl"]);
    assert_eq!(out.status.code(), Some(0), "{}", text(&out.stderr));
    let out = c4rizer(&dir, &["lint", "theme.dsl"]);
    let report = text(&out.stdout);
    assert!(
        report.contains("not fetched: https://h.test/theme.json — theme styles not drawn (the network is off"),
        "{report}"
    );
    assert!(text(&out.stderr).is_empty(), "{}", text(&out.stderr));
    // The cache is untouched.
    assert_eq!(cache.list().unwrap().len(), 2);
}

/// P3-612: `cache list` prints one line per cached URL with its fetch
/// time and size.
#[test]
fn p3_612_cache_list_prints_url_time_size() {
    let dir = Scratch::new("612-list");
    let out = c4rizer(&dir, &["cache", "list"]);
    assert_eq!(out.status.code(), Some(0));
    assert!(text(&out.stdout).starts_with("no cached URLs in "), "{}", text(&out.stdout));
    let cache = dir.cache();
    cache.put("https://h.test/b.dsl", b"12345", 1_700_000_000).unwrap();
    cache.put("https://h.test/a.png", b"123", 951_782_400).unwrap();
    let out = c4rizer(&dir, &["cache", "list"]);
    assert_eq!(out.status.code(), Some(0), "{}", text(&out.stderr));
    assert_eq!(
        text(&out.stdout),
        format!(
            "https://h.test/a.png  {}  3 bytes\nhttps://h.test/b.dsl  {}  5 bytes\n",
            utc_text(951_782_400),
            utc_text(1_700_000_000)
        )
    );
    let out = c4rizer(&dir, &["cache", "show"]);
    assert_eq!(out.status.code(), Some(2));
    assert!(text(&out.stderr).contains("give `list` or `clear`"), "{}", text(&out.stderr));
}

/// P3-612: `cache clear` deletes the cache folder and exits 0, also when
/// there is none.
#[test]
fn p3_612_cache_clear_deletes_folder() {
    let dir = Scratch::new("612-clear");
    let cache = dir.cache();
    cache.put("https://h.test/b.dsl", b"12345", 1_700_000_000).unwrap();
    let out = c4rizer(&dir, &["cache", "clear"]);
    assert_eq!(out.status.code(), Some(0), "{}", text(&out.stderr));
    assert!(text(&out.stdout).starts_with("removed "), "{}", text(&out.stdout));
    assert!(!cache.dir().exists());
    let out = c4rizer(&dir, &["cache", "clear"]);
    assert_eq!(out.status.code(), Some(0), "{}", text(&out.stderr));
}

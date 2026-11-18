//! Step-4 contract tests of the network row, core part (P3-600..P3-621).
//! No test opens a network connection (P3-616): fetching goes through a
//! test `Loader` or a test `Transport` serving in-memory bytes.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU32, Ordering};

use c4rizer_core::model::{CacheEntry, NetCache, fnv1a, utc_text};

/// A fresh, empty folder under the system temp dir, removed on drop.
struct Scratch(PathBuf);

impl Scratch {
    fn new(name: &str) -> Scratch {
        static NEXT: AtomicU32 = AtomicU32::new(0);
        let n = NEXT.fetch_add(1, Ordering::Relaxed);
        let dir = std::env::temp_dir().join(format!("c4rizer-net-{}-{n}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("temp dir");
        Scratch(dir)
    }

    fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// P3-609: one file per URL named by the FNV-1a hash of the URL, and an
/// index file mapping the hash to the URL and its fetch time.
#[test]
fn p3_609_cache_file_named_by_fnv1a_with_index() {
    // FNV-1a 64 reference values.
    assert_eq!(fnv1a(b""), 0xcbf2_9ce4_8422_2325);
    assert_eq!(fnv1a(b"a"), 0xaf63_dc4c_8601_ec8c);
    assert_eq!(fnv1a(b"foobar"), 0x8594_4171_f739_67e8);
    let scratch = Scratch::new("609");
    let cache = NetCache::new(scratch.path().join("net"));
    let url = "https://example.test/themes/team/theme.json";
    cache.put(url, b"{}", 1_700_000_000).expect("put");
    let name = format!("{:016x}", fnv1a(url.as_bytes()));
    let body = cache.dir().join(&name);
    assert_eq!(std::fs::read(&body).expect("body file"), b"{}");
    let index = std::fs::read_to_string(cache.dir().join("index")).expect("index");
    assert!(index.lines().any(|l| l.contains(&name) && l.contains(url) && l.contains("1700000000")), "{index}");
    assert_eq!(cache.get(url).expect("get"), Some((b"{}".to_vec(), 1_700_000_000)));
    assert_eq!(cache.get("https://example.test/other").expect("get"), None);
    // A newer copy replaces the older one.
    cache.put(url, b"{ }", 1_700_000_100).expect("put again");
    assert_eq!(cache.get(url).expect("get"), Some((b"{ }".to_vec(), 1_700_000_100)));
    assert_eq!(cache.list().expect("list").len(), 1);
    // `$XDG_CACHE_HOME/c4rizer/net/`, else `~/.cache/c4rizer/net/`.
    let env = |xdg: Option<&'static str>, home: Option<&'static str>| {
        move |name: &str| match name {
            "XDG_CACHE_HOME" => xdg.map(str::to_owned),
            "HOME" => home.map(str::to_owned),
            _ => None,
        }
    };
    let dir = |e: &dyn Fn(&str) -> Option<String>| NetCache::from_env(e).map(|c| c.dir().to_path_buf());
    assert_eq!(dir(&env(Some("/xdg"), Some("/home/u"))), Some(PathBuf::from("/xdg/c4rizer/net")));
    assert_eq!(dir(&env(None, Some("/home/u"))), Some(PathBuf::from("/home/u/.cache/c4rizer/net")));
    assert_eq!(dir(&env(Some("relative"), Some("/home/u"))), Some(PathBuf::from("/home/u/.cache/c4rizer/net")));
    assert_eq!(dir(&env(None, None)), None);
}

/// P3-612 (core half): the listing gives each URL with its fetch time
/// and size, sorted by URL; clearing deletes the folder.
#[test]
fn p3_612_cache_list_and_clear() {
    let scratch = Scratch::new("612");
    let cache = NetCache::new(scratch.path().join("net"));
    assert_eq!(cache.list().expect("missing cache lists empty"), Vec::new());
    cache.put("https://b.test/x.dsl", b"12345", 20).expect("put");
    cache.put("https://a.test/y.png", b"123", 10).expect("put");
    assert_eq!(
        cache.list().expect("list"),
        vec![
            CacheEntry { url: "https://a.test/y.png".into(), fetched: 10, size: 3 },
            CacheEntry { url: "https://b.test/x.dsl".into(), fetched: 20, size: 5 },
        ]
    );
    cache.clear().expect("clear");
    assert!(!cache.dir().exists());
    cache.clear().expect("clearing a missing cache is no error");
    assert_eq!(utc_text(0), "1970-01-01 00:00:00 UTC");
    assert_eq!(utc_text(951_782_400), "2000-02-29 00:00:00 UTC");
    assert_eq!(utc_text(1_700_000_000), "2023-11-14 22:13:20 UTC");
}

#[test]
fn p3_604_only_https_and_https_redirects() {
    todo!()
}

#[test]
fn p3_605_redirect_size_and_time_limits() {
    todo!()
}

#[test]
fn p3_610_fetched_again_each_run_memo_reused() {
    todo!()
}

#[test]
fn p3_611_failed_fetch_uses_cache_with_date() {
    todo!()
}

#[test]
fn p3_616_loader_fetch_default_is_unsupported() {
    todo!()
}

#[test]
fn p3_620_fetch_events_name_url_and_bytes() {
    todo!()
}

#[test]
fn p3_601_network_off_keeps_phase_2() {
    todo!()
}

#[test]
fn p3_603_include_and_extends_fetched() {
    todo!()
}

#[test]
fn p3_606_relative_paths_resolve_against_url() {
    todo!()
}

#[test]
fn p3_607_fetched_include_cycle_names_url() {
    todo!()
}

#[test]
fn p3_603_themes_icons_images_fetched() {
    todo!()
}

#[test]
fn p3_608_diagram_sources_never_fetched() {
    todo!()
}

#[test]
fn p3_613_not_fetched_notice_names_url_and_what() {
    todo!()
}

#[test]
fn p3_618_fetched_svg_embedded_as_data_uri() {
    todo!()
}

#[test]
fn p3_619_cached_builds_byte_identical() {
    todo!()
}

#[test]
fn p3_621_default_theme_never_fetched() {
    todo!()
}

#[cfg(feature = "network")]
#[test]
fn p3_617_https_transport_limits_configured() {
    todo!()
}

/// Real fetcher against a silent 127.0.0.1 listener: no internet, but a
/// real socket and a 10 s wait, so it is ignored (P3-616).
#[cfg(feature = "network")]
#[test]
#[ignore = "real fetcher, 127.0.0.1 only, waits TIMEOUT_SECS"]
fn net_p3_605_timeout_on_silent_loopback() {
    todo!()
}

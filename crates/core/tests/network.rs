//! Step-4 contract tests of the network row, core part (P3-600..P3-621).
//! No test opens a network connection (P3-616): fetching goes through a
//! test `Loader` or a test `Transport` serving in-memory bytes.

use std::cell::RefCell;
use std::collections::BTreeMap;
use std::io;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::atomic::{AtomicU32, Ordering};

use c4rizer_core::model::{
    CacheEntry, FetchEvent, FsLoader, Loader, MAX_BODY, NetCache, NetLoader, NoFiles, Response,
    Transport, fnv1a, utc_text,
};

/// What the test transport answers for one URL.
#[derive(Debug, Clone)]
enum Route {
    /// 200 with this body.
    Body(Vec<u8>),
    /// A redirect (status, `Location`).
    Redirect(u16, String),
    /// Another status, no body.
    Status(u16),
    /// The request fails with this kind.
    Fail(io::ErrorKind),
}

/// A `Transport` serving `routes` from memory and recording each URL
/// asked for (P3-616: no connection).
#[derive(Debug, Clone, Default)]
struct Fake {
    routes: BTreeMap<String, Route>,
    calls: Rc<RefCell<Vec<String>>>,
}

impl Fake {
    fn with(routes: &[(&str, Route)]) -> Fake {
        Fake {
            routes: routes.iter().map(|(u, r)| ((*u).to_owned(), r.clone())).collect(),
            calls: Rc::default(),
        }
    }

    fn calls(&self) -> Vec<String> {
        self.calls.borrow().clone()
    }
}

impl Transport for Fake {
    fn get(&mut self, url: &str, limit: u64) -> io::Result<Response> {
        self.calls.borrow_mut().push(url.to_owned());
        let response = |status, location, body: Vec<u8>| Response { status, location, body };
        match self.routes.get(url) {
            Some(Route::Body(b)) => {
                let keep = usize::try_from(limit + 1).unwrap_or(usize::MAX).min(b.len());
                Ok(response(200, None, b[..keep].to_vec()))
            }
            Some(Route::Redirect(status, to)) => Ok(response(*status, Some(to.clone()), Vec::new())),
            Some(Route::Status(status)) => Ok(response(*status, None, Vec::new())),
            Some(Route::Fail(kind)) => Err(io::Error::new(*kind, "test transport failure")),
            None => Ok(response(404, None, Vec::new())),
        }
    }
}

/// A `NetLoader` over `fake`, with `cache` if given.
fn net(fake: &Fake, cache: Option<NetCache>) -> NetLoader {
    NetLoader::new(Box::new(fake.clone()), cache)
}

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
    let fake = Fake::with(&[
        ("https://a.test/ok.dsl", Route::Body(b"ok".to_vec())),
        ("https://a.test/hop", Route::Redirect(302, "/ok.dsl".into())),
        ("https://a.test/down", Route::Redirect(301, "http://b.test/plain.dsl".into())),
    ]);
    let mut loader = net(&fake, None);
    for url in ["http://a.test/ok.dsl", "ftp://a.test/ok.dsl", "file:///etc/hosts"] {
        let e = loader.fetch(url).expect_err("not https");
        assert_eq!(e.kind(), io::ErrorKind::InvalidInput);
        assert!(e.to_string().contains(url), "{e}");
    }
    assert!(fake.calls().is_empty(), "nothing asked of the transport for a non-https URL");
    assert_eq!(loader.fetch("https://a.test/ok.dsl").expect("https"), b"ok");
    // A relative redirect resolves against the URL and is followed.
    assert_eq!(loader.fetch("https://a.test/hop").expect("redirect"), b"ok");
    // A redirect to a non-HTTPS URL is refused, naming both URLs.
    let e = loader.fetch("https://a.test/down").expect_err("redirect to http");
    let text = e.to_string();
    assert!(text.contains("https://a.test/down") && text.contains("http://b.test/plain.dsl"), "{text}");
    assert!(!fake.calls().iter().any(|c| c.starts_with("http://")), "{:?}", fake.calls());
}

#[test]
fn p3_605_redirect_size_and_time_limits() {
    let mut routes: Vec<(String, Route)> = (0..6)
        .map(|i| (format!("https://a.test/r{i}"), Route::Redirect(307, format!("https://a.test/r{}", i + 1))))
        .collect();
    routes.push(("https://a.test/r6".into(), Route::Body(b"end".to_vec())));
    routes.push(("https://a.test/big".into(), Route::Body(vec![b'x'; usize::try_from(MAX_BODY).unwrap() + 1])));
    routes.push(("https://a.test/full".into(), Route::Body(vec![b'x'; usize::try_from(MAX_BODY).unwrap()])));
    routes.push(("https://a.test/slow".into(), Route::Fail(io::ErrorKind::TimedOut)));
    routes.push(("https://a.test/gone".into(), Route::Status(404)));
    let refs: Vec<(&str, Route)> = routes.iter().map(|(u, r)| (u.as_str(), r.clone())).collect();
    let fake = Fake::with(&refs);
    let mut loader = net(&fake, None);
    // Five redirects are followed; a sixth is refused.
    assert_eq!(loader.fetch("https://a.test/r1").expect("5 redirects"), b"end");
    let e = loader.fetch("https://a.test/r0").expect_err("6 redirects");
    assert!(e.to_string().contains("https://a.test/r0") && e.to_string().contains("5 redirects"), "{e}");
    // 10 MiB is accepted, one byte more is refused.
    assert_eq!(loader.fetch("https://a.test/full").expect("10 MiB").len() as u64, MAX_BODY);
    let e = loader.fetch("https://a.test/big").expect_err("over 10 MiB");
    assert!(e.to_string().contains("https://a.test/big") && e.to_string().contains("10 MiB"), "{e}");
    let e = loader.fetch("https://a.test/slow").expect_err("timeout");
    assert_eq!(e.kind(), io::ErrorKind::TimedOut);
    assert!(e.to_string().contains("https://a.test/slow") && e.to_string().contains("10 s"), "{e}");
    let e = loader.fetch("https://a.test/gone").expect_err("404");
    assert!(e.to_string().contains("https://a.test/gone") && e.to_string().contains("404"), "{e}");
}

#[test]
fn p3_610_fetched_again_each_run_memo_reused() {
    let url = "https://a.test/theme.json";
    let fake = Fake::with(&[(url, Route::Body(b"{}".to_vec()))]);
    let mut first = net(&fake, None);
    first.fetch(url).expect("fetch");
    first.fetch(url).expect("same build");
    assert_eq!(fake.calls().len(), 1, "one connection per URL per build");
    let mut second = net(&fake, None);
    second.fetch(url).expect("next build");
    assert_eq!(fake.calls().len(), 2, "every build fetches again");
    // The editor's rebuild after a save reuses what the open fetched.
    let mut rebuild = net(&fake, None);
    rebuild.reuse(first.fetched());
    assert_eq!(rebuild.fetch(url).expect("reused"), b"{}");
    assert_eq!(fake.calls().len(), 2, "no connection for reused bytes");
    assert_eq!(first.fetched().get(url).map(Vec::as_slice), Some(&b"{}"[..]));
}

#[test]
fn p3_611_failed_fetch_uses_cache_with_date() {
    let scratch = Scratch::new("611");
    let cache = NetCache::new(scratch.path().join("net"));
    let (cached, fresh, missing) = ("https://a.test/c.json", "https://a.test/f.json", "https://a.test/m.json");
    cache.put(cached, b"old", 1_700_000_000).expect("seed");
    let fake = Fake::with(&[
        (cached, Route::Fail(io::ErrorKind::ConnectionRefused)),
        (fresh, Route::Body(b"new".to_vec())),
        (missing, Route::Status(500)),
    ]);
    let mut loader = net(&fake, Some(cache.clone()));
    assert_eq!(loader.fetch(cached).expect("served from cache"), b"old");
    assert_eq!(loader.fetch(fresh).expect("fetched"), b"new");
    assert!(loader.fetch(missing).is_err(), "no cached copy: the fetch fails");
    assert_eq!(cache.get(fresh).expect("stored").map(|(b, _)| b), Some(b"new".to_vec()));
    let events = loader.events();
    assert!(matches!(&events[0], FetchEvent::FromCache { url, fetched: 1_700_000_000, .. } if url == cached));
    let line = events[0].to_string();
    assert!(line.contains(cached) && line.contains("2023-11-14 22:13:20 UTC"), "{line}");
    assert!(matches!(&events[2], FetchEvent::Failed { url, .. } if url == missing));
    assert!(events[2].to_string().contains("fetch failed"), "{}", events[2]);
}

#[test]
fn p3_616_loader_fetch_default_is_unsupported() {
    /// A test loader serving bytes from memory.
    struct Memory;
    impl Loader for Memory {
        fn read(&mut self, _: &Path) -> io::Result<Vec<u8>> {
            Err(io::ErrorKind::NotFound.into())
        }
        fn read_dir(&mut self, _: &Path) -> io::Result<Vec<PathBuf>> {
            Err(io::ErrorKind::NotFound.into())
        }
        fn canonicalize(&mut self, p: &Path) -> io::Result<PathBuf> {
            Ok(p.to_path_buf())
        }
        fn env(&self, _: &str) -> Option<String> {
            None
        }
        fn fetch(&mut self, _: &str) -> io::Result<Vec<u8>> {
            Ok(b"bytes".to_vec())
        }
    }
    // A loader that does not implement `fetch` is "network off".
    for e in [NoFiles.fetch("https://a.test/x.dsl"), FsLoader.fetch("https://a.test/x.dsl")] {
        assert_eq!(e.expect_err("off").kind(), io::ErrorKind::Unsupported);
    }
    assert_eq!(Memory.fetch("https://a.test/x").expect("memory"), b"bytes");
}

#[test]
fn p3_620_fetch_events_name_url_and_bytes() {
    let fake = Fake::with(&[("https://a.test/i.png", Route::Body(b"abc".to_vec()))]);
    let mut loader = net(&fake, None);
    loader.fetch("https://a.test/i.png").expect("fetch");
    assert_eq!(
        loader.events(),
        &[FetchEvent::Fetched { url: "https://a.test/i.png".into(), bytes: 3 }]
    );
    assert_eq!(loader.events()[0].to_string(), "fetched https://a.test/i.png (3 bytes)");
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
    use c4rizer_core::model::HttpsTransport;
    // Refused before any connection: the real fetcher never speaks plain
    // HTTP, not even to 127.0.0.1.
    let mut t = HttpsTransport::new();
    let e = t.get("http://127.0.0.1:9/x.dsl", MAX_BODY).expect_err("not https");
    assert_eq!(e.kind(), io::ErrorKind::InvalidInput);
    // It plugs into `NetLoader`, which refuses the same before asking it.
    let mut loader = NetLoader::new(Box::new(HttpsTransport::default()), None);
    assert_eq!(loader.fetch("http://127.0.0.1:9/x.dsl").expect_err("off").kind(), io::ErrorKind::InvalidInput);
}

/// Real fetcher against a silent 127.0.0.1 listener: no internet, but a
/// real socket and a 10 s wait, so it is ignored (P3-616).
#[cfg(feature = "network")]
#[test]
#[ignore = "real fetcher, 127.0.0.1 only, waits TIMEOUT_SECS"]
fn net_p3_605_timeout_on_silent_loopback() {
    use c4rizer_core::model::{HttpsTransport, TIMEOUT_SECS};
    let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind loopback");
    let port = listener.local_addr().expect("addr").port();
    // Accept and say nothing: the TLS handshake never completes.
    let hold = std::thread::spawn(move || {
        let conn = listener.accept();
        std::thread::sleep(std::time::Duration::from_secs(TIMEOUT_SECS + 3));
        drop(conn);
    });
    let started = std::time::Instant::now();
    let url = format!("https://127.0.0.1:{port}/theme.json");
    let mut loader = NetLoader::new(Box::new(HttpsTransport::new()), None);
    let e = loader.fetch(&url).expect_err("silent server");
    let took = started.elapsed().as_secs();
    assert_eq!(e.kind(), io::ErrorKind::TimedOut, "{e}");
    assert!(e.to_string().contains(&url) && e.to_string().contains("10 s"), "{e}");
    assert!((TIMEOUT_SECS - 1..=TIMEOUT_SECS + 2).contains(&took), "took {took} s");
    hold.join().expect("listener thread");
}

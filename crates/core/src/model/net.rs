//! The network row (P3-600..P3-621): the fetch cache, the HTTPS fetch
//! policy (`NetLoader`) over a one-request `Transport`, and the real
//! transport behind feature `network`. Core never fetches on its own:
//! `parse_with` asks `Loader::fetch`, whose default body says the network
//! is off.

use std::collections::BTreeMap;
use std::fmt::{self, Write as _};
use std::io;
use std::path::{Path, PathBuf};

use super::{FsLoader, Loader};

/// True when `text` starts with `https://`, any case.
pub(super) fn is_https(text: &str) -> bool {
    text.get(..8).is_some_and(|p| p.eq_ignore_ascii_case("https://"))
}

/// True when `text` starts with `http://` or `https://`, any case.
#[allow(dead_code, reason = "used by the include and theme hooks (network-04, -05)")]
pub(super) fn is_http_url(text: &str) -> bool {
    is_https(text) || text.get(..7).is_some_and(|p| p.eq_ignore_ascii_case("http://"))
}

/// `reference` resolved against the absolute URL `base` (RFC 3986 §5.2,
/// enough of it for file paths): a reference with a scheme stays as it
/// is; `//host/..` takes `base`'s scheme; `/path` takes `base`'s origin;
/// anything else replaces `base`'s last path segment, `.` and `..`
/// folded (P3-606). The result is never a local path.
pub(super) fn join_url(base: &str, reference: &str) -> String {
    let has_scheme = reference
        .find(':')
        .is_some_and(|i| i > 0 && !reference[..i].contains(['/', '?', '#']));
    if has_scheme {
        return reference.to_owned();
    }
    let scheme_end = base.find("://").map_or(0, |i| i + 3);
    if let Some(rest) = reference.strip_prefix("//") {
        return format!("{}{rest}", &base[..scheme_end]);
    }
    let path_start = base[scheme_end..].find('/').map_or(base.len(), |i| scheme_end + i);
    let origin = &base[..path_start];
    let base_path = &base[path_start..];
    let base_path = &base_path[..base_path.find(['?', '#']).unwrap_or(base_path.len())];
    let joined = if reference.starts_with('/') {
        reference.to_owned()
    } else {
        let folder = &base_path[..base_path.rfind('/').map_or(0, |i| i + 1)];
        format!("{}{reference}", if folder.is_empty() { "/" } else { folder })
    };
    let (path, tail) = match joined.find(['?', '#']) {
        Some(i) => joined.split_at(i),
        None => (joined.as_str(), ""),
    };
    let mut segments: Vec<&str> = Vec::new();
    let parts: Vec<&str> = path.split('/').skip(1).collect();
    for (i, part) in parts.iter().enumerate() {
        let last = i + 1 == parts.len();
        match *part {
            "." => {
                if last {
                    segments.push("");
                }
            }
            ".." => {
                segments.pop();
                if last {
                    segments.push("");
                }
            }
            other => segments.push(other),
        }
    }
    format!("{origin}/{}{tail}", segments.join("/"))
}

/// Seconds after the Unix epoch, now.
fn now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs())
}

/// The most redirects one fetch follows (P3-605).
pub const MAX_REDIRECTS: usize = 5;
/// The largest body one fetch accepts, in bytes: 10 MiB (P3-605).
pub const MAX_BODY: u64 = 10 * 1024 * 1024;
/// The time one fetch may take, in seconds (P3-605).
pub const TIMEOUT_SECS: u64 = 10;

/// The 64-bit FNV-1a hash of `bytes`: the cache names a URL's file by it
/// (P3-609, ruling N2: no hash crate).
#[must_use]
pub fn fnv1a(bytes: &[u8]) -> u64 {
    const OFFSET: u64 = 0xcbf2_9ce4_8422_2325;
    const PRIME: u64 = 0x0000_0100_0000_01b3;
    bytes
        .iter()
        .fold(OFFSET, |hash, &b| (hash ^ u64::from(b)).wrapping_mul(PRIME))
}

/// `secs` after the Unix epoch as `YYYY-MM-DD HH:MM:SS UTC`: how cache
/// listings and notices show a fetch time (P3-611, P3-612).
#[must_use]
pub fn utc_text(secs: u64) -> String {
    let days = secs / 86_400;
    let rest = secs % 86_400;
    let (year, month, day) = civil(days);
    format!(
        "{year:04}-{month:02}-{day:02} {:02}:{:02}:{:02} UTC",
        rest / 3600,
        rest % 3600 / 60,
        rest % 60
    )
}

/// Year, month and day of `days` after 1970-01-01 (proleptic Gregorian;
/// H. Hinnant's `civil_from_days`, unsigned since days are never before
/// the epoch here).
fn civil(days: u64) -> (u64, u64, u64) {
    let z = days + 719_468;
    let era = z / 146_097;
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + u64::from(month <= 2);
    (year, month, day)
}

/// The name of the cache's index file.
const INDEX: &str = "index";

/// One cached URL (P3-612).
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct CacheEntry {
    /// The URL as fetched.
    pub url: String,
    /// When it was fetched, seconds after the Unix epoch.
    pub fetched: u64,
    /// The body's size in bytes.
    pub size: u64,
}

/// The fetch cache (P3-609): one file per URL, named by `fnv1a` of the
/// URL in hex, and an `index` file mapping each hash to its fetch time,
/// size and URL. Read only to serve a failed fetch (P3-611, ruling N1).
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct NetCache {
    dir: PathBuf,
}

impl NetCache {
    /// The cache in folder `dir` (created on the first `put`).
    #[must_use]
    pub fn new(dir: impl Into<PathBuf>) -> NetCache {
        NetCache { dir: dir.into() }
    }

    /// The cache at `$XDG_CACHE_HOME/c4rizer/net/`, else
    /// `$HOME/.cache/c4rizer/net/`, looking variables up through `env`;
    /// `None` when neither gives an absolute folder (P3-609).
    #[must_use]
    // DECISION: the XDG base-directory rule: a relative or empty
    // `$XDG_CACHE_HOME` is ignored, as the spec says.
    pub fn from_env(env: &dyn Fn(&str) -> Option<String>) -> Option<NetCache> {
        let absolute = |v: Option<String>| v.map(PathBuf::from).filter(|p| p.is_absolute());
        let base = absolute(env("XDG_CACHE_HOME"))
            .or_else(|| absolute(env("HOME")).map(|home| home.join(".cache")))?;
        Some(NetCache::new(base.join("c4rizer").join("net")))
    }

    /// The cache folder.
    #[must_use]
    pub fn dir(&self) -> &Path {
        &self.dir
    }

    /// The index: hash to entry, as stored.
    fn index(&self) -> io::Result<BTreeMap<u64, CacheEntry>> {
        let text = match std::fs::read_to_string(self.dir.join(INDEX)) {
            Ok(text) => text,
            Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(BTreeMap::new()),
            Err(e) => return Err(e),
        };
        // DECISION: one line per URL, `<hash hex>\t<fetch secs>\t<size>\t<url>`;
        // a malformed line is skipped (a URL has no tab or newline).
        let mut out = BTreeMap::new();
        for line in text.lines() {
            let mut fields = line.splitn(4, '\t');
            let (Some(hash), Some(fetched), Some(size), Some(url)) =
                (fields.next(), fields.next(), fields.next(), fields.next())
            else {
                continue;
            };
            let (Ok(hash), Ok(fetched), Ok(size)) =
                (u64::from_str_radix(hash, 16), fetched.parse(), size.parse())
            else {
                continue;
            };
            out.insert(hash, CacheEntry { url: url.to_owned(), fetched, size });
        }
        Ok(out)
    }

    /// Writes `bytes` to `name` in the folder through a temporary file,
    /// so a reader never sees half a file.
    fn write(&self, name: &str, bytes: &[u8]) -> io::Result<()> {
        let tmp = self.dir.join(format!(".{name}.{}.tmp", std::process::id()));
        std::fs::write(&tmp, bytes)?;
        std::fs::rename(&tmp, self.dir.join(name)).inspect_err(|_| {
            let _ = std::fs::remove_file(&tmp);
        })
    }

    /// Stores `bytes` as the copy of `url`, fetched at `fetched`
    /// (seconds after the Unix epoch), replacing any older copy.
    ///
    /// # Errors
    ///
    /// Any error creating the folder or writing its files.
    // DECISION: two URLs with the same FNV-1a hash share one file; the
    // later put wins and the index names its URL, so `get` of the other
    // finds no copy rather than a wrong one.
    pub fn put(&self, url: &str, bytes: &[u8], fetched: u64) -> io::Result<()> {
        if url.contains(['\t', '\n', '\r']) {
            return Ok(());
        }
        std::fs::create_dir_all(&self.dir)?;
        let hash = fnv1a(url.as_bytes());
        self.write(&format!("{hash:016x}"), bytes)?;
        let mut index = self.index()?;
        index.insert(
            hash,
            CacheEntry {
                url: url.to_owned(),
                fetched,
                size: bytes.len() as u64,
            },
        );
        let mut text = String::new();
        for (hash, entry) in &index {
            let _ = writeln!(text, "{hash:016x}\t{}\t{}\t{}", entry.fetched, entry.size, entry.url);
        }
        self.write(INDEX, text.as_bytes())
    }

    /// The cached copy of `url` and its fetch time, `None` when there is
    /// none.
    ///
    /// # Errors
    ///
    /// Any read error other than a missing cache.
    pub fn get(&self, url: &str) -> io::Result<Option<(Vec<u8>, u64)>> {
        let hash = fnv1a(url.as_bytes());
        let Some(entry) = self.index()?.remove(&hash).filter(|e| e.url == url) else {
            return Ok(None);
        };
        match std::fs::read(self.dir.join(format!("{hash:016x}"))) {
            Ok(bytes) => Ok(Some((bytes, entry.fetched))),
            Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(None),
            Err(e) => Err(e),
        }
    }

    /// Every cached URL, sorted by URL; empty for a missing cache.
    ///
    /// # Errors
    ///
    /// Any read error other than a missing cache.
    pub fn list(&self) -> io::Result<Vec<CacheEntry>> {
        let mut out: Vec<CacheEntry> = self.index()?.into_values().collect();
        out.sort();
        Ok(out)
    }

    /// Deletes the cache folder; a missing folder is no error (P3-612).
    ///
    /// # Errors
    ///
    /// Any error removing it.
    pub fn clear(&self) -> io::Result<()> {
        match std::fs::remove_dir_all(&self.dir) {
            Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(()),
            other => other,
        }
    }
}

/// The answer to one HTTP GET, redirects not followed.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Response {
    /// The HTTP status code.
    pub status: u16,
    /// The `Location` header, for a redirect.
    pub location: Option<String>,
    /// The body, at most `limit + 1` bytes of it (`Transport::get`).
    pub body: Vec<u8>,
}

/// One HTTP GET with no redirect following: what `NetLoader` fetches
/// through. `HttpsTransport` is the real one (feature `network`); tests
/// implement it with in-memory bytes (P3-616).
pub trait Transport {
    /// GETs `url` (always `https://`, `NetLoader` checks), reading at most
    /// `limit + 1` body bytes so an oversized body is seen without
    /// reading it all.
    ///
    /// # Errors
    ///
    /// No connection, a TLS failure, or `ErrorKind::TimedOut` when
    /// `TIMEOUT_SECS` ran out.
    fn get(&mut self, url: &str, limit: u64) -> io::Result<Response>;
}

/// What one fetch of a `NetLoader` did, in fetch order (P3-611, P3-620).
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum FetchEvent {
    /// A connection was made and the body read.
    Fetched {
        /// The URL asked for.
        url: String,
        /// The body's size.
        bytes: usize,
    },
    /// The fetch failed and the cached copy was used.
    FromCache {
        /// The URL asked for.
        url: String,
        /// The cached copy's fetch time, seconds after the Unix epoch.
        fetched: u64,
        /// Why the fetch failed.
        reason: String,
    },
    /// The fetch failed and there is no cached copy.
    Failed {
        /// The URL asked for.
        url: String,
        /// Why the fetch failed.
        reason: String,
    },
}

impl fmt::Display for FetchEvent {
    /// `fetched <url> (<n> bytes)` (P3-620); `<url>: fetch failed
    /// (<reason>) — using the cached copy fetched <date>` (P3-611); `<url>:
    /// fetch failed (<reason>) and no cached copy exists`.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            FetchEvent::Fetched { url, bytes } => write!(f, "fetched {url} ({bytes} bytes)"),
            FetchEvent::FromCache { url, fetched, reason } => write!(
                f,
                "{url}: fetch failed ({reason}) — using the cached copy fetched {}",
                utc_text(*fetched)
            ),
            FetchEvent::Failed { url, reason } => {
                write!(f, "{url}: fetch failed ({reason}) and no cached copy exists")
            }
        }
    }
}

/// `FsLoader` for files plus HTTPS fetching (P3-603..P3-611): only
/// `https://` URLs and redirects, at most `MAX_REDIRECTS` redirects and
/// `MAX_BODY` bytes, every success stored in the cache, the cached copy
/// used when a fetch fails. A URL is fetched once per loader; a loader
/// is one build (P3-610).
pub struct NetLoader {
    transport: Box<dyn Transport>,
    cache: Option<NetCache>,
    memo: BTreeMap<String, Vec<u8>>,
    events: Vec<FetchEvent>,
}

impl fmt::Debug for NetLoader {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("NetLoader")
            .field("cache", &self.cache)
            .field("fetched", &self.memo.keys().collect::<Vec<_>>())
            .field("events", &self.events)
            .finish_non_exhaustive()
    }
}

impl NetLoader {
    /// Fetches through `transport`, storing in and falling back to
    /// `cache` (none: no cache).
    #[must_use]
    pub fn new(transport: Box<dyn Transport>, cache: Option<NetCache>) -> NetLoader {
        NetLoader {
            transport,
            cache,
            memo: BTreeMap::new(),
            events: Vec::new(),
        }
    }

    /// The bytes at `url` over the transport: redirects followed up to
    /// `MAX_REDIRECTS`, each one HTTPS, a 2xx body up to `MAX_BODY`.
    /// Errors name the URL and the limit.
    fn download(&mut self, url: &str) -> io::Result<Vec<u8>> {
        let mut current = url.to_owned();
        let mut hops = 0;
        loop {
            let response = self.transport.get(&current, MAX_BODY).map_err(|e| {
                if e.kind() == io::ErrorKind::TimedOut {
                    io::Error::new(e.kind(), format!("`{current}` timed out after {TIMEOUT_SECS} s"))
                } else {
                    io::Error::new(e.kind(), format!("`{current}`: {e}"))
                }
            })?;
            match response.status {
                200..=299 => {
                    if response.body.len() as u64 > MAX_BODY {
                        return Err(io::Error::other(format!(
                            "`{current}` is larger than 10 MiB, the most c4rizer fetches"
                        )));
                    }
                    return Ok(response.body);
                }
                301 | 302 | 303 | 307 | 308 => {
                    let Some(location) = response.location else {
                        return Err(io::Error::other(format!(
                            "`{current}` redirects (HTTP {}) without a Location",
                            response.status
                        )));
                    };
                    let next = join_url(&current, location.trim());
                    if !is_https(&next) {
                        return Err(io::Error::other(format!(
                            "the redirect from `{current}` to `{next}` is refused: only https:// is fetched"
                        )));
                    }
                    if hops == MAX_REDIRECTS {
                        return Err(io::Error::other(format!(
                            "`{url}` redirects more than {MAX_REDIRECTS} times, the most c4rizer follows (stopped at `{next}`; limit: {MAX_REDIRECTS} redirects)"
                        )));
                    }
                    hops += 1;
                    current = next;
                }
                status => {
                    return Err(io::Error::other(format!("`{current}` answered HTTP status {status}")));
                }
            }
        }
    }

    /// Serves these bytes for their URLs without a connection: the
    /// editor's rebuild after a save reuses what the open fetched
    /// (P3-610).
    pub fn reuse(&mut self, fetched: &BTreeMap<String, Vec<u8>>) {
        self.memo.extend(fetched.iter().map(|(u, b)| (u.clone(), b.clone())));
    }

    /// Every URL this loader served, with its bytes.
    #[must_use]
    pub fn fetched(&self) -> &BTreeMap<String, Vec<u8>> {
        &self.memo
    }

    /// What each fetch did, in order (P3-620).
    #[must_use]
    pub fn events(&self) -> &[FetchEvent] {
        &self.events
    }
}

impl Loader for NetLoader {
    fn read(&mut self, path: &Path) -> io::Result<Vec<u8>> {
        FsLoader.read(path)
    }

    fn read_dir(&mut self, path: &Path) -> io::Result<Vec<PathBuf>> {
        FsLoader.read_dir(path)
    }

    fn canonicalize(&mut self, path: &Path) -> io::Result<PathBuf> {
        FsLoader.canonicalize(path)
    }

    fn env(&self, name: &str) -> Option<String> {
        FsLoader.env(name)
    }

    /// The bytes at `url` (see the type).
    ///
    /// # Errors
    ///
    /// `ErrorKind::InvalidInput` for a URL that is not `https://`; any
    /// other failure, with the URL and the limit in the message, when no
    /// cached copy exists.
    // DECISION: a failed fetch of any kind (no connection, timeout,
    // non-2xx, a limit, a refused redirect) falls back to the cache; only
    // a non-HTTPS URL itself is refused before anything else.
    // DECISION: a cache write failure is not a fetch failure: the bytes
    // are served and the next failed fetch finds no (or an older) copy.
    fn fetch(&mut self, url: &str) -> io::Result<Vec<u8>> {
        if !is_https(url) {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                format!("`{url}` is not an https:// URL — c4rizer fetches https:// only"),
            ));
        }
        if let Some(bytes) = self.memo.get(url) {
            return Ok(bytes.clone());
        }
        match self.download(url) {
            Ok(bytes) => {
                if let Some(cache) = &self.cache {
                    let _ = cache.put(url, &bytes, now());
                }
                self.events.push(FetchEvent::Fetched {
                    url: url.to_owned(),
                    bytes: bytes.len(),
                });
                self.memo.insert(url.to_owned(), bytes.clone());
                Ok(bytes)
            }
            Err(e) => {
                let reason = e.to_string();
                let cached = self.cache.as_ref().and_then(|c| c.get(url).ok().flatten());
                if let Some((bytes, fetched)) = cached {
                    self.events.push(FetchEvent::FromCache {
                        url: url.to_owned(),
                        fetched,
                        reason,
                    });
                    self.memo.insert(url.to_owned(), bytes.clone());
                    return Ok(bytes);
                }
                self.events.push(FetchEvent::Failed {
                    url: url.to_owned(),
                    reason,
                });
                Err(e)
            }
        }
    }
}

/// The real `Transport` (P3-617): ureq with rustls, blocking, HTTPS only,
/// `TIMEOUT_SECS` for the whole request, redirects returned, not
/// followed.
#[cfg(feature = "network")]
pub struct HttpsTransport {
    agent: ureq::Agent,
}

#[cfg(feature = "network")]
impl fmt::Debug for HttpsTransport {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let config = self.agent.config();
        f.debug_struct("HttpsTransport")
            .field("https_only", &config.https_only())
            .field("max_redirects", &config.max_redirects())
            .field("timeout", &config.timeouts().global)
            .finish()
    }
}

#[cfg(feature = "network")]
impl HttpsTransport {
    /// A transport with the P3-605 limits.
    #[must_use]
    // DECISION: ureq follows no redirect (`max_redirects(0)` returns the
    // 3xx response), so `NetLoader` checks every hop for https:// and
    // counts them; non-2xx statuses come back as responses, not errors.
    pub fn new() -> HttpsTransport {
        let config = ureq::Agent::config_builder()
            .https_only(true)
            .max_redirects(0)
            .http_status_as_error(false)
            .timeout_global(Some(std::time::Duration::from_secs(TIMEOUT_SECS)))
            .user_agent(concat!("c4rizer/", env!("CARGO_PKG_VERSION")))
            .build();
        HttpsTransport {
            agent: ureq::Agent::new_with_config(config),
        }
    }
}

#[cfg(feature = "network")]
impl Default for HttpsTransport {
    fn default() -> Self {
        HttpsTransport::new()
    }
}

#[cfg(feature = "network")]
impl Transport for HttpsTransport {
    fn get(&mut self, url: &str, limit: u64) -> io::Result<Response> {
        use std::io::Read as _;
        if !is_https(url) {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                format!("`{url}` is not an https:// URL"),
            ));
        }
        let mut response = self.agent.get(url).call().map_err(ureq_error)?;
        let status = response.status().as_u16();
        let location = response
            .headers()
            .get("location")
            .and_then(|v| v.to_str().ok())
            .map(str::to_owned);
        let mut body = Vec::new();
        response
            .body_mut()
            .as_reader()
            .take(limit.saturating_add(1))
            .read_to_end(&mut body)
            .map_err(|e| match e.get_ref().and_then(|i| i.downcast_ref::<ureq::Error>()) {
                Some(ureq::Error::Timeout(_)) => io::Error::new(io::ErrorKind::TimedOut, e.to_string()),
                _ => e,
            })?;
        Ok(Response { status, location, body })
    }
}

/// A ureq error as an `io::Error`, a timeout as `ErrorKind::TimedOut`.
#[cfg(feature = "network")]
fn ureq_error(e: ureq::Error) -> io::Error {
    match e {
        ureq::Error::Timeout(_) => io::Error::new(io::ErrorKind::TimedOut, e.to_string()),
        ureq::Error::Io(io) => io,
        other => io::Error::other(other.to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::join_url;

    /// P3-605, P3-617: the real transport is HTTPS only, follows no
    /// redirect itself and stops after `TIMEOUT_SECS`.
    #[cfg(feature = "network")]
    #[test]
    fn https_transport_config() {
        let t = super::HttpsTransport::new();
        let config = t.agent.config();
        assert!(config.https_only());
        assert_eq!(config.max_redirects(), 0);
        assert!(!config.http_status_as_error());
        assert_eq!(config.timeouts().global, Some(std::time::Duration::from_secs(super::TIMEOUT_SECS)));
    }

    /// P3-606: relative references resolve against the fetching URL.
    #[test]
    fn join_url_resolves_like_a_browser() {
        let base = "https://h.test/a/b/c.dsl?x=1";
        assert_eq!(join_url(base, "d.dsl"), "https://h.test/a/b/d.dsl");
        assert_eq!(join_url(base, "../d.dsl"), "https://h.test/a/d.dsl");
        assert_eq!(join_url(base, "./e/f.png"), "https://h.test/a/b/e/f.png");
        assert_eq!(join_url(base, "../../../../up.dsl"), "https://h.test/up.dsl");
        assert_eq!(join_url(base, "/root.dsl"), "https://h.test/root.dsl");
        assert_eq!(join_url(base, "//cdn.test/x.png"), "https://cdn.test/x.png");
        assert_eq!(join_url(base, "https://o.test/y"), "https://o.test/y");
        assert_eq!(join_url(base, "http://o.test/y"), "http://o.test/y");
        assert_eq!(join_url("https://h.test", "x.dsl"), "https://h.test/x.dsl");
        assert_eq!(join_url("https://h.test:8443/t/theme.json", "icons/i.svg"), "https://h.test:8443/t/icons/i.svg");
    }
}

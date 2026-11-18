//! The network row, CLI part (P3-600..P3-620): the `--network` flag, the
//! fetching loader handed to `run_with`, the `fetched` lines and cache
//! notices on stderr, and `c4rizer cache list|clear`.
//!
//! Depends on: `c4rizer-core` (`model::NetLoader`, `model::NetCache`).

use std::ffi::OsString;
use std::io::{self, Write};

use c4rizer_core::model::{FetchEvent, NetCache, Transport, utc_text};

use crate::{CliError, Exit, UsageError};

/// The subcommands that take `--network` (P3-600). `export`, `merge` and
/// `generate` are not in this CLI yet (row exporters).
pub(crate) const NETWORK_COMMANDS: [&str; 5] = ["render", "lint", "validate", "inspect", "list"];

/// `c4rizer cache list|clear` (P3-612).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) enum CacheAction {
    /// One line per cached URL with its fetch time and size.
    List,
    /// Delete the cache folder.
    Clear,
}

/// The network setting of one run: off (no transport, no cache read,
/// P3-601), or on with a transport and the cache.
pub(crate) struct Net {
    /// The transport fetches go through; `None`: network off.
    pub(crate) transport: Option<Box<dyn Transport>>,
    /// The fetch cache; `None` when no cache folder could be found.
    pub(crate) cache: Option<NetCache>,
}

impl Net {
    /// Network off.
    pub(crate) fn off() -> Net {
        Net {
            transport: None,
            cache: None,
        }
    }

    /// Network on: the real transport and the user's cache (P3-617,
    /// P3-609). Without the `network` feature `take_network_flag` refuses
    /// the flag first, so this is never reached; it is then off.
    pub(crate) fn on() -> Net {
        #[cfg(feature = "network")]
        {
            Net {
                transport: Some(Box::new(c4rizer_core::model::HttpsTransport::new())),
                cache: user_cache(),
            }
        }
        #[cfg(not(feature = "network"))]
        {
            Net::off()
        }
    }
}

/// The user's fetch cache, from `$XDG_CACHE_HOME` or `$HOME` (P3-609).
pub(crate) fn user_cache() -> Option<NetCache> {
    NetCache::from_env(&|name| std::env::var(name).ok())
}

/// Removes `--network` from the arguments of a subcommand that takes it
/// and says whether it was there. For any other subcommand the arguments
/// come back unchanged, so `--network` stays an unknown flag.
///
/// # Errors
///
/// `Repeated("--network")` when given twice; `NoNetworkSupport` when this
/// build has no `network` feature and the flag is given (P3-617).
// DECISION: `--network` may stand anywhere after the subcommand, like
// the other flags; it takes no value.
pub(crate) fn take_network_flag(args: Vec<OsString>, supported: bool) -> Result<(Vec<OsString>, bool), UsageError> {
    let takes = args
        .first()
        .and_then(|a| a.to_str())
        .is_some_and(|c| NETWORK_COMMANDS.contains(&c));
    if !takes {
        return Ok((args, false));
    }
    let count = args.iter().filter(|a| *a == "--network").count();
    if count > 1 {
        return Err(UsageError::Repeated("--network"));
    }
    if count == 1 && !supported {
        return Err(UsageError::NoNetworkSupport);
    }
    let rest = args.into_iter().filter(|a| a != "--network").collect();
    Ok((rest, count == 1))
}

/// The `cache` subcommand's argument: `list` or `clear`.
///
/// # Errors
///
/// A usage error for anything else.
pub(crate) fn parse_cache(mut args: impl Iterator<Item = OsString>) -> Result<CacheAction, UsageError> {
    let lossy = |a: &OsString| a.to_string_lossy().into_owned();
    let action = match args.next() {
        Some(a) if a == "list" => CacheAction::List,
        Some(a) if a == "clear" => CacheAction::Clear,
        Some(a) => return Err(UsageError::CacheAction(lossy(&a))),
        None => return Err(UsageError::CacheAction(String::new())),
    };
    match args.next() {
        None => Ok(action),
        Some(extra) => Err(UsageError::ExtraArgument(lossy(&extra))),
    }
}

/// Runs `cache list` or `cache clear` on `cache` (P3-612).
///
/// # Errors
///
/// `CliError::NoCache` without a cache folder, `CliError::Cache` on a
/// read or remove failure, `CliError::Report` on a write failure.
// DECISION: `cache list` prints `<url>  <fetch time>  <n> bytes` per URL,
// sorted by URL, and `no cached URLs in <folder>` for an empty cache;
// `cache clear` prints `removed <folder>`. Structurizr has no cache.
pub(crate) fn run_cache(action: CacheAction, cache: Option<&NetCache>, stdout: &mut impl Write) -> Result<Exit, CliError> {
    let cache = cache.ok_or(CliError::NoCache)?;
    let failed = |source| CliError::Cache {
        path: cache.dir().to_path_buf(),
        source,
    };
    match action {
        CacheAction::List => {
            let entries = cache.list().map_err(failed)?;
            if entries.is_empty() {
                writeln!(stdout, "no cached URLs in {}", cache.dir().display()).map_err(CliError::Report)?;
            }
            for e in &entries {
                writeln!(stdout, "{}  {}  {} bytes", e.url, utc_text(e.fetched), e.size).map_err(CliError::Report)?;
            }
        }
        CacheAction::Clear => {
            cache.clear().map_err(failed)?;
            writeln!(stdout, "removed {}", cache.dir().display()).map_err(CliError::Report)?;
        }
    }
    stdout.flush().map_err(CliError::Report)?;
    Ok(Exit::Clean)
}

/// Writes each fetch event as one line on `stderr`: `fetched <url> (<n>
/// bytes)` (P3-620), else `notice: <event>` (P3-611).
///
/// # Errors
///
/// The first error of `stderr`.
pub(crate) fn print_events(events: &[FetchEvent], stderr: &mut impl Write) -> io::Result<()> {
    for event in events {
        match event {
            FetchEvent::Fetched { .. } => writeln!(stderr, "{event}")?,
            _ => writeln!(stderr, "notice: {event}")?,
        }
    }
    stderr.flush()
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;
    use std::ffi::OsString;
    use std::io;
    use std::path::PathBuf;

    use c4rizer_core::model::{NetCache, Response, Transport};

    use super::{Net, take_network_flag};
    use crate::tests::Scratch;
    use crate::{Command, Exit, Input, UsageError, parse_args, parse_args_net, run_with};

    fn args(list: &[&str]) -> Vec<OsString> {
        list.iter().map(OsString::from).collect()
    }

    /// A `Transport` serving bytes from memory; any other URL fails as
    /// if the connection were refused (P3-616).
    #[derive(Debug, Clone, Default)]
    struct Memory(BTreeMap<String, Vec<u8>>);

    impl Transport for Memory {
        fn get(&mut self, url: &str, _limit: u64) -> io::Result<Response> {
            match self.0.get(url) {
                Some(body) => Ok(Response { status: 200, location: None, body: body.clone() }),
                None => Err(io::ErrorKind::ConnectionRefused.into()),
            }
        }
    }

    fn memory(urls: &[(&str, &str)]) -> Memory {
        Memory(urls.iter().map(|(u, b)| ((*u).to_owned(), b.as_bytes().to_vec())).collect())
    }

    /// Runs `command` with the network on through `transport`; gives the
    /// result, stdout and stderr.
    fn go(command: &Command, transport: Memory, cache: Option<NetCache>) -> (Result<Exit, crate::CliError>, String, String) {
        let (mut out, mut err) = (Vec::new(), Vec::new());
        let net = Net { transport: Some(Box::new(transport)), cache };
        let result = run_with(command, net, &mut io::empty(), &mut out, &mut err);
        (result, String::from_utf8(out).unwrap(), String::from_utf8(err).unwrap())
    }

    const THEMED: &str = "workspace {\n  model {\n    shop = softwareSystem \"Shop\"\n  }\n  views {\n    systemContext shop \"ctx\" {\n      include *\n      autoLayout\n    }\n    theme https://h.test/theme.json\n  }\n}\n";
    const THEME: &str = "{\"elements\": [{\"tag\": \"Software System\", \"background\": \"#123456\"}]}";

    /// P3-600: `--network` on `render`, `lint`, `validate`, `inspect` and
    /// `list`, anywhere after the subcommand, once; any other subcommand
    /// keeps it an unknown flag.
    #[test]
    fn p3_600_network_flag_on_model_commands() {
        for (line, rest) in [
            (&["render", "--network", "a.dsl", "-o", "out"][..], &["render", "a.dsl", "-o", "out"][..]),
            (&["lint", "a.dsl", "--network"], &["lint", "a.dsl"]),
            (&["validate", "--network", "-workspace", "a.dsl"], &["validate", "-workspace", "a.dsl"]),
            (&["inspect", "-workspace", "a.dsl", "--network"], &["inspect", "-workspace", "a.dsl"]),
            (&["list", "--network", "a.dsl"], &["list", "a.dsl"]),
        ] {
            assert_eq!(take_network_flag(args(line), true), Ok((args(rest), true)), "{line:?}");
            assert!(parse_args_net(args(rest), true).is_ok(), "{rest:?}");
        }
        assert_eq!(take_network_flag(args(&["lint", "a.dsl"]), true), Ok((args(&["lint", "a.dsl"]), false)));
        assert_eq!(
            take_network_flag(args(&["lint", "--network", "a.dsl", "--network"]), true),
            Err(UsageError::Repeated("--network"))
        );
        for line in [&["import-layout", "a.json", "a.dsl", "--network"][..], &["cache", "list", "--network"], &["version", "--network"]] {
            let (rest, on) = take_network_flag(args(line), true).unwrap();
            assert!(!on);
            assert!(parse_args(rest).is_err(), "{line:?}: --network is not taken here");
        }
    }

    /// P3-603 (amending P2-603): `-workspace <https url>` needs
    /// `--network`; with it the workspace and its relative includes are
    /// fetched.
    #[test]
    fn p3_603_workspace_url_needs_network() {
        let url = "https://h.test/ws/w.dsl";
        assert_eq!(
            parse_args_net(args(&["validate", "-workspace", url]), false),
            Err(UsageError::WorkspaceUrl(url.into()))
        );
        let command = parse_args_net(args(&["list", "-workspace", url]), true).unwrap();
        assert_eq!(command, Command::List { input: Input::Path(PathBuf::from(url)) });
        let transport = memory(&[
            (url, "workspace {\n  model {\n    !include people.dsl\n  }\n}\n"),
            ("https://h.test/ws/people.dsl", "operator = person \"Operator\"\n"),
        ]);
        let (result, out, err) = go(&command, transport, None);
        assert_eq!(result.unwrap(), Exit::Clean);
        assert!(out.contains("Operator"), "{out}");
        assert!(err.contains(&format!("fetched {url} (")), "{err}");
        assert!(err.contains("fetched https://h.test/ws/people.dsl ("), "{err}");
    }

    /// P3-617: a build without the `network` feature refuses `--network`
    /// as a usage error naming the missing support.
    #[test]
    fn p3_617_no_network_build_refuses_flag() {
        let line = args(&["render", "a.dsl", "-o", "out", "--network"]);
        let e = take_network_flag(line.clone(), false).unwrap_err();
        assert_eq!(e, UsageError::NoNetworkSupport);
        assert!(e.to_string().contains("network support"), "{e}");
        assert_eq!(take_network_flag(line, cfg!(feature = "network")).is_ok(), cfg!(feature = "network"));
    }

    /// P3-620, P3-611: one `fetched <url> (<n> bytes)` line per
    /// connection on stderr; a failed fetch served from the cache gives a
    /// notice naming the URL and the copy's fetch date.
    #[test]
    fn p3_620_cli_prints_fetched_lines() {
        let dir = Scratch::new("p3-620");
        let file = dir.file("ws.dsl", THEMED);
        let lint = Command::Lint { input: Input::Path(file) };
        let (result, out, err) = go(&lint, memory(&[("https://h.test/theme.json", THEME)]), None);
        result.expect("lint runs");
        assert_eq!(err, format!("fetched https://h.test/theme.json ({} bytes)\n", THEME.len()));
        assert!(!out.contains("not fetched"), "{out}");
        let cache = NetCache::new(dir.path().join("cache"));
        cache.put("https://h.test/theme.json", THEME.as_bytes(), 1_700_000_000).unwrap();
        let (result, out, err) = go(&lint, memory(&[]), Some(cache));
        result.expect("lint runs");
        assert!(err.starts_with("notice: https://h.test/theme.json: fetch failed ("), "{err}");
        assert!(err.contains("using the cached copy fetched 2023-11-14 22:13:20 UTC"), "{err}");
        assert!(!out.contains("not fetched"), "{out}");
        // No copy: the theme is not drawn, and both outputs say so.
        let (result, out, err) = go(&lint, memory(&[]), None);
        result.expect("lint runs");
        assert!(err.contains("fetch failed") && err.contains("no cached copy"), "{err}");
        assert!(out.contains("not fetched: https://h.test/theme.json — theme styles not drawn"), "{out}");
    }
}

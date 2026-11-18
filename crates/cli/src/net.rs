//! The network row, CLI part (P3-600..P3-620): the `--network` flag, the
//! fetching loader handed to `run_with`, the `fetched` lines and cache
//! notices on stderr, and `c4rizer cache list|clear`.
//!
//! Depends on: `c4rizer-core` (`model::NetLoader`, `model::NetCache`).

use std::ffi::OsString;
use std::io::{self, Write};

use c4rizer_core::model::{FetchEvent, NetCache, Transport};

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
        todo!()
    }
}

/// Removes `--network` from the arguments of a subcommand that takes it
/// and says whether it was there. For any other subcommand the arguments
/// come back unchanged, so `--network` stays an unknown flag.
///
/// # Errors
///
/// `Repeated("--network")` when given twice; `NoNetworkSupport` when this
/// build has no `network` feature and the flag is given (P3-617).
pub(crate) fn take_network_flag(args: Vec<OsString>, supported: bool) -> Result<(Vec<OsString>, bool), UsageError> {
    let _ = (args, supported);
    todo!()
}

/// The `cache` subcommand's argument: `list` or `clear`.
///
/// # Errors
///
/// A usage error for anything else.
pub(crate) fn parse_cache(args: impl Iterator<Item = OsString>) -> Result<CacheAction, UsageError> {
    let _ = args;
    todo!()
}

/// Runs `cache list` or `cache clear` on `cache` (P3-612).
///
/// # Errors
///
/// `CliError::NoCache` without a cache folder, `CliError::Cache` on a
/// read or remove failure, `CliError::Report` on a write failure.
pub(crate) fn run_cache(action: CacheAction, cache: Option<&NetCache>, stdout: &mut impl Write) -> Result<Exit, CliError> {
    let _ = (action, cache, stdout);
    todo!()
}

/// Writes each fetch event as one line on `stderr`: `fetched <url> (<n>
/// bytes)` (P3-620), else `notice: <event>` (P3-611).
///
/// # Errors
///
/// The first error of `stderr`.
pub(crate) fn print_events(events: &[FetchEvent], stderr: &mut impl Write) -> io::Result<()> {
    let _ = (events, stderr);
    todo!()
}

#[cfg(test)]
mod tests {
    #[test]
    fn p3_600_network_flag_on_model_commands() {
        todo!()
    }

    #[test]
    fn p3_603_workspace_url_needs_network() {
        todo!()
    }

    #[test]
    fn p3_617_no_network_build_refuses_flag() {
        todo!()
    }

    #[test]
    fn p3_620_cli_prints_fetched_lines() {
        todo!()
    }
}

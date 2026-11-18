//! Step-4 contract tests of the network row, core part (P3-600..P3-621).
//! No test opens a network connection (P3-616): fetching goes through a
//! test `Loader` or a test `Transport` serving in-memory bytes.

#[test]
fn p3_609_cache_file_named_by_fnv1a_with_index() {
    todo!()
}

#[test]
fn p3_612_cache_list_and_clear() {
    todo!()
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

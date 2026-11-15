//! EDIT-14 smoke tests: launch the editor binary with `--startup-report`
//! and read the ms it prints (HLD §8: one smoke test launches the
//! binary). All need a display, so none is named `perf_*`: the release
//! perf oracle (`--ignored perf_`) runs headless. The QA startup script
//! runs them with `--ignored startup_` on a machine with a display.

// TODO(srs): EDIT-14 budgets (500 ms with `example/`, 300 ms with no
// file) are marked "confirm once the window backend is measured" in the
// SRS; the perf tests pin them as written.

use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::sync::Mutex;
use std::time::{Duration, Instant};

/// How long a launch may take before the test gives up and kills it.
const LAUNCH_LIMIT: Duration = Duration::from_secs(30);

/// Held around each launch so the startup tests never open windows at
/// the same time.
// DECISION: a static lock instead of asking for `--test-threads=1`: the
// QA command runs with default parallel threads, and three windows
// starting together slow each other past the EDIT-14 budgets, which
// measures the test harness rather than the editor.
static ONE_AT_A_TIME: Mutex<()> = Mutex::new(());

/// `tests/data/example.dsl` at the workspace root.
fn example() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../tests/data/example.dsl")
}

/// Runs the editor with `--startup-report` and `args`, with its per-user
/// state in a fresh directory; returns the ms it printed. Panics with
/// stderr when it fails or prints no number.
fn startup_ms(tag: &str, args: &[PathBuf]) -> u64 {
    // A panicking test poisons the lock; the next launch is still valid.
    let _one = ONE_AT_A_TIME.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
    let config = std::env::temp_dir().join(format!("c4rizer-startup-{}-{tag}", std::process::id()));
    let _ = std::fs::remove_dir_all(&config);
    std::fs::create_dir_all(&config).expect("config dir");
    let mut child = Command::new(env!("CARGO_BIN_EXE_c4rizer-editor"))
        .arg("--startup-report")
        .args(args)
        .env("XDG_CONFIG_HOME", &config)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("launch the editor");
    let began = Instant::now();
    loop {
        if child.try_wait().expect("poll the editor").is_some() {
            break;
        }
        if began.elapsed() > LAUNCH_LIMIT {
            let _ = child.kill();
            panic!("the editor did not exit within {LAUNCH_LIMIT:?}");
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    let out = child.wait_with_output().expect("editor output");
    let _ = std::fs::remove_dir_all(&config);
    let stdout = String::from_utf8_lossy(&out.stdout);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(out.status.success(), "exit {:?}; stderr: {stderr}", out.status);
    let lines: Vec<&str> = stdout.lines().filter(|l| !l.trim().is_empty()).collect();
    assert_eq!(lines.len(), 1, "one line expected; stdout: {stdout}; stderr: {stderr}");
    let ms = lines[0]
        .trim()
        .parse::<u64>()
        .unwrap_or_else(|_| panic!("not whole ms: {:?}", lines[0]));
    eprintln!("{tag}: {ms} ms");
    ms
}

/// EDIT-14: `c4rizer-editor --startup-report` prints one line of whole ms
/// and exits 0.
#[test]
#[ignore = "needs a display; run by the QA startup script"]
fn startup_edit_14_report_prints_ms_and_exits() {
    let ms = startup_ms("report", &[]);
    assert!(ms < 60_000, "{ms} ms");
}

/// EDIT-14 (plan editor-02 `edit_14_first_view_under_500ms`): with
/// `tests/data/example.dsl` the first frame shows the first view in under
/// 500 ms, release build.
#[test]
#[ignore = "needs a display; release build, run by the QA startup script"]
fn startup_edit_14_first_view_under_500ms() {
    let ms = startup_ms("example", &[example()]);
    assert!(ms < 500, "first view after {ms} ms");
}

/// EDIT-14: with no file the start screen shows in under 300 ms, release
/// build.
#[test]
#[ignore = "needs a display; release build, run by the QA startup script"]
fn startup_edit_14_no_file_under_300ms() {
    let ms = startup_ms("empty", &[]);
    assert!(ms < 300, "start screen after {ms} ms");
}

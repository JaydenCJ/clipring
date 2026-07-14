//! End-to-end tests against the compiled `clipring` binary.
//!
//! Each test gets its own temp state directory (cleaned up on drop) passed
//! via `--state`, runs the real binary through `CARGO_BIN_EXE_clipring`,
//! and asserts on stdout/stderr bytes and exit codes. There is no terminal
//! in a test run, so OSC 52 emission falls back to stdout — which is exactly
//! what lets these tests capture and verify the sequences. Everything is
//! offline and deterministic.

use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};
use std::sync::atomic::{AtomicU32, Ordering};

const BIN: &str = env!("CARGO_BIN_EXE_clipring");

static COUNTER: AtomicU32 = AtomicU32::new(0);

/// A per-test scratch state directory, removed on drop.
struct State(PathBuf);

impl State {
    fn new() -> State {
        let n = COUNTER.fetch_add(1, Ordering::SeqCst);
        let dir = std::env::temp_dir().join(format!("clipring-test-{}-{n}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        State(dir)
    }

    fn path(&self) -> &Path {
        &self.0
    }

    fn arg(&self) -> String {
        self.0.to_string_lossy().into_owned()
    }
}

impl Drop for State {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// Run clipring with `args`, optionally feeding bytes on stdin.
fn clipring(state: &State, args: &[&str], stdin: Option<&[u8]>) -> Output {
    use std::io::Write;
    let mut cmd = Command::new(BIN);
    cmd.arg("--state")
        .arg(state.arg())
        .args(args)
        .env_remove("CLIPRING_CAPACITY")
        .env_remove("CLIPRING_LIMIT")
        .env_remove("CLIPRING_WRAP")
        .env_remove("TMUX")
        .env("TERM", "xterm-256color")
        .stdin(if stdin.is_some() {
            Stdio::piped()
        } else {
            Stdio::null()
        })
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut child = cmd.spawn().expect("spawn clipring");
    if let Some(bytes) = stdin {
        child.stdin.take().unwrap().write_all(bytes).unwrap();
    }
    child.wait_with_output().expect("wait for clipring")
}

fn stdout_str(out: &Output) -> String {
    String::from_utf8_lossy(&out.stdout).into_owned()
}

fn stderr_str(out: &Output) -> String {
    String::from_utf8_lossy(&out.stderr).into_owned()
}

fn ok(out: &Output) {
    assert!(
        out.status.success(),
        "expected success, got {:?}\nstderr: {}",
        out.status.code(),
        stderr_str(out)
    );
}

// ------------------------------------------------------------- lifecycle

#[test]
fn version_prints_crate_version() {
    let state = State::new();
    let out = clipring(&state, &["--version"], None);
    ok(&out);
    assert_eq!(
        stdout_str(&out),
        format!("clipring {}\n", env!("CARGO_PKG_VERSION"))
    );
}

#[test]
fn help_lists_commands_and_unknown_commands_exit_2() {
    let state = State::new();
    let out = clipring(&state, &["--help"], None);
    ok(&out);
    let text = stdout_str(&out);
    assert!(text.contains("COMMANDS:"));
    assert!(text.contains("ENVIRONMENT:"));
    let out = clipring(&state, &["frobnicate"], None);
    assert_eq!(out.status.code(), Some(2));
    assert!(stderr_str(&out).contains("unknown command"));
}

// ------------------------------------------------------------ copy/emit

#[test]
fn copy_emits_osc52_on_stdout_when_no_tty() {
    let state = State::new();
    let out = clipring(&state, &["copy"], Some(b"hello"));
    ok(&out);
    // base64("hello") = aGVsbG8=
    assert_eq!(out.stdout, b"\x1b]52;c;aGVsbG8=\x07");
    assert!(stderr_str(&out).contains("copied 5 B"));
}

#[test]
fn copy_from_args_joins_words() {
    let state = State::new();
    let out = clipring(&state, &["copy", "two", "words"], None);
    ok(&out);
    let pasted = clipring(&state, &["paste"], None);
    assert_eq!(pasted.stdout, b"two words");
}

#[test]
fn copy_after_double_dash_takes_flags_literally() {
    // `--` must stop both the copy flag parser AND global-flag extraction:
    // the words after it are content, even when they look like our flags.
    let state = State::new();
    let out = clipring(
        &state,
        &["copy", "--no-emit", "--", "--capacity", "5"],
        None,
    );
    ok(&out);
    let pasted = clipring(&state, &["paste"], None);
    assert_eq!(pasted.stdout, b"--capacity 5");
}

#[test]
fn copy_flags_shape_the_sequence() {
    let state = State::new();
    let out = clipring(&state, &["copy", "--primary"], Some(b"x"));
    ok(&out);
    assert!(out.stdout.starts_with(b"\x1b]52;p;"));
    let out = clipring(&state, &["copy", "--wrap", "tmux"], Some(b"x"));
    ok(&out);
    assert!(out.stdout.starts_with(b"\x1bPtmux;\x1b\x1b]52;"));
    assert!(out.stdout.ends_with(b"\x1b\\"));
}

#[test]
fn copy_over_limit_stores_but_does_not_emit() {
    let state = State::new();
    let big = vec![b'a'; 100];
    let out = clipring(&state, &["copy", "--limit", "16"], Some(&big));
    ok(&out);
    assert!(out.stdout.is_empty(), "no sequence should be emitted");
    assert!(stderr_str(&out).contains("exceeds limit 16 B"));
    // The entry is still in history and retrievable.
    let pasted = clipring(&state, &["paste"], None);
    assert_eq!(pasted.stdout, big);
}

#[test]
fn copy_trim_strips_trailing_newlines_only() {
    let state = State::new();
    ok(&clipring(
        &state,
        &["copy", "--trim"],
        Some(b"  spaced  \n\n"),
    ));
    let pasted = clipring(&state, &["paste"], None);
    assert_eq!(pasted.stdout, b"  spaced  ");
    // Empty stdin (after nothing at all) is an operational error, not a
    // silently-stored empty entry.
    let out = clipring(&state, &["copy"], Some(b""));
    assert_eq!(out.status.code(), Some(1));
    assert!(stderr_str(&out).contains("nothing to copy"));
}

#[test]
fn emit_skips_history_and_hard_errors_over_limit() {
    let state = State::new();
    let out = clipring(&state, &["emit"], Some(b"ephemeral"));
    ok(&out);
    assert_eq!(out.stdout, b"\x1b]52;c;ZXBoZW1lcmFs\x07");
    let list = clipring(&state, &["list"], None);
    assert!(stderr_str(&list).contains("history is empty"));
    // Unlike copy (which still stores), emit has nothing to fall back on:
    // an over-limit payload is a hard failure.
    let out = clipring(&state, &["emit", "--limit", "4"], Some(b"too big"));
    assert_eq!(out.status.code(), Some(1));
    assert!(out.stdout.is_empty());
}

// -------------------------------------------------------- paste & round trip

#[test]
fn paste_round_trips_exact_binary_bytes() {
    let state = State::new();
    let data: Vec<u8> = vec![0, 1, 2, 0x1b, 0xfe, 0xff, b'\n'];
    ok(&clipring(&state, &["copy", "--no-emit"], Some(&data)));
    let out = clipring(&state, &["paste"], None);
    ok(&out);
    assert_eq!(out.stdout, data, "paste must be byte-identical");
}

#[test]
fn paste_indexes_from_newest() {
    let state = State::new();
    ok(&clipring(&state, &["copy", "--no-emit", "first"], None));
    ok(&clipring(&state, &["copy", "--no-emit", "second"], None));
    assert_eq!(clipring(&state, &["paste", "0"], None).stdout, b"second");
    assert_eq!(clipring(&state, &["paste", "1"], None).stdout, b"first");
}

#[test]
fn paste_fails_usefully_when_index_or_history_missing() {
    let state = State::new();
    let out = clipring(&state, &["paste"], None);
    assert_eq!(out.status.code(), Some(1));
    assert!(stderr_str(&out).contains("history is empty"));
    ok(&clipring(&state, &["copy", "--no-emit", "only"], None));
    let out = clipring(&state, &["paste", "9"], None);
    assert_eq!(out.status.code(), Some(1));
    assert!(stderr_str(&out).contains("history has 1 entry"));
}

// -------------------------------------------------------------- decode

#[test]
fn decode_round_trips_emitted_sequence() {
    let state = State::new();
    let emitted = clipring(&state, &["emit"], Some(b"round trip me"));
    ok(&emitted);
    let out = clipring(&state, &["decode"], Some(&emitted.stdout));
    ok(&out);
    assert_eq!(out.stdout, b"round trip me");
}

#[test]
fn decode_unwraps_tmux_and_screen_envelopes() {
    let state = State::new();
    for wrap in ["tmux", "screen"] {
        let emitted = clipring(&state, &["emit", "--wrap", wrap], Some(b"wrapped"));
        ok(&emitted);
        let out = clipring(&state, &["decode"], Some(&emitted.stdout));
        ok(&out);
        assert_eq!(out.stdout, b"wrapped", "wrap mode {wrap}");
    }
}

#[test]
fn decode_json_reports_metadata_and_no_match_exits_1() {
    let state = State::new();
    let emitted = clipring(&state, &["emit", "--primary"], Some(b"meta"));
    let out = clipring(&state, &["decode", "--json"], Some(&emitted.stdout));
    ok(&out);
    let line = stdout_str(&out);
    assert!(line.contains(r#""selection":"p""#), "line was: {line}");
    assert!(line.contains(r#""size":4"#), "line was: {line}");
    let out = clipring(&state, &["decode"], Some(b"just plain text"));
    assert_eq!(out.status.code(), Some(1));
}

// ------------------------------------------------------ list, ring, pins

#[test]
fn list_shows_rows_newest_first_with_preview() {
    let state = State::new();
    ok(&clipring(&state, &["copy", "--no-emit", "alpha"], None));
    ok(&clipring(&state, &["copy", "--no-emit", "beta"], None));
    let out = clipring(&state, &["list"], None);
    ok(&out);
    let lines: Vec<String> = stdout_str(&out).lines().map(String::from).collect();
    assert_eq!(lines.len(), 2);
    assert!(lines[0].contains("beta") && lines[0].contains("  0"));
    assert!(lines[1].contains("alpha") && lines[1].contains("  1"));
}

#[test]
fn list_json_records_carry_stable_unique_ids() {
    let state = State::new();
    ok(&clipring(&state, &["copy", "--no-emit", "hello"], None));
    ok(&clipring(&state, &["copy", "--no-emit", "again"], None));
    let out = clipring(&state, &["list", "--json"], None);
    ok(&out);
    let text = stdout_str(&out);
    assert!(text.contains(r#""index":0"#));
    assert!(text.contains(r#""preview":"hello""#));
    assert!(text.contains(r#""text":true"#));
    // ids were assigned by two separate processes and must not collide.
    assert!(text.contains(r#""id":1"#), "json was: {text}");
    assert!(text.contains(r#""id":2"#), "json was: {text}");
}

#[test]
fn duplicate_copy_promotes_not_duplicates() {
    let state = State::new();
    ok(&clipring(&state, &["copy", "--no-emit", "repeat"], None));
    ok(&clipring(&state, &["copy", "--no-emit", "other"], None));
    ok(&clipring(&state, &["copy", "--no-emit", "repeat"], None));
    let out = clipring(&state, &["list"], None);
    let text = stdout_str(&out);
    assert_eq!(text.lines().count(), 2, "no duplicate rows:\n{text}");
    assert!(text.lines().next().unwrap().contains("repeat"));
}

#[test]
fn capacity_evicts_oldest_across_invocations() {
    let state = State::new();
    for word in ["one", "two", "three"] {
        ok(&clipring(
            &state,
            &["--capacity", "2", "copy", "--no-emit", word],
            None,
        ));
    }
    let out = clipring(&state, &["--capacity", "2", "list"], None);
    let text = stdout_str(&out);
    assert_eq!(text.lines().count(), 2);
    assert!(
        !text.contains("one"),
        "oldest entry must be evicted:\n{text}"
    );
}

#[test]
fn pinned_entries_survive_capacity_pressure() {
    let state = State::new();
    ok(&clipring(
        &state,
        &["--capacity", "2", "copy", "--no-emit", "precious"],
        None,
    ));
    ok(&clipring(&state, &["--capacity", "2", "pin", "0"], None));
    for i in 0..4 {
        let word = format!("noise-{i}");
        ok(&clipring(
            &state,
            &["--capacity", "2", "copy", "--no-emit", &word],
            None,
        ));
    }
    let out = clipring(&state, &["--capacity", "2", "list"], None);
    let text = stdout_str(&out);
    assert!(text.contains("precious"), "pinned entry evicted:\n{text}");
    assert!(
        text.lines().any(|l| l.starts_with('*')),
        "pin marker missing:\n{text}"
    );
}

#[test]
fn rm_and_clear_manage_the_ring() {
    let state = State::new();
    for word in ["a", "b", "c"] {
        ok(&clipring(&state, &["copy", "--no-emit", word], None));
    }
    ok(&clipring(&state, &["pin", "2"], None)); // pin "a"
    let out = clipring(&state, &["rm", "0"], None); // remove "c"
    ok(&out);
    assert!(stderr_str(&out).contains("removed entry 0"));
    let out = clipring(&state, &["clear"], None);
    ok(&out);
    assert!(
        stderr_str(&out).contains("kept 1 pinned"),
        "stderr: {}",
        stderr_str(&out)
    );
    let out = clipring(&state, &["clear", "--all"], None);
    ok(&out);
    let list = clipring(&state, &["list"], None);
    assert!(stderr_str(&list).contains("history is empty"));
}

// ------------------------------------------------------------- search

#[test]
fn search_filters_case_insensitively_and_exits_1_on_no_match() {
    let state = State::new();
    ok(&clipring(
        &state,
        &["copy", "--no-emit", "SELECT * FROM users"],
        None,
    ));
    ok(&clipring(
        &state,
        &["copy", "--no-emit", "plain text"],
        None,
    ));
    let out = clipring(&state, &["search", "select"], None);
    ok(&out);
    let text = stdout_str(&out);
    assert!(text.contains("SELECT"));
    assert!(!text.contains("plain"));
    // grep-like contract: no match is exit 1, so scripts can branch on it.
    let out = clipring(&state, &["search", "absent"], None);
    assert_eq!(out.status.code(), Some(1));
}

// --------------------------------------------------------------- pick

#[test]
fn pick_promotes_choice_and_emits_it() {
    let state = State::new();
    ok(&clipring(&state, &["copy", "--no-emit", "wanted"], None));
    ok(&clipring(&state, &["copy", "--no-emit", "newer"], None));
    // Choose index 1 ("wanted"); menu goes to stderr, sequence to stdout.
    let out = clipring(&state, &["pick"], Some(b"1\n"));
    ok(&out);
    assert!(stderr_str(&out).contains("re-copied entry 1"));
    let decoded = clipring(&state, &["decode"], Some(&out.stdout));
    assert_eq!(decoded.stdout, b"wanted");
    // The chosen entry is now the newest.
    assert_eq!(clipring(&state, &["paste"], None).stdout, b"wanted");
}

#[test]
fn pick_print_writes_raw_bytes_to_stdout() {
    let state = State::new();
    ok(&clipring(
        &state,
        &["copy", "--no-emit", "raw output"],
        None,
    ));
    let out = clipring(&state, &["pick", "--print"], Some(b"0\n"));
    ok(&out);
    assert_eq!(out.stdout, b"raw output");
}

#[test]
fn pick_cancels_on_q_and_rejects_bad_choices() {
    let state = State::new();
    ok(&clipring(
        &state,
        &["copy", "--no-emit", "keep order"],
        None,
    ));
    let out = clipring(&state, &["pick"], Some(b"q\n"));
    ok(&out);
    assert!(stderr_str(&out).contains("cancelled"));
    assert!(out.stdout.is_empty());
    let out = clipring(&state, &["pick"], Some(b"5\n"));
    assert_eq!(out.status.code(), Some(1));
    assert!(stderr_str(&out).contains("out of range"));
}

// ------------------------------------------------------- state handling

#[test]
fn damaged_history_lines_warn_but_do_not_break() {
    let state = State::new();
    ok(&clipring(&state, &["copy", "--no-emit", "good"], None));
    // Append garbage, as a torn write would leave behind.
    let path = state.path().join("history.jsonl");
    let mut content = std::fs::read_to_string(&path).unwrap();
    content.push_str("{torn record\n");
    std::fs::write(&path, content).unwrap();
    let out = clipring(&state, &["list"], None);
    ok(&out);
    assert!(stdout_str(&out).contains("good"));
    assert!(stderr_str(&out).contains("skipped 1 damaged"));
}

#[test]
fn info_reports_state_and_settings() {
    let state = State::new();
    ok(&clipring(&state, &["copy", "--no-emit", "x"], None));
    ok(&clipring(&state, &["pin", "0"], None));
    let out = clipring(&state, &["info"], None);
    ok(&out);
    let text = stdout_str(&out);
    assert!(text.contains("entries:  1 (1 pinned)"), "info was: {text}");
    assert!(text.contains("capacity: 50"));
    assert!(text.contains("wrap:     none"));
    assert!(text.contains("history.jsonl"));
}

#[test]
fn closed_stdout_pipe_exits_zero_without_panicking() {
    // `clipring paste | head -c 1` style usage: the reader goes away after
    // one byte. A naive println!-based CLI panics with a broken-pipe
    // abort here; clipring must stop quietly with exit 0, like grep/cat.
    // The 200 KB payload overflows the 64 KiB pipe buffer, so the write
    // deterministically hits EPIPE once `head` exits.
    let state = State::new();
    let big = vec![b'x'; 200_000];
    ok(&clipring(&state, &["copy", "--no-emit"], Some(&big)));
    let out = std::process::Command::new("sh")
        .arg("-c")
        .arg("{ \"$0\" --state \"$1\" paste; echo \"rc=$?\" >&2; } | head -c 1 >/dev/null")
        .arg(BIN)
        .arg(state.arg())
        .output()
        .expect("run sh pipeline");
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(!err.contains("panic"), "clipring panicked: {err}");
    assert!(
        err.contains("rc=0"),
        "expected exit 0 on EPIPE, stderr: {err}"
    );
    // Same guarantee for the line-oriented commands.
    let out = std::process::Command::new("sh")
        .arg("-c")
        .arg("{ \"$0\" --state \"$1\" list; echo \"rc=$?\" >&2; } | head -c 1 >/dev/null")
        .arg(BIN)
        .arg(state.arg())
        .output()
        .expect("run sh pipeline");
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(!err.contains("panic"), "clipring list panicked: {err}");
    assert!(
        err.contains("rc=0"),
        "list must exit 0 on EPIPE, stderr: {err}"
    );
}

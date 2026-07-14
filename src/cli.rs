//! Command-line surface: parsing, dispatch, and the thin I/O layer.
//!
//! Everything interesting lives in the pure modules (`ring`, `osc52`,
//! `store`, `picker`); this file wires them to argv, stdin/stdout, the
//! history file, and — for emission — the controlling terminal.

use std::io::{self, IsTerminal, Read, Write};
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

use crate::osc52::{self, Selection, Wrap};
use crate::picker;
use crate::ring::{Entry, Ring};
use crate::store;
use crate::textutil::{human_size, preview};
use crate::VERSION;

/// Default cap on the base64 payload; ~75 KB of raw bytes. Terminals and
/// multiplexers commonly truncate or drop OSC 52 sequences beyond this.
pub const DEFAULT_LIMIT: usize = 100_000;
/// Default history capacity (unpinned entries).
pub const DEFAULT_CAPACITY: usize = 50;

#[derive(Debug)]
enum CliError {
    /// Bad invocation — exit 2, point at --help.
    Usage(String),
    /// The invocation was fine but the operation failed — exit 1.
    Op(String),
    /// Downstream closed our stdout (e.g. `clipring list | head -1`).
    /// Like grep/cat we stop writing and exit 0 — never a crash.
    Pipe,
}

impl From<io::Error> for CliError {
    fn from(e: io::Error) -> CliError {
        if e.kind() == io::ErrorKind::BrokenPipe {
            CliError::Pipe
        } else {
            CliError::Op(e.to_string())
        }
    }
}

type CliResult = Result<i32, CliError>;

/// Entry point: parse argv (minus the program name) and run. Returns the
/// process exit code.
pub fn run(args: &[String]) -> i32 {
    match dispatch(args) {
        Ok(code) => code,
        Err(CliError::Usage(msg)) => {
            eprintln!("clipring: {msg}");
            eprintln!("try 'clipring --help'");
            2
        }
        Err(CliError::Op(msg)) => {
            eprintln!("clipring: {msg}");
            1
        }
        Err(CliError::Pipe) => 0,
    }
}

fn dispatch(args: &[String]) -> CliResult {
    let (globals, command, rest) = split_globals(args)?;
    let Some(command) = command else {
        return Err(CliError::Usage("no command given".to_string()));
    };
    match command.as_str() {
        "--version" | "-V" | "version" => {
            writeln!(io::stdout().lock(), "clipring {VERSION}")?;
            Ok(0)
        }
        "--help" | "-h" | "help" => {
            write!(io::stdout().lock(), "{}", help_text())?;
            Ok(0)
        }
        "copy" | "c" => cmd_copy(&globals, &rest),
        "paste" | "p" => cmd_paste(&globals, &rest),
        "list" | "ls" => cmd_list(&globals, &rest),
        "pick" => cmd_pick(&globals, &rest),
        "search" => cmd_search(&globals, &rest),
        "pin" => cmd_set_pin(&globals, &rest, true),
        "unpin" => cmd_set_pin(&globals, &rest, false),
        "rm" => cmd_rm(&globals, &rest),
        "clear" => cmd_clear(&globals, &rest),
        "emit" => cmd_emit(&globals, &rest),
        "decode" => cmd_decode(&rest),
        "info" => cmd_info(&globals),
        other => Err(CliError::Usage(format!("unknown command '{other}'"))),
    }
}

// ---------------------------------------------------------------- globals

struct Globals {
    state_dir: PathBuf,
    capacity: usize,
}

impl Globals {
    fn load_ring(&self) -> Result<Ring, CliError> {
        let (ring, skipped) = store::load(&self.state_dir, self.capacity)
            .map_err(|e| CliError::Op(format!("cannot read history: {e}")))?;
        if skipped > 0 {
            eprintln!(
                "clipring: warning: skipped {skipped} damaged history line(s) in {}",
                self.state_dir.join(store::HISTORY_FILE).display()
            );
        }
        Ok(ring)
    }

    fn save_ring(&self, ring: &Ring) -> Result<(), CliError> {
        store::save(&self.state_dir, ring)
            .map_err(|e| CliError::Op(format!("cannot write history: {e}")))
    }
}

/// Pull `--state DIR` / `--capacity N` (valid before or after the command)
/// out of argv; the first bare word is the command. A `--` after the command
/// ends global-flag extraction, so `copy -- --capacity 5` copies the literal
/// text instead of losing it to the global parser.
fn split_globals(args: &[String]) -> Result<(Globals, Option<String>, Vec<String>), CliError> {
    let mut state_flag: Option<String> = None;
    let mut capacity_flag: Option<usize> = None;
    let mut command: Option<String> = None;
    let mut rest: Vec<String> = Vec::new();
    let mut literal = false;
    let mut it = args.iter();
    while let Some(arg) = it.next() {
        if literal {
            rest.push(arg.clone());
            continue;
        }
        match arg.as_str() {
            "--" if command.is_some() => {
                literal = true;
                rest.push(arg.clone());
            }
            "--state" => {
                state_flag = Some(take_value(&mut it, "--state")?);
            }
            "--capacity" => {
                capacity_flag = Some(parse_count(
                    &take_value(&mut it, "--capacity")?,
                    "--capacity",
                )?);
            }
            _ if command.is_none() => command = Some(arg.clone()),
            _ => rest.push(arg.clone()),
        }
    }
    let env = |k: &str| std::env::var(k).ok();
    let state_dir = store::resolve_state_dir(
        state_flag.as_deref(),
        env("CLIPRING_STATE").as_deref(),
        env("XDG_STATE_HOME").as_deref(),
        env("HOME").as_deref(),
    )
    .map_err(CliError::Op)?;
    let capacity = match capacity_flag {
        Some(n) => n,
        None => match env("CLIPRING_CAPACITY") {
            Some(v) => parse_count(&v, "$CLIPRING_CAPACITY")?,
            None => DEFAULT_CAPACITY,
        },
    };
    Ok((
        Globals {
            state_dir,
            capacity,
        },
        command,
        rest,
    ))
}

// ------------------------------------------------------------- emit opts

struct EmitOpts {
    selection: Selection,
    wrap: Wrap,
    limit: usize,
    force_stdout: bool,
}

impl EmitOpts {
    fn from_env() -> Result<EmitOpts, CliError> {
        let wrap = match std::env::var("CLIPRING_WRAP").ok().as_deref() {
            Some(v) => parse_wrap(v)?,
            None => None,
        };
        let limit = match std::env::var("CLIPRING_LIMIT").ok() {
            Some(v) => parse_count(&v, "$CLIPRING_LIMIT")?,
            None => DEFAULT_LIMIT,
        };
        Ok(EmitOpts {
            selection: Selection::Clipboard,
            wrap: wrap.unwrap_or_else(detected_wrap),
            limit,
            force_stdout: false,
        })
    }

    /// Try to consume one emit-related flag; true if it was one.
    fn take_flag(
        &mut self,
        arg: &str,
        it: &mut std::slice::Iter<String>,
    ) -> Result<bool, CliError> {
        match arg {
            "--primary" => self.selection = Selection::Primary,
            "--stdout" => self.force_stdout = true,
            "--wrap" => {
                let v = take_value(it, "--wrap")?;
                self.wrap = parse_wrap(&v)?.unwrap_or_else(detected_wrap);
            }
            "--limit" => self.limit = parse_count(&take_value(it, "--limit")?, "--limit")?,
            _ => return Ok(false),
        }
        Ok(true)
    }

    fn over_limit(&self, data: &[u8]) -> Option<(usize, usize)> {
        let len = osc52::payload_len(data);
        (self.limit > 0 && len > self.limit).then_some((len, self.limit))
    }

    /// Build the sequence and write it to the controlling terminal (or
    /// stdout). Returns where it went, for the status line.
    fn emit(&self, data: &[u8]) -> Result<&'static str, CliError> {
        let seq = osc52::sequence(self.selection, data, self.wrap);
        if !self.force_stdout {
            if let Ok(mut tty) = std::fs::OpenOptions::new().write(true).open("/dev/tty") {
                tty.write_all(&seq)?;
                tty.flush()?;
                return Ok("/dev/tty");
            }
        }
        let mut out = io::stdout().lock();
        out.write_all(&seq)?;
        out.flush()?;
        Ok("stdout")
    }
}

/// `auto`/env-driven wrap detection against the real environment.
fn detected_wrap() -> Wrap {
    osc52::detect_wrap(
        std::env::var("TMUX").ok().as_deref(),
        std::env::var("TERM").ok().as_deref(),
    )
}

// -------------------------------------------------------------- commands

fn cmd_copy(globals: &Globals, args: &[String]) -> CliResult {
    let mut opts = EmitOpts::from_env()?;
    let mut store_entry = true;
    let mut emit = true;
    let mut trim = false;
    let mut text_args: Vec<String> = Vec::new();
    let mut literal = false;
    let mut it = args.iter();
    while let Some(arg) = it.next() {
        if literal || !arg.starts_with('-') || arg == "-" {
            text_args.push(arg.clone());
        } else if arg == "--" {
            literal = true;
        } else if arg == "--no-store" {
            store_entry = false;
        } else if arg == "--no-emit" {
            emit = false;
        } else if arg == "--trim" {
            trim = true;
        } else if !opts.take_flag(arg, &mut it)? {
            return Err(CliError::Usage(format!("copy: unknown flag '{arg}'")));
        }
    }
    let mut data = if text_args.is_empty() || text_args == ["-"] {
        read_stdin()?
    } else {
        text_args.join(" ").into_bytes()
    };
    if trim {
        while data.last().is_some_and(|&b| b == b'\n' || b == b'\r') {
            data.pop();
        }
    }
    if data.is_empty() {
        return Err(CliError::Op(
            "nothing to copy (stdin was empty)".to_string(),
        ));
    }
    let size = human_size(data.len() as u64);
    let mut ring = globals.load_ring()?;
    if store_entry {
        ring.push(data.clone(), now_ms());
        globals.save_ring(&ring)?;
    }
    let mut note = format!(
        "{} {size}",
        if store_entry {
            "copied"
        } else {
            "copied (unstored)"
        }
    );
    if emit {
        if let Some((len, limit)) = opts.over_limit(&data) {
            note.push_str(&format!(
                "; NOT sent to terminal: payload {len} B exceeds limit {limit} B (--limit 0 lifts it)"
            ));
        } else {
            let dest = opts.emit(&data)?;
            note.push_str(&format!(
                " -> {} via {dest} [{}]",
                opts.selection.param_name(),
                opts.wrap.name()
            ));
        }
    } else {
        note.push_str(" (stored only)");
    }
    eprintln!(
        "clipring: {note} (history: {}/{})",
        ring.len(),
        ring.capacity()
    );
    Ok(0)
}

fn cmd_paste(globals: &Globals, args: &[String]) -> CliResult {
    let index = optional_index(args, "paste")?.unwrap_or(0);
    let ring = globals.load_ring()?;
    if ring.is_empty() {
        return Err(CliError::Op("history is empty".to_string()));
    }
    let entry = ring
        .get(index)
        .ok_or_else(|| CliError::Op(index_error(index, &ring)))?;
    let mut out = io::stdout().lock();
    out.write_all(&entry.data)?;
    out.flush()?;
    Ok(0)
}

fn cmd_list(globals: &Globals, args: &[String]) -> CliResult {
    let mut json = false;
    let mut count: Option<usize> = None;
    let mut it = args.iter();
    while let Some(arg) = it.next() {
        match arg.as_str() {
            "--json" => json = true,
            "-n" | "--count" => count = Some(parse_count(&take_value(&mut it, "-n")?, "-n")?),
            other => return Err(CliError::Usage(format!("list: unknown flag '{other}'"))),
        }
    }
    let ring = globals.load_ring()?;
    if ring.is_empty() && !json {
        eprintln!("clipring: history is empty");
        return Ok(0);
    }
    let now = now_ms();
    let take = count.unwrap_or(usize::MAX);
    let mut out = io::stdout().lock();
    for (i, entry) in ring.iter().enumerate().take(take) {
        if json {
            writeln!(out, "{}", entry_json(i, entry))?;
        } else {
            writeln!(out, "{}", picker::format_row(i, entry, now))?;
        }
    }
    out.flush()?;
    Ok(0)
}

fn cmd_pick(globals: &Globals, args: &[String]) -> CliResult {
    let mut opts = EmitOpts::from_env()?;
    let mut print = false;
    let mut it = args.iter();
    while let Some(arg) = it.next() {
        if arg == "--print" {
            print = true;
        } else if !opts.take_flag(arg, &mut it)? {
            return Err(CliError::Usage(format!("pick: unknown flag '{arg}'")));
        }
    }
    let mut ring = globals.load_ring()?;
    if ring.is_empty() {
        return Err(CliError::Op(
            "history is empty — nothing to pick".to_string(),
        ));
    }
    let now = now_ms();
    for line in picker::menu_lines(&ring, now) {
        eprintln!("{line}");
    }
    eprint!("pick (0-{}, q to cancel)> ", ring.len() - 1);
    io::stderr().flush()?;
    let mut input = String::new();
    io::stdin().read_line(&mut input)?;
    let Some(index) = picker::parse_choice(&input, ring.len()).map_err(CliError::Op)? else {
        eprintln!("clipring: cancelled");
        return Ok(0);
    };
    let entry = ring.promote(index, now).map_err(CliError::Op)?;
    let data = entry.data.clone();
    let size = human_size(data.len() as u64);
    globals.save_ring(&ring)?;
    if print {
        let mut out = io::stdout().lock();
        out.write_all(&data)?;
        out.flush()?;
    } else {
        if let Some((len, limit)) = opts.over_limit(&data) {
            return Err(CliError::Op(format!(
                "payload {len} B exceeds limit {limit} B (--limit 0 lifts it)"
            )));
        }
        let dest = opts.emit(&data)?;
        eprintln!("clipring: re-copied entry {index} ({size}) via {dest}");
    }
    Ok(0)
}

fn cmd_search(globals: &Globals, args: &[String]) -> CliResult {
    let mut json = false;
    let mut pattern: Option<String> = None;
    for arg in args {
        match arg.as_str() {
            "--json" => json = true,
            _ if pattern.is_none() => pattern = Some(arg.clone()),
            other => {
                return Err(CliError::Usage(format!(
                    "search: unexpected argument '{other}'"
                )))
            }
        }
    }
    let pattern = pattern.ok_or_else(|| CliError::Usage("search: missing PATTERN".to_string()))?;
    let ring = globals.load_ring()?;
    let hits = ring.search(&pattern);
    if hits.is_empty() {
        eprintln!("clipring: no entry matches '{pattern}'");
        return Ok(1); // grep-like: no match is exit 1
    }
    let now = now_ms();
    let mut out = io::stdout().lock();
    for i in hits {
        let entry = ring.get(i).expect("search returned valid index");
        if json {
            writeln!(out, "{}", entry_json(i, entry))?;
        } else {
            writeln!(out, "{}", picker::format_row(i, entry, now))?;
        }
    }
    out.flush()?;
    Ok(0)
}

fn cmd_set_pin(globals: &Globals, args: &[String], pinned: bool) -> CliResult {
    let verb = if pinned { "pin" } else { "unpin" };
    let index = required_index(args, verb)?;
    let mut ring = globals.load_ring()?;
    ring.set_pinned(index, pinned).map_err(CliError::Op)?;
    globals.save_ring(&ring)?;
    eprintln!("clipring: {verb}ned entry {index}");
    Ok(0)
}

fn cmd_rm(globals: &Globals, args: &[String]) -> CliResult {
    let index = required_index(args, "rm")?;
    let mut ring = globals.load_ring()?;
    let entry = ring.remove(index).map_err(CliError::Op)?;
    globals.save_ring(&ring)?;
    eprintln!(
        "clipring: removed entry {index} ({}): {}",
        human_size(entry.data.len() as u64),
        preview(&entry.data, 40)
    );
    Ok(0)
}

fn cmd_clear(globals: &Globals, args: &[String]) -> CliResult {
    let all = match args {
        [] => false,
        [a] if a == "--all" => true,
        [other, ..] => return Err(CliError::Usage(format!("clear: unknown flag '{other}'"))),
    };
    let mut ring = globals.load_ring()?;
    let removed = ring.clear(all);
    globals.save_ring(&ring)?;
    let kept = ring.len();
    if all || kept == 0 {
        eprintln!("clipring: removed {removed} entr{}", plural_y(removed));
    } else {
        eprintln!(
            "clipring: removed {removed} entr{}, kept {kept} pinned",
            plural_y(removed)
        );
    }
    Ok(0)
}

fn cmd_emit(_globals: &Globals, args: &[String]) -> CliResult {
    let mut opts = EmitOpts::from_env()?;
    let mut it = args.iter();
    while let Some(arg) = it.next() {
        if !opts.take_flag(arg, &mut it)? {
            return Err(CliError::Usage(format!("emit: unknown flag '{arg}'")));
        }
    }
    let data = read_stdin()?;
    if data.is_empty() {
        return Err(CliError::Op(
            "nothing to emit (stdin was empty)".to_string(),
        ));
    }
    if let Some((len, limit)) = opts.over_limit(&data) {
        return Err(CliError::Op(format!(
            "payload {len} B exceeds limit {limit} B (--limit 0 lifts it)"
        )));
    }
    opts.emit(&data)?;
    Ok(0)
}

fn cmd_decode(args: &[String]) -> CliResult {
    let json = match args {
        [] => false,
        [a] if a == "--json" => true,
        [other, ..] => return Err(CliError::Usage(format!("decode: unknown flag '{other}'"))),
    };
    let input = read_stdin()?;
    let captures = osc52::extract(&input);
    if captures.is_empty() {
        eprintln!("clipring: no OSC 52 sequences found in input");
        return Ok(1);
    }
    let mut out = io::stdout().lock();
    for cap in &captures {
        if json {
            let line = crate::jsonl::encode(&[
                ("selection", crate::jsonl::Value::Str(cap.selection.clone())),
                ("size", crate::jsonl::Value::UInt(cap.data.len() as u64)),
                (
                    "text",
                    crate::jsonl::Value::Bool(std::str::from_utf8(&cap.data).is_ok()),
                ),
                (
                    "preview",
                    crate::jsonl::Value::Str(preview(&cap.data, picker::PREVIEW_CHARS)),
                ),
            ]);
            writeln!(out, "{line}")?;
        } else {
            out.write_all(&cap.data)?;
        }
    }
    out.flush()?;
    Ok(0)
}

fn cmd_info(globals: &Globals) -> CliResult {
    let ring = globals.load_ring()?;
    let opts = EmitOpts::from_env()?;
    let tty = if io::stdout().is_terminal() {
        "stdout is a terminal"
    } else {
        "stdout is piped"
    };
    let mut out = io::stdout().lock();
    writeln!(out, "clipring {VERSION}")?;
    writeln!(
        out,
        "state:    {}",
        globals.state_dir.join(store::HISTORY_FILE).display()
    )?;
    writeln!(
        out,
        "entries:  {} ({} pinned)",
        ring.len(),
        ring.pinned_count()
    )?;
    writeln!(out, "capacity: {}", ring.capacity())?;
    writeln!(out, "wrap:     {}", opts.wrap.name())?;
    writeln!(out, "limit:    {} bytes of base64", opts.limit)?;
    writeln!(out, "tty:      {tty}")?;
    Ok(0)
}

// --------------------------------------------------------------- helpers

fn entry_json(index: usize, entry: &Entry) -> String {
    crate::jsonl::encode(&[
        ("index", crate::jsonl::Value::UInt(index as u64)),
        ("id", crate::jsonl::Value::UInt(entry.id)),
        ("t", crate::jsonl::Value::UInt(entry.at_ms)),
        ("pin", crate::jsonl::Value::Bool(entry.pinned)),
        ("size", crate::jsonl::Value::UInt(entry.data.len() as u64)),
        ("text", crate::jsonl::Value::Bool(entry.text().is_some())),
        (
            "preview",
            crate::jsonl::Value::Str(preview(&entry.data, picker::PREVIEW_CHARS)),
        ),
    ])
}

fn read_stdin() -> Result<Vec<u8>, CliError> {
    let mut data = Vec::new();
    io::stdin().lock().read_to_end(&mut data)?;
    Ok(data)
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

fn take_value(it: &mut std::slice::Iter<String>, flag: &str) -> Result<String, CliError> {
    it.next()
        .cloned()
        .ok_or_else(|| CliError::Usage(format!("{flag} needs a value")))
}

/// Parse a non-negative count / size flag.
fn parse_count(value: &str, what: &str) -> Result<usize, CliError> {
    value
        .parse()
        .map_err(|_| CliError::Usage(format!("{what}: '{value}' is not a non-negative integer")))
}

/// Parse a wrap mode; `auto` (and only `auto`) maps to None = detect later.
fn parse_wrap(value: &str) -> Result<Option<Wrap>, CliError> {
    match value {
        "auto" => Ok(None),
        "none" => Ok(Some(Wrap::None)),
        "tmux" => Ok(Some(Wrap::Tmux)),
        "screen" => Ok(Some(Wrap::Screen)),
        other => Err(CliError::Usage(format!(
            "--wrap: '{other}' is not one of auto|none|tmux|screen"
        ))),
    }
}

fn required_index(args: &[String], verb: &str) -> Result<usize, CliError> {
    optional_index(args, verb)?.ok_or_else(|| CliError::Usage(format!("{verb}: missing INDEX")))
}

fn optional_index(args: &[String], verb: &str) -> Result<Option<usize>, CliError> {
    match args {
        [] => Ok(None),
        [one] => one
            .parse()
            .map(Some)
            .map_err(|_| CliError::Usage(format!("{verb}: '{one}' is not an index"))),
        [_, extra, ..] => Err(CliError::Usage(format!(
            "{verb}: unexpected argument '{extra}'"
        ))),
    }
}

fn index_error(index: usize, ring: &Ring) -> String {
    format!(
        "no entry {index} (history has {} entr{})",
        ring.len(),
        plural_y(ring.len())
    )
}

fn plural_y(n: usize) -> &'static str {
    if n == 1 {
        "y"
    } else {
        "ies"
    }
}

impl Selection {
    fn param_name(self) -> &'static str {
        match self {
            Selection::Clipboard => "clipboard",
            Selection::Primary => "primary",
        }
    }
}

fn help_text() -> String {
    format!(
        "\
clipring {VERSION} — terminal clipboard history over OSC 52

USAGE:
    clipring [--state DIR] [--capacity N] <COMMAND> [ARGS]

COMMANDS:
    copy [TEXT..]     Store stdin (or TEXT) in history and copy it to the
                      terminal clipboard via OSC 52   (alias: c)
    paste [INDEX]     Print entry INDEX (default 0 = newest) to stdout
                      (alias: p)
    list              Show the history ring            (alias: ls)
    pick              Numbered menu; re-copy the chosen entry
    search PATTERN    List entries containing PATTERN (case-insensitive)
    pin INDEX         Protect an entry from eviction
    unpin INDEX       Remove that protection
    rm INDEX          Delete one entry
    clear [--all]     Delete unpinned entries (--all: pinned too)
    emit              stdin -> OSC 52 sequence, without storing
    decode            Extract OSC 52 payloads from a byte stream on stdin
    info              Show state location, counts, and detected wrap
    help              Show this help

OPTIONS (copy / pick / emit):
    --primary         Target the primary selection instead of the clipboard
    --wrap MODE       Passthrough wrapping: auto|none|tmux|screen (default auto)
    --limit N         Max base64 payload bytes, 0 = unlimited (default {DEFAULT_LIMIT})
    --stdout          Write the sequence to stdout instead of /dev/tty
    --no-store        copy: emit without recording      --no-emit: record only
    --trim            copy: strip trailing newlines     --print: pick to stdout

OPTIONS (list / search / decode):
    --json            One JSON object per entry or capture
    -n, --count N     list: show at most N rows

ENVIRONMENT:
    CLIPRING_STATE     State directory (default ~/.local/state/clipring)
    CLIPRING_CAPACITY  History size, unpinned entries (default {DEFAULT_CAPACITY})
    CLIPRING_LIMIT     Default for --limit
    CLIPRING_WRAP      Default for --wrap
"
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_wrap_accepts_modes_and_rejects_typos() {
        assert_eq!(parse_wrap("none").unwrap(), Some(Wrap::None));
        assert_eq!(parse_wrap("tmux").unwrap(), Some(Wrap::Tmux));
        assert_eq!(parse_wrap("screen").unwrap(), Some(Wrap::Screen));
        assert!(parse_wrap("auto").unwrap().is_none());
        let CliError::Usage(msg) = parse_wrap("tmxu").unwrap_err() else {
            panic!("expected usage error");
        };
        assert!(msg.contains("auto|none|tmux|screen"), "msg was: {msg}");
    }

    #[test]
    fn parse_count_rejects_negatives_and_words() {
        assert!(parse_count("-1", "--limit").is_err());
        assert!(parse_count("many", "--limit").is_err());
        assert_eq!(parse_count("0", "--limit").unwrap(), 0);
    }

    #[test]
    fn optional_index_handles_all_shapes() {
        let none: [String; 0] = [];
        assert_eq!(optional_index(&none, "paste").unwrap(), None);
        assert_eq!(optional_index(&["7".into()], "paste").unwrap(), Some(7));
        assert!(optional_index(&["x".into()], "paste").is_err());
        assert!(optional_index(&["1".into(), "2".into()], "paste").is_err());
    }

    #[test]
    fn help_mentions_every_command() {
        let help = help_text();
        for cmd in [
            "copy", "paste", "list", "pick", "search", "pin", "unpin", "rm", "clear", "emit",
            "decode", "info",
        ] {
            assert!(help.contains(cmd), "help is missing '{cmd}'");
        }
        assert!(help.contains("COMMANDS:"));
    }

    #[test]
    fn entry_json_reports_text_flag_and_preview() {
        let entry = Entry {
            id: 3,
            at_ms: 9,
            pinned: true,
            data: b"hi there".to_vec(),
        };
        let line = entry_json(1, &entry);
        let map = crate::jsonl::parse(&line).unwrap();
        assert_eq!(map["index"].as_uint(), Some(1));
        assert_eq!(map["text"].as_bool(), Some(true));
        assert_eq!(map["preview"].as_str(), Some("hi there"));
        assert_eq!(map["pin"].as_bool(), Some(true));
    }
}

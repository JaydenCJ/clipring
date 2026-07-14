//! Persistence: the history file, its JSONL record format, and atomic saves.
//!
//! Layout: one directory (default `$XDG_STATE_HOME/clipring`, i.e.
//! `~/.local/state/clipring`) containing `history.jsonl` — one record per
//! entry, newest first. Saves go through a temp file + rename so a crash
//! mid-write can never leave a half-written history. Damaged lines are
//! skipped (and counted) on load instead of poisoning the whole ring.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use crate::base64;
use crate::jsonl::{self, Value};
use crate::ring::{Entry, Ring};

pub const HISTORY_FILE: &str = "history.jsonl";

/// Resolve the state directory from explicit flag > `$CLIPRING_STATE` >
/// `$XDG_STATE_HOME/clipring` > `$HOME/.local/state/clipring`.
pub fn resolve_state_dir(
    flag: Option<&str>,
    env_state: Option<&str>,
    xdg_state_home: Option<&str>,
    home: Option<&str>,
) -> Result<PathBuf, String> {
    if let Some(dir) = flag.or(env_state) {
        if dir.is_empty() {
            return Err("state directory is empty".to_string());
        }
        return Ok(PathBuf::from(dir));
    }
    if let Some(xdg) = xdg_state_home.filter(|s| !s.is_empty()) {
        return Ok(Path::new(xdg).join("clipring"));
    }
    if let Some(home) = home.filter(|s| !s.is_empty()) {
        return Ok(Path::new(home).join(".local/state/clipring"));
    }
    Err("cannot locate a state directory: set $HOME or $CLIPRING_STATE".to_string())
}

/// Serialize one entry as a JSONL record.
pub fn entry_to_line(entry: &Entry) -> String {
    jsonl::encode(&[
        ("id", Value::UInt(entry.id)),
        ("t", Value::UInt(entry.at_ms)),
        ("pin", Value::Bool(entry.pinned)),
        ("data", Value::Str(base64::encode(&entry.data))),
    ])
}

/// Parse one JSONL record back into an entry.
pub fn entry_from_line(line: &str) -> Result<Entry, String> {
    let map = jsonl::parse(line)?;
    let field = |k: &str| map.get(k).ok_or_else(|| format!("missing field '{k}'"));
    let id = field("id")?.as_uint().ok_or("'id' is not an integer")?;
    let at_ms = field("t")?.as_uint().ok_or("'t' is not an integer")?;
    let pinned = field("pin")?.as_bool().ok_or("'pin' is not a bool")?;
    let b64 = field("data")?.as_str().ok_or("'data' is not a string")?;
    let data = base64::decode(b64).map_err(|e| format!("bad 'data': {e}"))?;
    Ok(Entry {
        id,
        at_ms,
        pinned,
        data,
    })
}

/// Load the ring from `dir`. A missing file is an empty ring. Returns the
/// ring plus the number of damaged lines that were skipped.
pub fn load(dir: &Path, capacity: usize) -> io::Result<(Ring, usize)> {
    let path = dir.join(HISTORY_FILE);
    let text = match fs::read_to_string(&path) {
        Ok(t) => t,
        Err(e) if e.kind() == io::ErrorKind::NotFound => {
            return Ok((Ring::new(capacity), 0));
        }
        Err(e) => return Err(e),
    };
    let mut entries = Vec::new();
    let mut skipped = 0;
    for line in text.lines() {
        if line.trim().is_empty() {
            continue;
        }
        match entry_from_line(line) {
            Ok(entry) => entries.push(entry),
            Err(_) => skipped += 1,
        }
    }
    Ok((Ring::from_entries(entries, capacity), skipped))
}

/// Save the ring atomically: write `history.jsonl.tmp-<pid>`, then rename
/// over the live file. Creates the state directory if needed.
pub fn save(dir: &Path, ring: &Ring) -> io::Result<()> {
    fs::create_dir_all(dir)?;
    let mut body = String::new();
    for entry in ring.iter() {
        body.push_str(&entry_to_line(entry));
        body.push('\n');
    }
    let tmp = dir.join(format!("{HISTORY_FILE}.tmp-{}", std::process::id()));
    fs::write(&tmp, body)?;
    let result = fs::rename(&tmp, dir.join(HISTORY_FILE));
    if result.is_err() {
        let _ = fs::remove_file(&tmp);
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tempdir(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("clipring-store-{tag}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn record_round_trips_binary_and_pin_state() {
        let entry = Entry {
            id: 12,
            at_ms: 1_752_300_000_123,
            pinned: true,
            data: vec![0, 1, 2, 0xff, b'\n'],
        };
        let line = entry_to_line(&entry);
        assert_eq!(entry_from_line(&line).unwrap(), entry);
    }

    #[test]
    fn record_rejects_missing_fields_and_wrong_types() {
        let err = entry_from_line(r#"{"id":1,"t":2,"pin":false}"#).unwrap_err();
        assert!(err.contains("data"), "err was: {err}");
        assert!(entry_from_line(r#"{"id":"x","t":2,"pin":false,"data":""}"#).is_err());
        assert!(entry_from_line(r#"{"id":1,"t":2,"pin":false,"data":"@@"}"#).is_err());
    }

    #[test]
    fn missing_file_loads_as_empty_ring() {
        let dir = tempdir("missing");
        let (ring, skipped) = load(&dir, 50).unwrap();
        assert!(ring.is_empty());
        assert_eq!(skipped, 0);
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn save_then_load_preserves_order_and_ids() {
        let dir = tempdir("roundtrip");
        let mut ring = Ring::new(50);
        ring.push(b"one".to_vec(), 100);
        ring.push(b"two".to_vec(), 200);
        ring.set_pinned(1, true).unwrap();
        save(&dir, &ring).unwrap();
        let (loaded, skipped) = load(&dir, 50).unwrap();
        assert_eq!(skipped, 0);
        assert_eq!(loaded.len(), 2);
        assert_eq!(loaded.get(0).unwrap().data, b"two");
        assert!(loaded.get(1).unwrap().pinned);
        assert_eq!(loaded.get(1).unwrap().id, ring.get(1).unwrap().id);
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn damaged_lines_are_skipped_not_fatal() {
        // A power cut mid-append leaves a torn line; the rest of the history
        // must still load.
        let dir = tempdir("damaged");
        let good = entry_to_line(&Entry {
            id: 1,
            at_ms: 5,
            pinned: false,
            data: b"ok".to_vec(),
        });
        fs::write(
            dir.join(HISTORY_FILE),
            format!("{good}\n{{\"id\":2,\"t\":6,\"pin\":fal\nnot json at all\n"),
        )
        .unwrap();
        let (ring, skipped) = load(&dir, 50).unwrap();
        assert_eq!(ring.len(), 1);
        assert_eq!(skipped, 2);
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn save_leaves_no_temp_files_behind() {
        let dir = tempdir("tmpclean");
        let mut ring = Ring::new(50);
        ring.push(b"x".to_vec(), 1);
        save(&dir, &ring).unwrap();
        let names: Vec<String> = fs::read_dir(&dir)
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        assert_eq!(names, vec![HISTORY_FILE.to_string()]);
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn resolve_state_dir_precedence() {
        let flag = resolve_state_dir(Some("/a"), Some("/b"), Some("/c"), Some("/d")).unwrap();
        assert_eq!(flag, PathBuf::from("/a"));
        let env = resolve_state_dir(None, Some("/b"), Some("/c"), Some("/d")).unwrap();
        assert_eq!(env, PathBuf::from("/b"));
        let xdg = resolve_state_dir(None, None, Some("/c"), Some("/d")).unwrap();
        assert_eq!(xdg, PathBuf::from("/c/clipring"));
        let home = resolve_state_dir(None, None, None, Some("/d")).unwrap();
        assert_eq!(home, PathBuf::from("/d/.local/state/clipring"));
        assert!(resolve_state_dir(None, None, None, None).is_err());
    }

    #[test]
    fn resolve_state_dir_ignores_empty_xdg() {
        // An exported-but-empty XDG_STATE_HOME is a common misconfiguration;
        // fall through to $HOME instead of writing to "./clipring".
        let dir = resolve_state_dir(None, None, Some(""), Some("/d")).unwrap();
        assert_eq!(dir, PathBuf::from("/d/.local/state/clipring"));
    }
}

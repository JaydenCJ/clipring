//! The interactive picker's pure parts: row rendering and choice parsing.
//!
//! `clipring pick` prints a numbered menu (to stderr, so stdout stays clean
//! for `--print` piping) and reads one line. Everything that can be unit
//! tested — how a row renders, what counts as a valid choice — lives here;
//! `cli.rs` only does the I/O.

use crate::ring::{Entry, Ring};
use crate::textutil::{human_age, human_size, preview};

/// Width budget for the preview column in menu and list rows.
pub const PREVIEW_CHARS: usize = 56;

/// Render one history row: `[pin] index  age  size  preview`.
pub fn format_row(index: usize, entry: &Entry, now_ms: u64) -> String {
    format!(
        "{} {:>3}  {:>4}  {:>8}  {}",
        if entry.pinned { "*" } else { " " },
        index,
        human_age(now_ms, entry.at_ms),
        human_size(entry.data.len() as u64),
        preview(&entry.data, PREVIEW_CHARS),
    )
}

/// Render the full menu for `pick`, newest first.
pub fn menu_lines(ring: &Ring, now_ms: u64) -> Vec<String> {
    ring.iter()
        .enumerate()
        .map(|(i, e)| format_row(i, e, now_ms))
        .collect()
}

/// Parse the user's selection. Empty input or `q` cancels (`Ok(None)`);
/// a number in range selects; anything else is an error with guidance.
pub fn parse_choice(input: &str, len: usize) -> Result<Option<usize>, String> {
    let trimmed = input.trim();
    if trimmed.is_empty() || trimmed.eq_ignore_ascii_case("q") {
        return Ok(None);
    }
    let index: usize = trimmed
        .parse()
        .map_err(|_| format!("'{trimmed}' is not an index (enter 0-{}, or q)", len - 1))?;
    if index >= len {
        return Err(format!("index {index} out of range (0-{})", len - 1));
    }
    Ok(Some(index))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(data: &[u8], pinned: bool) -> Entry {
        Entry {
            id: 1,
            at_ms: 0,
            pinned,
            data: data.to_vec(),
        }
    }

    #[test]
    fn row_shows_pin_marker_index_age_size_preview() {
        let row = format_row(3, &entry(b"hello world", true), 45_000);
        assert_eq!(row, "*   3   45s      11 B  hello world");
        let row = format_row(0, &entry(b"x", false), 0);
        assert!(
            row.starts_with("    0"),
            "unpinned rows keep alignment: {row:?}"
        );
    }

    #[test]
    fn menu_lists_newest_first() {
        let mut ring = Ring::new(10);
        ring.push(b"older".to_vec(), 0);
        ring.push(b"newer".to_vec(), 1000);
        let lines = menu_lines(&ring, 2000);
        assert_eq!(lines.len(), 2);
        assert!(lines[0].contains("newer"));
        assert!(lines[1].contains("older"));
    }

    #[test]
    fn choice_accepts_in_range_numbers() {
        assert_eq!(parse_choice("0", 3).unwrap(), Some(0));
        assert_eq!(parse_choice(" 2 \n", 3).unwrap(), Some(2));
    }

    #[test]
    fn choice_cancels_on_empty_or_q() {
        assert_eq!(parse_choice("", 3).unwrap(), None);
        assert_eq!(parse_choice("\n", 3).unwrap(), None);
        assert_eq!(parse_choice("Q", 3).unwrap(), None);
    }

    #[test]
    fn choice_rejects_bad_input_with_guidance() {
        let err = parse_choice("3", 3).unwrap_err();
        assert!(err.contains("0-2"), "err was: {err}");
        let err = parse_choice("abc", 5).unwrap_err();
        assert!(err.contains("0-4"), "err was: {err}");
        assert!(err.contains('q'), "err was: {err}");
    }
}

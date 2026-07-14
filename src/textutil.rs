//! Human-facing formatting: previews, sizes, ages.
//!
//! Pure functions only — every caller passes the clock in, so list output is
//! fully testable. Previews must be safe to print: clipboard content is
//! attacker-adjacent (it can contain escape sequences), so every control
//! character is neutralized before it reaches the user's terminal.

/// One-line, control-safe preview of clipboard bytes, at most `max_chars`
/// characters. Binary content gets a hex sketch instead of raw bytes.
pub fn preview(data: &[u8], max_chars: usize) -> String {
    match std::str::from_utf8(data) {
        Ok(text) => preview_text(text, max_chars),
        Err(_) => {
            let head: Vec<String> = data.iter().take(8).map(|b| format!("{b:02x}")).collect();
            let ellipsis = if data.len() > 8 { " …" } else { "" };
            format!("(binary: {}{})", head.join(" "), ellipsis)
        }
    }
}

fn preview_text(text: &str, max_chars: usize) -> String {
    let mut out = String::new();
    let mut truncated = false;
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        if out.chars().count() >= max_chars {
            truncated = true;
            break;
        }
        match c {
            // Stop at the first line break; note that more follows.
            '\n' | '\r' => {
                if chars.peek().is_some() || c == '\r' {
                    truncated = true;
                }
                break;
            }
            '\t' => out.push(' '),
            // Neutralize every other control char (incl. ESC — a stored
            // escape sequence must never execute when listed).
            c if c.is_control() => out.push('·'),
            c => out.push(c),
        }
    }
    if truncated {
        out.push('…');
    }
    out
}

/// Human-readable byte count: `0 B`, `914 B`, `1.2 KB`, `3.4 MB`, `1.0 GB`.
pub fn human_size(n: u64) -> String {
    const UNITS: [&str; 3] = ["KB", "MB", "GB"];
    if n < 1024 {
        return format!("{n} B");
    }
    let mut value = n as f64;
    let mut unit = "B";
    for u in UNITS {
        value /= 1024.0;
        unit = u;
        if value < 1024.0 {
            break;
        }
    }
    format!("{value:.1} {unit}")
}

/// Compact age: `now` (< 2 s), then `45s`, `12m`, `5h`, `9d`.
pub fn human_age(now_ms: u64, at_ms: u64) -> String {
    let secs = now_ms.saturating_sub(at_ms) / 1000;
    match secs {
        0..=1 => "now".to_string(),
        2..=59 => format!("{secs}s"),
        60..=3599 => format!("{}m", secs / 60),
        3600..=86_399 => format!("{}h", secs / 3600),
        _ => format!("{}d", secs / 86_400),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn preview_takes_first_line_and_marks_continuation() {
        assert_eq!(preview(b"one line", 40), "one line");
        assert_eq!(preview(b"first\nsecond", 40), "first…");
        assert_eq!(preview(b"trailing newline\n", 40), "trailing newline");
        assert_eq!(preview(b"crlf line\r\nnext", 40), "crlf line…");
        assert_eq!(preview(b"a\tb", 40), "a b", "tabs flatten to spaces");
    }

    #[test]
    fn preview_truncates_at_char_boundary() {
        // Multi-byte chars must not be split; count chars, not bytes.
        assert_eq!(preview("héllo wörld".as_bytes(), 5), "héllo…");
        assert_eq!(preview("日本語テキスト".as_bytes(), 3), "日本語…");
    }

    #[test]
    fn preview_neutralizes_escape_sequences() {
        // A stored ANSI sequence must never execute when the list renders.
        let p = preview(b"\x1b[31mred\x1b[0m", 40);
        assert!(!p.contains('\x1b'), "preview leaked ESC: {p:?}");
        assert!(p.contains("red"));
    }

    #[test]
    fn preview_sketches_binary() {
        let p = preview(&[0x89, 0x50, 0x4e, 0x47, 0xff], 40);
        assert_eq!(p, "(binary: 89 50 4e 47 ff)");
        let long = preview(&[0u8; 100], 40);
        assert!(long.ends_with('…'), "long binary gets ellipsis: {long}");
    }

    #[test]
    fn human_size_boundaries() {
        assert_eq!(human_size(0), "0 B");
        assert_eq!(human_size(1023), "1023 B");
        assert_eq!(human_size(1024), "1.0 KB");
        assert_eq!(human_size(1536), "1.5 KB");
        assert_eq!(human_size(1024 * 1024), "1.0 MB");
        assert_eq!(human_size(5 * 1024 * 1024 * 1024), "5.0 GB");
    }

    #[test]
    fn human_age_buckets_and_clock_skew() {
        let now = 1_000_000_000;
        assert_eq!(human_age(now, now), "now");
        assert_eq!(human_age(now, now - 1_500), "now");
        assert_eq!(human_age(now, now - 45_000), "45s");
        assert_eq!(human_age(now, now - 12 * 60_000), "12m");
        assert_eq!(human_age(now, now - 5 * 3_600_000), "5h");
        assert_eq!(human_age(now, now - 9 * 86_400_000), "9d");
        // An entry "from the future" (NTP step, copied state dir) must not
        // underflow — it is simply "now".
        assert_eq!(human_age(1000, 99_999_999), "now");
    }
}

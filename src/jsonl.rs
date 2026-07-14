//! Minimal flat-JSON codec for clipring's history records.
//!
//! Each history entry is one JSON object per line (JSONL). The objects are
//! flat — string, unsigned-integer, and boolean values only — so a full JSON
//! library would be 99% dead weight. This module implements exactly the
//! subset the store needs, strictly (bad lines are rejected, then skipped by
//! the store with a warning rather than corrupting the ring).

use std::collections::BTreeMap;

/// A flat JSON value.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Value {
    Str(String),
    UInt(u64),
    Bool(bool),
}

impl Value {
    pub fn as_str(&self) -> Option<&str> {
        match self {
            Value::Str(s) => Some(s),
            _ => None,
        }
    }
    pub fn as_uint(&self) -> Option<u64> {
        match self {
            Value::UInt(n) => Some(*n),
            _ => None,
        }
    }
    pub fn as_bool(&self) -> Option<bool> {
        match self {
            Value::Bool(b) => Some(*b),
            _ => None,
        }
    }
}

/// Serialize fields as a single-line JSON object, keys in the given order.
pub fn encode(fields: &[(&str, Value)]) -> String {
    let mut out = String::from("{");
    for (i, (k, v)) in fields.iter().enumerate() {
        if i > 0 {
            out.push(',');
        }
        out.push('"');
        out.push_str(&escape(k));
        out.push_str("\":");
        match v {
            Value::Str(s) => {
                out.push('"');
                out.push_str(&escape(s));
                out.push('"');
            }
            Value::UInt(n) => out.push_str(&n.to_string()),
            Value::Bool(b) => out.push_str(if *b { "true" } else { "false" }),
        }
    }
    out.push('}');
    out
}

/// Escape a string for embedding in a JSON string literal.
pub fn escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 => out.push_str(&format!("\\u{:04x}", c as u32)),
            c => out.push(c),
        }
    }
    out
}

/// Parse one flat JSON object. Unknown value types (nested objects, arrays,
/// null, floats, negative numbers) are errors — history records never
/// contain them, so their presence means the file is not ours or is damaged.
pub fn parse(line: &str) -> Result<BTreeMap<String, Value>, String> {
    let mut p = Parser {
        bytes: line.as_bytes(),
        pos: 0,
    };
    p.skip_ws();
    p.expect(b'{')?;
    let mut map = BTreeMap::new();
    p.skip_ws();
    if p.peek() == Some(b'}') {
        p.pos += 1;
    } else {
        loop {
            p.skip_ws();
            let key = p.string()?;
            p.skip_ws();
            p.expect(b':')?;
            p.skip_ws();
            let value = p.value()?;
            map.insert(key, value);
            p.skip_ws();
            match p.next() {
                Some(b',') => continue,
                Some(b'}') => break,
                other => return Err(format!("expected ',' or '}}', got {other:?}")),
            }
        }
    }
    p.skip_ws();
    if p.pos != p.bytes.len() {
        return Err(format!("trailing bytes at offset {}", p.pos));
    }
    Ok(map)
}

struct Parser<'a> {
    bytes: &'a [u8],
    pos: usize,
}

impl Parser<'_> {
    fn peek(&self) -> Option<u8> {
        self.bytes.get(self.pos).copied()
    }

    fn next(&mut self) -> Option<u8> {
        let b = self.peek();
        if b.is_some() {
            self.pos += 1;
        }
        b
    }

    fn skip_ws(&mut self) {
        while matches!(self.peek(), Some(b' ' | b'\t' | b'\n' | b'\r')) {
            self.pos += 1;
        }
    }

    fn expect(&mut self, want: u8) -> Result<(), String> {
        match self.next() {
            Some(b) if b == want => Ok(()),
            other => Err(format!("expected '{}', got {other:?}", want as char)),
        }
    }

    fn value(&mut self) -> Result<Value, String> {
        match self.peek() {
            Some(b'"') => Ok(Value::Str(self.string()?)),
            Some(b'0'..=b'9') => self.number(),
            Some(b't') => self.literal("true", Value::Bool(true)),
            Some(b'f') => self.literal("false", Value::Bool(false)),
            other => Err(format!("unsupported value starting with {other:?}")),
        }
    }

    fn literal(&mut self, word: &str, value: Value) -> Result<Value, String> {
        if self.bytes[self.pos..].starts_with(word.as_bytes()) {
            self.pos += word.len();
            Ok(value)
        } else {
            Err(format!("expected literal '{word}'"))
        }
    }

    fn number(&mut self) -> Result<Value, String> {
        let start = self.pos;
        while matches!(self.peek(), Some(b'0'..=b'9')) {
            self.pos += 1;
        }
        if matches!(self.peek(), Some(b'.' | b'e' | b'E' | b'-' | b'+')) {
            return Err("floats are not valid in history records".to_string());
        }
        std::str::from_utf8(&self.bytes[start..self.pos])
            .unwrap()
            .parse::<u64>()
            .map(Value::UInt)
            .map_err(|e| format!("bad integer: {e}"))
    }

    fn string(&mut self) -> Result<String, String> {
        self.expect(b'"')?;
        let mut out = String::new();
        loop {
            match self.next() {
                None => return Err("unterminated string".to_string()),
                Some(b'"') => return Ok(out),
                Some(b'\\') => match self.next() {
                    Some(b'"') => out.push('"'),
                    Some(b'\\') => out.push('\\'),
                    Some(b'/') => out.push('/'),
                    Some(b'b') => out.push('\u{8}'),
                    Some(b'f') => out.push('\u{c}'),
                    Some(b'n') => out.push('\n'),
                    Some(b'r') => out.push('\r'),
                    Some(b't') => out.push('\t'),
                    Some(b'u') => out.push(self.unicode_escape()?),
                    other => return Err(format!("bad escape {other:?}")),
                },
                Some(b) if b < 0x20 => return Err("raw control byte in string".to_string()),
                Some(b) => {
                    // Copy the full UTF-8 sequence this byte starts.
                    let len = utf8_len(b);
                    let end = self.pos - 1 + len;
                    let slice = self
                        .bytes
                        .get(self.pos - 1..end)
                        .ok_or("truncated UTF-8 sequence")?;
                    out.push_str(std::str::from_utf8(slice).map_err(|e| e.to_string())?);
                    self.pos = end;
                }
            }
        }
    }

    fn unicode_escape(&mut self) -> Result<char, String> {
        let hi = self.hex4()?;
        if (0xD800..0xDC00).contains(&hi) {
            // High surrogate: a \uXXXX low surrogate must follow.
            if self.next() != Some(b'\\') || self.next() != Some(b'u') {
                return Err("high surrogate without low surrogate".to_string());
            }
            let lo = self.hex4()?;
            if !(0xDC00..0xE000).contains(&lo) {
                return Err("invalid low surrogate".to_string());
            }
            let code = 0x10000 + ((hi - 0xD800) << 10) + (lo - 0xDC00);
            return char::from_u32(code).ok_or_else(|| "bad surrogate pair".to_string());
        }
        char::from_u32(hi).ok_or_else(|| format!("invalid codepoint \\u{hi:04x}"))
    }

    fn hex4(&mut self) -> Result<u32, String> {
        let mut n = 0u32;
        for _ in 0..4 {
            let b = self.next().ok_or("truncated \\u escape")?;
            let d = (b as char).to_digit(16).ok_or("non-hex in \\u escape")?;
            n = n * 16 + d;
        }
        Ok(n)
    }
}

fn utf8_len(first: u8) -> usize {
    match first {
        0x00..=0x7F => 1,
        0xC0..=0xDF => 2,
        0xE0..=0xEF => 3,
        _ => 4,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn encodes_all_value_kinds() {
        let line = encode(&[
            ("id", Value::UInt(7)),
            ("pin", Value::Bool(false)),
            ("data", Value::Str("aGk=".into())),
        ]);
        assert_eq!(line, r#"{"id":7,"pin":false,"data":"aGk="}"#);
    }

    #[test]
    fn escape_covers_control_and_quote_chars() {
        assert_eq!(escape("a\"b\\c\nd\te\u{1}"), "a\\\"b\\\\c\\nd\\te\\u0001");
    }

    #[test]
    fn round_trips_awkward_strings() {
        // Clipboard content is arbitrary text; quotes/newlines/emoji must
        // survive the store untouched.
        for s in [
            "",
            "plain",
            "line1\nline2",
            "\"quoted\" \\ back",
            "汉字 🎉",
            "\t\r",
        ] {
            let line = encode(&[("v", Value::Str(s.into()))]);
            let map = parse(&line).unwrap();
            assert_eq!(map["v"].as_str(), Some(s), "input {s:?}");
        }
    }

    #[test]
    fn parses_object_with_whitespace() {
        let map = parse(" { \"a\" : 1 , \"b\" : true } ").unwrap();
        assert_eq!(map["a"].as_uint(), Some(1));
        assert_eq!(map["b"].as_bool(), Some(true));
    }

    #[test]
    fn parses_empty_object() {
        assert!(parse("{}").unwrap().is_empty());
    }

    #[test]
    fn parses_unicode_escapes_and_surrogate_pairs() {
        let map = parse(r#"{"v":"é🎉"}"#).unwrap();
        assert_eq!(map["v"].as_str(), Some("é🎉"));
    }

    #[test]
    fn rejects_lone_high_surrogate() {
        assert!(parse(r#"{"v":"\ud83c"}"#).is_err());
    }

    #[test]
    fn rejects_damaged_or_foreign_json() {
        // Two records glued onto one line, nested values, null, floats,
        // negatives, torn strings: all mean the file was damaged or is not
        // ours. The parser must error, never silently guess.
        for bad in [
            r#"{"a":1}{"b":2}"#,
            r#"{"a":{}}"#,
            r#"{"a":[1]}"#,
            r#"{"a":null}"#,
            r#"{"a":1.5}"#,
            r#"{"a":-3}"#,
            r#"{"a":"oops}"#,
        ] {
            assert!(parse(bad).is_err(), "accepted damaged line: {bad}");
        }
    }

    #[test]
    fn u64_boundary_survives() {
        let line = encode(&[("t", Value::UInt(u64::MAX))]);
        assert_eq!(parse(&line).unwrap()["t"].as_uint(), Some(u64::MAX));
    }
}

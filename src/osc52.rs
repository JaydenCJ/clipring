//! OSC 52 sequence construction, multiplexer wrapping, and extraction.
//!
//! OSC 52 is the escape sequence that asks the *terminal emulator* — not the
//! machine the shell runs on — to set a selection. Because the sequence rides
//! the terminal byte stream, it works unchanged across SSH hops. The catch is
//! multiplexers: tmux and GNU screen consume unknown escapes unless the
//! sequence is wrapped in their respective passthrough envelopes. This module
//! owns both directions: building wrapped sequences, and extracting/decoding
//! OSC 52 payloads back out of a raw byte stream (unwrapping as needed).

use crate::base64;

const ESC: u8 = 0x1b;
const BEL: u8 = 0x07;

/// Which terminal selection the sequence targets.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Selection {
    /// The system clipboard (`c`) — what Ctrl-V pastes.
    Clipboard,
    /// The X11 primary selection (`p`) — what middle-click pastes.
    Primary,
}

impl Selection {
    pub fn param(self) -> &'static str {
        match self {
            Selection::Clipboard => "c",
            Selection::Primary => "p",
        }
    }
}

/// Passthrough envelope required for the sequence to reach the terminal.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Wrap {
    /// No multiplexer in the way; emit the bare OSC 52 sequence.
    None,
    /// tmux DCS passthrough (`ESC Ptmux;` … `ESC \`, inner ESCs doubled).
    /// Requires `set -g allow-passthrough on` in tmux ≥ 3.3.
    Tmux,
    /// GNU screen: the sequence is split into ≤ 768-byte DCS chunks, because
    /// screen truncates long device control strings.
    Screen,
}

impl Wrap {
    pub fn name(self) -> &'static str {
        match self {
            Wrap::None => "none",
            Wrap::Tmux => "tmux",
            Wrap::Screen => "screen",
        }
    }
}

/// Decide the wrap from the environment: an active `$TMUX` wins, then a
/// `$TERM` beginning with `screen` (tmux also uses screen-* TERMs, which is
/// why `$TMUX` is checked first).
pub fn detect_wrap(tmux: Option<&str>, term: Option<&str>) -> Wrap {
    if tmux.is_some_and(|v| !v.is_empty()) {
        return Wrap::Tmux;
    }
    if term.is_some_and(|v| v.starts_with("screen")) {
        return Wrap::Screen;
    }
    Wrap::None
}

/// Build the (possibly wrapped) OSC 52 sequence that copies `data`.
pub fn sequence(selection: Selection, data: &[u8], wrap: Wrap) -> Vec<u8> {
    let bare = bare_sequence(selection, &base64::encode(data));
    match wrap {
        Wrap::None => bare,
        Wrap::Tmux => wrap_tmux(&bare),
        Wrap::Screen => wrap_screen(&bare),
    }
}

/// Length in bytes of the base64 payload `data` would produce — the number
/// terminals apply their OSC 52 size caps to.
pub fn payload_len(data: &[u8]) -> usize {
    data.len().div_ceil(3) * 4
}

fn bare_sequence(selection: Selection, b64: &str) -> Vec<u8> {
    let mut seq = Vec::with_capacity(b64.len() + 8);
    seq.extend_from_slice(b"\x1b]52;");
    seq.extend_from_slice(selection.param().as_bytes());
    seq.push(b';');
    seq.extend_from_slice(b64.as_bytes());
    seq.push(BEL);
    seq
}

/// tmux passthrough: `ESC Ptmux;` + sequence with every ESC doubled + `ESC \`.
fn wrap_tmux(seq: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(seq.len() + 16);
    out.extend_from_slice(b"\x1bPtmux;");
    for &b in seq {
        if b == ESC {
            out.push(ESC);
        }
        out.push(b);
    }
    out.extend_from_slice(b"\x1b\\");
    out
}

/// screen passthrough: the sequence is cut into chunks of at most 768 bytes,
/// each wrapped in its own DCS (`ESC P` … `ESC \`). screen forwards DCS
/// content verbatim but drops overlong ones, hence the chunking.
fn wrap_screen(seq: &[u8]) -> Vec<u8> {
    const CHUNK: usize = 768;
    let mut out = Vec::with_capacity(seq.len() + 4 * seq.len().div_ceil(CHUNK));
    for chunk in seq.chunks(CHUNK) {
        out.extend_from_slice(b"\x1bP");
        out.extend_from_slice(chunk);
        out.extend_from_slice(b"\x1b\\");
    }
    out
}

/// One OSC 52 set found in a byte stream.
#[derive(Debug, PartialEq, Eq)]
pub struct Capture {
    /// The raw selection parameter (`c`, `p`, `cp`, or empty).
    pub selection: String,
    /// The decoded clipboard bytes.
    pub data: Vec<u8>,
}

/// Extract every decodable OSC 52 *set* from `input`, unwrapping tmux and
/// screen passthrough envelopes first. Query sequences (`?` payloads) and
/// payloads that fail base64 decoding are skipped — the caller wants
/// clipboard contents, not protocol chatter.
pub fn extract(input: &[u8]) -> Vec<Capture> {
    let stream = unwrap_passthrough(input);
    let mut captures = Vec::new();
    let mut i = 0;
    while let Some(start) = find(&stream[i..], b"\x1b]52;") {
        let body_start = i + start + 5;
        let Some((selection, payload, end)) = split_osc_body(&stream[body_start..]) else {
            break; // unterminated sequence at end of stream
        };
        if payload != b"?" {
            if let Ok(text) = std::str::from_utf8(payload) {
                if let Ok(data) = base64::decode(text) {
                    captures.push(Capture {
                        selection: String::from_utf8_lossy(selection).into_owned(),
                        data,
                    });
                }
            }
        }
        i = body_start + end;
    }
    captures
}

/// Split an OSC 52 body (`<sel>;<payload><terminator>`) and return
/// (selection, payload, bytes consumed). Terminator is BEL or ST (`ESC \`).
fn split_osc_body(body: &[u8]) -> Option<(&[u8], &[u8], usize)> {
    let semi = body.iter().position(|&b| b == b';')?;
    let rest = &body[semi + 1..];
    let mut j = 0;
    while j < rest.len() {
        match rest[j] {
            BEL => return Some((&body[..semi], &rest[..j], semi + 1 + j + 1)),
            ESC if rest.get(j + 1) == Some(&b'\\') => {
                return Some((&body[..semi], &rest[..j], semi + 1 + j + 2));
            }
            _ => j += 1,
        }
    }
    None
}

/// Remove tmux and screen DCS envelopes from a byte stream. tmux content has
/// its ESCs doubled and must be un-doubled; screen chunks are verbatim. Bytes
/// outside any envelope pass through unchanged.
fn unwrap_passthrough(input: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(input.len());
    let mut i = 0;
    while i < input.len() {
        if input[i] == ESC && input.get(i + 1) == Some(&b'P') {
            if input[i + 2..].starts_with(b"tmux;") {
                i = unwrap_tmux_body(input, i + 7, &mut out);
            } else {
                i = unwrap_screen_body(input, i + 2, &mut out);
            }
        } else {
            out.push(input[i]);
            i += 1;
        }
    }
    out
}

/// Copy a tmux passthrough body starting at `from`, un-doubling ESCs, until
/// the ST terminator. Returns the index just past the envelope.
fn unwrap_tmux_body(input: &[u8], from: usize, out: &mut Vec<u8>) -> usize {
    let mut i = from;
    while i < input.len() {
        if input[i] == ESC {
            match input.get(i + 1) {
                Some(&ESC) => {
                    out.push(ESC);
                    i += 2;
                }
                Some(&b'\\') => return i + 2,
                _ => {
                    out.push(ESC);
                    i += 1;
                }
            }
        } else {
            out.push(input[i]);
            i += 1;
        }
    }
    i
}

/// Copy a screen DCS chunk body verbatim until ST. Returns the index just
/// past the envelope.
fn unwrap_screen_body(input: &[u8], from: usize, out: &mut Vec<u8>) -> usize {
    let mut i = from;
    while i < input.len() {
        if input[i] == ESC && input.get(i + 1) == Some(&b'\\') {
            return i + 2;
        }
        out.push(input[i]);
        i += 1;
    }
    i
}

fn find(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    haystack.windows(needle.len()).position(|w| w == needle)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bare_sequence_has_exact_frame_per_selection() {
        // The frame bytes are the protocol; any deviation breaks terminals.
        let seq = sequence(Selection::Clipboard, b"hi", Wrap::None);
        assert_eq!(seq, b"\x1b]52;c;aGk=\x07");
        let seq = sequence(Selection::Primary, b"hi", Wrap::None);
        assert_eq!(seq, b"\x1b]52;p;aGk=\x07");
    }

    #[test]
    fn tmux_wrap_doubles_inner_escapes() {
        let seq = sequence(Selection::Clipboard, b"x", Wrap::Tmux);
        assert!(seq.starts_with(b"\x1bPtmux;"));
        assert!(seq.ends_with(b"\x1b\\"));
        // The inner sequence's leading ESC must appear doubled.
        let body = &seq[7..seq.len() - 2];
        assert!(body.starts_with(b"\x1b\x1b]52;c;"));
        // BEL is not an ESC and must not be doubled.
        assert_eq!(body.iter().filter(|&&b| b == 0x07).count(), 1);
    }

    #[test]
    fn screen_wrap_chunks_long_sequences() {
        // 2000 raw bytes -> ~2668-byte b64 sequence -> at least 4 DCS chunks.
        let seq = sequence(Selection::Clipboard, &vec![b'a'; 2000], Wrap::Screen);
        let chunks = seq.windows(2).filter(|w| w == b"\x1bP").count();
        assert!(chunks >= 4, "expected >=4 chunks, got {chunks}");
        // No chunk body may exceed 768 bytes.
        for part in split_dcs_bodies(&seq) {
            assert!(part.len() <= 768, "chunk of {} bytes", part.len());
        }
    }

    fn split_dcs_bodies(seq: &[u8]) -> Vec<Vec<u8>> {
        let mut bodies = Vec::new();
        let mut i = 0;
        while i + 1 < seq.len() {
            assert_eq!(&seq[i..i + 2], b"\x1bP", "chunk must start with DCS");
            let mut j = i + 2;
            while !(seq[j] == 0x1b && seq[j + 1] == b'\\') {
                j += 1;
            }
            bodies.push(seq[i + 2..j].to_vec());
            i = j + 2;
        }
        bodies
    }

    #[test]
    fn detect_prefers_tmux_over_screen_term() {
        // Inside tmux, TERM is usually screen-256color; $TMUX must win.
        assert_eq!(
            detect_wrap(Some("/tmp/tmux-0/default,42,0"), Some("screen-256color")),
            Wrap::Tmux
        );
        assert_eq!(
            detect_wrap(None, Some("screen.xterm-256color")),
            Wrap::Screen
        );
        assert_eq!(detect_wrap(None, Some("xterm-256color")), Wrap::None);
        assert_eq!(detect_wrap(Some(""), Some("xterm")), Wrap::None);
        assert_eq!(detect_wrap(None, None), Wrap::None);
    }

    #[test]
    fn extract_reads_bare_sequence() {
        let caps = extract(b"noise\x1b]52;c;aGVsbG8=\x07more");
        assert_eq!(caps.len(), 1);
        assert_eq!(caps[0].selection, "c");
        assert_eq!(caps[0].data, b"hello");
    }

    #[test]
    fn extract_accepts_st_terminator() {
        let caps = extract(b"\x1b]52;p;aGk=\x1b\\");
        assert_eq!(caps[0].selection, "p");
        assert_eq!(caps[0].data, b"hi");
    }

    #[test]
    fn extract_round_trips_tmux_wrap() {
        let data = b"tmux carried this".to_vec();
        let seq = sequence(Selection::Clipboard, &data, Wrap::Tmux);
        let caps = extract(&seq);
        assert_eq!(caps.len(), 1);
        assert_eq!(caps[0].data, data);
    }

    #[test]
    fn extract_round_trips_screen_chunks() {
        // The OSC frame itself is split across DCS chunk boundaries; the
        // extractor must reassemble it before parsing.
        let data = vec![7u8; 3000];
        let seq = sequence(Selection::Clipboard, &data, Wrap::Screen);
        let caps = extract(&seq);
        assert_eq!(caps.len(), 1);
        assert_eq!(caps[0].data, data);
    }

    #[test]
    fn extract_round_trips_binary_data() {
        let data: Vec<u8> = (0u8..=255).collect();
        for wrap in [Wrap::None, Wrap::Tmux, Wrap::Screen] {
            let seq = sequence(Selection::Clipboard, &data, wrap);
            assert_eq!(extract(&seq)[0].data, data, "wrap {wrap:?}");
        }
    }

    #[test]
    fn extract_finds_multiple_sets_in_order() {
        let mut stream = sequence(Selection::Clipboard, b"first", Wrap::None);
        stream.extend_from_slice(b"\x1b[1mother escapes\x1b[0m");
        stream.extend(sequence(Selection::Primary, b"second", Wrap::Tmux));
        let caps = extract(&stream);
        assert_eq!(caps.len(), 2);
        assert_eq!(caps[0].data, b"first");
        assert_eq!(caps[1].data, b"second");
        assert_eq!(caps[1].selection, "p");
    }

    #[test]
    fn extract_skips_protocol_chatter_and_damage() {
        // `?` asks the terminal to *report* the clipboard (no data), invalid
        // base64 is noise, and a truncated tail (a cut-off recording) must
        // not panic or produce a bogus entry.
        let caps = extract(b"\x1b]52;c;?\x07\x1b]52;c;eWVz\x07");
        assert_eq!(caps.len(), 1);
        assert_eq!(caps[0].data, b"yes");
        assert!(extract(b"\x1b]52;c;!not-base64!\x07").is_empty());
        assert!(extract(b"\x1b]52;c;aGVsbG8=").is_empty());
    }

    #[test]
    fn payload_len_matches_encoded_length() {
        for n in [0usize, 1, 2, 3, 100, 1000] {
            let data = vec![0u8; n];
            assert_eq!(payload_len(&data), base64::encode(&data).len(), "n={n}");
        }
    }
}

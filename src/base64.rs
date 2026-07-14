//! RFC 4648 standard-alphabet base64, encode and decode.
//!
//! OSC 52 carries its payload as base64. Implementing the codec here keeps
//! the crate dependency-free, and lets the decoder be deliberately lenient
//! about interleaved whitespace — some terminals and multiplexers fold long
//! sequences across lines before they reach us.

const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";

/// Encode `data` as padded standard base64.
pub fn encode(data: &[u8]) -> String {
    let mut out = String::with_capacity(data.len().div_ceil(3) * 4);
    for chunk in data.chunks(3) {
        let b0 = chunk[0] as u32;
        let b1 = *chunk.get(1).unwrap_or(&0) as u32;
        let b2 = *chunk.get(2).unwrap_or(&0) as u32;
        let n = (b0 << 16) | (b1 << 8) | b2;
        out.push(ALPHABET[(n >> 18) as usize & 63] as char);
        out.push(ALPHABET[(n >> 12) as usize & 63] as char);
        out.push(if chunk.len() > 1 {
            ALPHABET[(n >> 6) as usize & 63] as char
        } else {
            '='
        });
        out.push(if chunk.len() > 2 {
            ALPHABET[n as usize & 63] as char
        } else {
            '='
        });
    }
    out
}

/// Decode standard base64. ASCII whitespace is skipped; `=` padding is
/// accepted but not required (unpadded tails of 2 or 3 sextets decode fine).
/// Any other non-alphabet byte is an error.
pub fn decode(s: &str) -> Result<Vec<u8>, String> {
    let mut out = Vec::with_capacity(s.len() / 4 * 3);
    let mut acc: u32 = 0;
    let mut nsext = 0u8;
    let mut seen_pad = false;
    for (i, b) in s.bytes().enumerate() {
        if b.is_ascii_whitespace() {
            continue;
        }
        if b == b'=' {
            seen_pad = true;
            continue;
        }
        if seen_pad {
            return Err(format!("data after '=' padding at byte {i}"));
        }
        let v = sextet(b).ok_or_else(|| format!("invalid base64 byte 0x{b:02x} at {i}"))?;
        acc = (acc << 6) | v as u32;
        nsext += 1;
        if nsext == 4 {
            out.push((acc >> 16) as u8);
            out.push((acc >> 8) as u8);
            out.push(acc as u8);
            acc = 0;
            nsext = 0;
        }
    }
    match nsext {
        0 => {}
        1 => return Err("truncated base64: lone trailing sextet".to_string()),
        2 => out.push((acc >> 4) as u8),
        3 => {
            out.push((acc >> 10) as u8);
            out.push((acc >> 2) as u8);
        }
        _ => unreachable!(),
    }
    Ok(out)
}

fn sextet(b: u8) -> Option<u8> {
    match b {
        b'A'..=b'Z' => Some(b - b'A'),
        b'a'..=b'z' => Some(b - b'a' + 26),
        b'0'..=b'9' => Some(b - b'0' + 52),
        b'+' => Some(62),
        b'/' => Some(63),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rfc4648_vectors_encode_and_decode() {
        // The canonical RFC 4648 §10 test vectors pin the alphabet and
        // padding behavior exactly, in both directions.
        for (raw, b64) in [
            (&b""[..], ""),
            (b"f", "Zg=="),
            (b"fo", "Zm8="),
            (b"foo", "Zm9v"),
            (b"foob", "Zm9vYg=="),
            (b"fooba", "Zm9vYmE="),
            (b"foobar", "Zm9vYmFy"),
        ] {
            assert_eq!(encode(raw), b64);
            assert_eq!(decode(b64).unwrap(), raw);
        }
    }

    #[test]
    fn decodes_unpadded_input() {
        // Terminals are not required to pad; the decoder must not be either.
        assert_eq!(decode("Zg").unwrap(), b"f");
        assert_eq!(decode("Zm8").unwrap(), b"fo");
    }

    #[test]
    fn decode_skips_interleaved_whitespace() {
        // screen re-flows long DCS payloads; whitespace must be transparent.
        assert_eq!(decode("Zm 9v\nYm\r\nFy").unwrap(), b"foobar");
    }

    #[test]
    fn round_trips_all_byte_values() {
        let data: Vec<u8> = (0u8..=255).collect();
        assert_eq!(decode(&encode(&data)).unwrap(), data);
    }

    #[test]
    fn round_trips_every_tail_length() {
        // Padding bugs hide in the tail; exercise lengths 0..=6 explicitly.
        for n in 0..=6usize {
            let data = vec![0xA5u8; n];
            assert_eq!(decode(&encode(&data)).unwrap(), data, "len {n}");
        }
    }

    #[test]
    fn rejects_malformed_input() {
        // Invalid byte (with its offset named), a lone trailing sextet, and
        // data after '=' padding are all corruption, never silently decoded.
        let err = decode("Zm9*").unwrap_err();
        assert!(err.contains("0x2a"), "err was: {err}");
        assert!(decode("Zm9vY").is_err());
        assert!(decode("Zg==Zg==").is_err());
    }

    #[test]
    fn encode_output_is_ascii_and_4_aligned() {
        let s = encode(&[0, 159, 146, 150]);
        assert!(s.is_ascii());
        assert_eq!(s.len() % 4, 0);
    }
}

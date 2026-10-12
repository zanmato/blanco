//! Binary cell values.
//!
//! Result rows carry every value as text, so the bytes of a binary column
//! travel as `0x` followed by lowercase hex. That form is lossless, every
//! driver produces it, and [`decode`] turns it back into the bytes for the
//! viewer, a file, an export or a literal in the backend's own spelling
//! ([`crate::Dialect::binary_literal`]).

const PREFIX: &str = "0x";

/// The cell text for `bytes`.
pub fn encode(bytes: &[u8]) -> String {
    const DIGITS: &[u8; 16] = b"0123456789abcdef";
    let mut text = String::with_capacity(PREFIX.len() + bytes.len() * 2);
    text.push_str(PREFIX);
    for byte in bytes {
        text.push(DIGITS[usize::from(byte >> 4)] as char);
        text.push(DIGITS[usize::from(byte & 0x0f)] as char);
    }
    text
}

/// The hex digits of a value written as `0x…` or PostgreSQL's `\x…`, when it
/// is one. Text without such a prefix is not hex: `cafe` is four characters.
fn prefixed_digits(text: &str) -> Option<&str> {
    let text = text.trim();
    text.strip_prefix(PREFIX)
        .or_else(|| text.strip_prefix("0X"))
        .or_else(|| text.strip_prefix("\\x"))
}

/// Whether `text` is written as hex, valid or not.
pub fn is_hex_notation(text: &str) -> bool {
    prefixed_digits(text).is_some()
}

/// The bytes of a value in hex notation. `None` when it has no hex prefix or
/// the digits after it are not whole bytes.
pub fn decode(text: &str) -> Option<Vec<u8>> {
    decode_digits(prefixed_digits(text)?)
}

/// As [`decode`], and also accepts the digits alone, for places that only
/// take binary, such as a filter on a binary column.
pub fn decode_lenient(text: &str) -> Option<Vec<u8>> {
    decode_digits(prefixed_digits(text).unwrap_or(text.trim()))
}

fn decode_digits(digits: &str) -> Option<Vec<u8>> {
    let digits = digits.as_bytes();
    if digits.len() % 2 != 0 {
        return None;
    }
    digits
        .chunks_exact(2)
        .map(|pair| Some(nibble(pair[0])? << 4 | nibble(pair[1])?))
        .collect()
}

fn nibble(digit: u8) -> Option<u8> {
    match digit {
        b'0'..=b'9' => Some(digit - b'0'),
        b'a'..=b'f' => Some(digit - b'a' + 10),
        b'A'..=b'F' => Some(digit - b'A' + 10),
        _ => None,
    }
}

/// How many bytes a value in hex notation holds, without decoding it.
pub fn byte_length(text: &str) -> Option<usize> {
    let digits = prefixed_digits(text)?;
    (digits.len() % 2 == 0).then_some(digits.len() / 2)
}

/// The digits of a value in hex notation, lowercased and without the prefix,
/// when they are whole bytes.
pub fn hex_digits(text: &str) -> Option<String> {
    let digits = prefixed_digits(text)?;
    (digits.len() % 2 == 0 && digits.bytes().all(|digit| nibble(digit).is_some()))
        .then(|| digits.to_ascii_lowercase())
}

/// Base64 with the standard alphabet and padding.
pub fn to_base64(bytes: &[u8]) -> String {
    const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut text = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let group = u32::from(chunk[0]) << 16
            | u32::from(chunk.get(1).copied().unwrap_or(0)) << 8
            | u32::from(chunk.get(2).copied().unwrap_or(0));
        let sextet = |shift: u32| ALPHABET[((group >> shift) & 0x3f) as usize] as char;
        text.push(sextet(18));
        text.push(sextet(12));
        text.push(if chunk.len() > 1 { sextet(6) } else { '=' });
        text.push(if chunk.len() > 2 { sextet(0) } else { '=' });
    }
    text
}

/// A size for people: `512 B`, `1.4 KB`, `3.2 MB`.
pub fn format_size(bytes: usize) -> String {
    const UNITS: [&str; 4] = ["KB", "MB", "GB", "TB"];
    if bytes < 1024 {
        return format!("{bytes} B");
    }
    let mut size = bytes as f64 / 1024.0;
    let mut unit = 0;
    while size >= 1024.0 && unit + 1 < UNITS.len() {
        size /= 1024.0;
        unit += 1;
    }
    format!("{size:.1} {}", UNITS[unit])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trips_every_byte() {
        let bytes: Vec<u8> = (0..=255).collect();
        let text = encode(&bytes);
        assert!(text.starts_with("0x000102"));
        assert_eq!(decode(&text), Some(bytes));
        assert_eq!(byte_length(&text), Some(256));
        assert_eq!(encode(&[]), "0x");
        assert_eq!(decode("0x"), Some(Vec::new()));
    }

    #[test]
    fn only_prefixed_text_is_hex() {
        assert_eq!(decode("\\xDEADbeef"), Some(vec![0xde, 0xad, 0xbe, 0xef]));
        assert_eq!(decode(" 0XFF "), Some(vec![0xff]));
        assert_eq!(decode("cafe"), None);
        assert!(!is_hex_notation("cafe"));
        assert_eq!(decode_lenient("cafe"), Some(vec![0xca, 0xfe]));
        // Written as hex, but not whole bytes or not digits.
        assert!(is_hex_notation("0xabc"));
        assert_eq!(decode("0xabc"), None);
        assert_eq!(decode("0xzz"), None);
        assert_eq!(hex_digits("0xAB"), Some("ab".to_string()));
        assert_eq!(hex_digits("0xA"), None);
    }

    #[test]
    fn base64_pads() {
        assert_eq!(to_base64(b""), "");
        assert_eq!(to_base64(b"f"), "Zg==");
        assert_eq!(to_base64(b"fo"), "Zm8=");
        assert_eq!(to_base64(b"foo"), "Zm9v");
        assert_eq!(to_base64(b"foobar"), "Zm9vYmFy");
        assert_eq!(to_base64(&[0xfb, 0xff]), "+/8=");
    }

    #[test]
    fn sizes() {
        assert_eq!(format_size(0), "0 B");
        assert_eq!(format_size(1023), "1023 B");
        assert_eq!(format_size(1536), "1.5 KB");
        assert_eq!(format_size(5 * 1024 * 1024), "5.0 MB");
    }
}

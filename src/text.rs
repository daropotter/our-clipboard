//! Content rules shared by clipboard adapters and the future synchronization core.
use std::io::Read;

/// Decimal megabytes of UTF-8 payload, excluding protocol overhead.
pub const MAX_TEXT_BYTES: usize = 10_000_000;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TextError {
    Empty,
    ContainsNul,
    TooLarge,
    InvalidUtf8,
    ReadFailed,
}

impl TextError {
    /// Safe to display: never includes clipboard contents or backend error details.
    pub fn message(self) -> &'static str {
        match self {
            Self::Empty => "Clipboard text is empty; skipped.",
            Self::ContainsNul => "Clipboard text contains NUL; skipped.",
            Self::TooLarge => "Clipboard text exceeds 10,000,000 UTF-8 bytes; skipped.",
            Self::InvalidUtf8 => "Clipboard text is not valid UTF-8; skipped.",
            Self::ReadFailed => "Clipboard read failed.",
        }
    }
}

pub fn validate(text: &str) -> Result<(), TextError> {
    if text.len() > MAX_TEXT_BYTES {
        Err(TextError::TooLarge)
    } else if text.is_empty() {
        Err(TextError::Empty)
    } else if text.contains('\0') {
        Err(TextError::ContainsNul)
    } else {
        Ok(())
    }
}

/// Read at most the limit plus one byte to detect oversize without truncation.
/// This bounds allocation, not time spent waiting on the clipboard owner.
pub fn read_bounded(reader: impl Read) -> Result<String, TextError> {
    let mut bytes = Vec::new();
    reader
        .take((MAX_TEXT_BYTES + 1) as u64)
        .read_to_end(&mut bytes)
        .map_err(|_| TextError::ReadFailed)?;
    if bytes.len() > MAX_TEXT_BYTES {
        return Err(TextError::TooLarge);
    }
    let text = String::from_utf8(bytes).map_err(|_| TextError::InvalidUtf8)?;
    validate(&text)?;
    Ok(text)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io;

    #[test]
    fn preserves_whitespace_newlines_and_unicode_without_normalization() {
        for text in [" \t\n", "first\r\nsecond\n", "Zażółć 🦀\ne\u{301}"] {
            let observed = read_bounded(text.as_bytes()).unwrap();
            assert!(observed == text, "content changed (omitted)");
        }
    }

    #[test]
    fn rejects_empty_nul_and_invalid_utf8() {
        assert_eq!(validate(""), Err(TextError::Empty));
        assert_eq!(validate("before\0after"), Err(TextError::ContainsNul));
        assert!(matches!(
            read_bounded(&[0xff][..]),
            Err(TextError::InvalidUtf8)
        ));
    }

    #[test]
    fn decimal_byte_limit_is_inclusive_and_not_a_character_limit() {
        let exact = "🦀".repeat(MAX_TEXT_BYTES / 4);
        assert_eq!(validate(&exact), Ok(()));
        let too_large = exact + "a";
        assert_eq!(validate(&too_large), Err(TextError::TooLarge));
        assert!(matches!(
            read_bounded(too_large.as_bytes()),
            Err(TextError::TooLarge)
        ));
    }

    #[test]
    fn oversized_stream_stops_after_limit_plus_one_byte() {
        struct Stream(usize);
        impl Read for Stream {
            fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
                self.0 += buf.len();
                assert!(self.0 <= MAX_TEXT_BYTES + 1, "read exceeded bound");
                buf.fill(b'x');
                Ok(buf.len())
            }
        }
        let mut stream = Stream(0);
        assert!(matches!(
            read_bounded(&mut stream),
            Err(TextError::TooLarge)
        ));
        assert_eq!(stream.0, MAX_TEXT_BYTES + 1);
    }

    #[test]
    fn stream_errors_are_distinct_from_invalid_content() {
        struct Broken;
        impl Read for Broken {
            fn read(&mut self, _: &mut [u8]) -> io::Result<usize> {
                Err(io::Error::other("synthetic failure"))
            }
        }
        assert!(matches!(read_bounded(Broken), Err(TextError::ReadFailed)));
    }
}

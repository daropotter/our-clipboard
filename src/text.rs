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
    ReadTimedOut,
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
            Self::ReadTimedOut => "Clipboard payload read timed out.",
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
    finish_text(bytes)
}

fn finish_text(bytes: Vec<u8>) -> Result<String, TextError> {
    let text = String::from_utf8(bytes).map_err(|_| TextError::InvalidUtf8)?;
    validate(&text)?;
    Ok(text)
}

/// Bound the payload pipe by both bytes and a fixed deadline (not an idle timeout).
/// Compositor negotiation before obtaining this pipe is outside this deadline.
#[cfg(target_os = "linux")]
pub fn read_bounded_fd(
    reader: impl Read + std::os::fd::AsFd,
    timeout: std::time::Duration,
) -> Result<String, TextError> {
    finish_text(read_bytes_bounded_fd(reader, MAX_TEXT_BYTES, timeout)?)
}

#[cfg(target_os = "linux")]
pub(crate) fn read_bytes_bounded_fd(
    mut reader: impl Read + std::os::fd::AsFd,
    limit: usize,
    timeout: std::time::Duration,
) -> Result<Vec<u8>, TextError> {
    use rustix::event::{PollFd, PollFlags, Timespec, poll};
    use std::time::Instant;
    let deadline = Instant::now()
        .checked_add(timeout)
        .ok_or(TextError::ReadFailed)?;
    let max_read = limit.checked_add(1).ok_or(TextError::ReadFailed)?;
    let mut bytes = Vec::new();
    let mut chunk = [0; 8192];
    loop {
        let remaining = deadline
            .checked_duration_since(Instant::now())
            .ok_or(TextError::ReadTimedOut)?;
        let timespec = Timespec::try_from(remaining).map_err(|_| TextError::ReadFailed)?;
        let readiness = {
            let mut fds = [PollFd::new(&reader, PollFlags::IN)];
            poll(&mut fds, Some(&timespec))
        };
        match readiness {
            Ok(0) => return Err(TextError::ReadTimedOut),
            Ok(_) => {}
            Err(rustix::io::Errno::INTR) => continue,
            Err(_) => return Err(TextError::ReadFailed),
        }
        let available = chunk.len().min(max_read - bytes.len());
        let count = match reader.read(&mut chunk[..available]) {
            Ok(count) => count,
            Err(error) if error.kind() == std::io::ErrorKind::Interrupted => continue,
            Err(_) => return Err(TextError::ReadFailed),
        };
        if count == 0 {
            return Ok(bytes);
        }
        bytes.extend_from_slice(&chunk[..count]);
        if bytes.len() > limit {
            return Err(TextError::TooLarge);
        }
    }
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
        let observed = read_bounded(exact.as_bytes()).unwrap();
        assert!(
            observed == exact,
            "inclusive-limit read changed contents (omitted)"
        );
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

#[cfg(all(test, target_os = "linux"))]
mod fd_tests {
    use super::*;
    use std::{io::Write, os::unix::net::UnixStream, time::Duration};

    #[test]
    fn stalled_owner_times_out_without_returning_partial_text() {
        let (reader, mut writer) = UnixStream::pair().unwrap();
        writer.write_all(b"incomplete synthetic payload").unwrap();
        // Keep the writer open: EOF never arrives.
        assert!(matches!(
            read_bounded_fd(reader, Duration::from_millis(50)),
            Err(TextError::ReadTimedOut)
        ));
        drop(writer);
    }

    #[test]
    fn payload_eof_preserves_unicode_and_whitespace() {
        let (reader, mut writer) = UnixStream::pair().unwrap();
        let sample = " 🦀\r\nsecond line\t";
        writer.write_all(sample.as_bytes()).unwrap();
        drop(writer);
        let observed = read_bounded_fd(reader, Duration::from_secs(1)).unwrap();
        assert!(observed == sample, "contents changed (omitted)");
    }

    #[test]
    fn pipe_limit_is_inclusive_and_oversize_is_never_truncated() {
        for (bytes, accepted) in [(32, true), (33, false)] {
            let (reader, mut writer) = UnixStream::pair().unwrap();
            writer.write_all(&vec![b'x'; bytes]).unwrap();
            drop(writer);
            let result = read_bytes_bounded_fd(reader, 32, Duration::from_secs(1));
            if accepted {
                assert!(result.is_ok());
            } else {
                assert!(matches!(result, Err(TextError::TooLarge)));
            }
        }
    }
}

//! Local change/application prototype. No network, clipboard history, or content logging.
use crate::{clipboard::TextClipboard, origin::TaggedClipboard, watch::Watcher};
use std::time::{Duration, Instant};

/// Intentionally has no Debug implementation: it owns clipboard resources and a marker.
pub struct ClipboardSession {
    clipboard: TextClipboard,
    tagged: TaggedClipboard,
    watcher: Watcher,
    marker: [u8; 32],
}

impl ClipboardSession {
    pub fn new() -> Result<Self, &'static str> {
        let clipboard = TextClipboard::new()?;
        let tagged = TaggedClipboard::new(clipboard.backend_name())?;
        let watcher = Watcher::new()?;
        let mut marker = [0; 32];
        getrandom::fill(&mut marker).map_err(|_| "Could not create a clipboard session marker.")?;
        Ok(Self {
            clipboard,
            tagged,
            watcher,
            marker,
        })
    }

    /// Rebaseline notifications after Idle without reading text or releasing ownership.
    /// Changes made while paused become the baseline, never new local publications.
    pub fn rebaseline_observer(&mut self) -> Result<(), &'static str> {
        self.watcher = Watcher::new()?;
        Ok(())
    }

    /// Apply a received value with a local marker, never by comparing text hashes.
    pub fn apply_received_text(&mut self, text: &str) -> Result<(), &'static str> {
        crate::text::validate(text).map_err(|error| error.message())?;
        self.tagged.apply(text, &self.marker)
    }

    fn is_own(&mut self) -> Result<bool, &'static str> {
        // TARGETS/marker queries can race a new native selection owner. Retry the
        // observation rather than treating an inconclusive marker as a local copy.
        let mut error = "Clipboard marker inspection failed.";
        for attempt in 0..3 {
            match self.tagged.is_own(&self.marker) {
                Ok(own) => return Ok(own),
                Err(e) => error = e,
            }
            if attempt < 2 {
                std::thread::sleep(Duration::from_millis(5));
            }
        }
        Err(error)
    }

    /// Return only a new local text observation. Startup contents are not read or emitted.
    pub fn next_local_text(&mut self, timeout: Duration) -> Result<Option<String>, &'static str> {
        let deadline = Instant::now() + timeout;
        while let Some(remaining) = deadline.checked_duration_since(Instant::now()) {
            if !self.watcher.wait(remaining)? {
                break;
            }
            if self.is_own()? {
                continue;
            }
            let text = match self.clipboard.get_text() {
                Ok(text) => text,
                Err(message)
                    if [
                        crate::text::TextError::Empty,
                        crate::text::TextError::ContainsNul,
                        crate::text::TextError::TooLarge,
                        crate::text::TextError::InvalidUtf8,
                    ]
                    .iter()
                    .any(|error| error.message() == message) =>
                {
                    continue;
                }
                Err(message) => return Err(message),
            };
            // Applying received data can race a queued notification. Check after reading too.
            if !self.is_own()? {
                return Ok(Some(text));
            }
        }
        Ok(None)
    }
}

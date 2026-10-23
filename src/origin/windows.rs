use super::{FAILED, FORMAT};
use clipboard_win::{Clipboard, options::NoClear, raw};

pub struct TaggedClipboard {
    format: u32,
}
impl TaggedClipboard {
    pub fn new(_: &str) -> Result<Self, &'static str> {
        Ok(Self {
            format: clipboard_win::register_format(FORMAT).ok_or(FAILED)?.get(),
        })
    }
    pub fn apply(&mut self, text: &str, marker: &[u8; 32]) -> Result<(), &'static str> {
        let _lock = Clipboard::new_attempts(10).map_err(|_| FAILED)?;
        clipboard_win::empty().map_err(|_| FAILED)?;
        // Marker first: do not write received text if setting its marker fails.
        raw::set_without_clear(self.format, marker).map_err(|_| FAILED)?;
        raw::set_string_with(text, NoClear).map_err(|_| FAILED)
    }
    pub fn is_own(&mut self, marker: &[u8; 32]) -> Result<bool, &'static str> {
        let _lock = Clipboard::new_attempts(10).map_err(|_| FAILED)?;
        if !clipboard_win::is_format_avail(self.format) {
            return Ok(false);
        }
        let size = clipboard_win::size(self.format).ok_or(FAILED)?.get();
        if !(32..=64).contains(&size) {
            return Ok(false);
        }
        let mut observed = Vec::new();
        raw::get_vec(self.format, &mut observed).map_err(|_| FAILED)?;
        Ok(observed.starts_with(marker) && observed[32..].iter().all(|&b| b == 0))
    }
}

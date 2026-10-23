use super::{FAILED, FORMAT};
use clipboard_rs::{Clipboard, ClipboardContent, ClipboardContext};

pub enum TaggedClipboard {
    X11(ClipboardContext),
    Wayland,
    Gnome(crate::gnome::Adapter),
}

impl TaggedClipboard {
    pub fn new(backend: &str) -> Result<Self, &'static str> {
        if backend == "GNOME Shell adapter" {
            return crate::gnome::Adapter::new().map(Self::Gnome);
        }
        if backend == "Wayland data-control" {
            Ok(Self::Wayland)
        } else {
            ClipboardContext::new().map(Self::X11).map_err(|_| FAILED)
        }
    }

    pub fn apply(&mut self, text: &str, marker: &[u8; 32]) -> Result<(), &'static str> {
        match self {
            Self::Gnome(adapter) => adapter.apply(text, marker),
            Self::X11(context) => context
                .set(vec![
                    ClipboardContent::Other(FORMAT.into(), marker.to_vec()),
                    ClipboardContent::Text(text.into()),
                ])
                .map_err(|_| FAILED),
            Self::Wayland => {
                use wl_clipboard_rs::copy::{MimeSource, MimeType, Options, Source};
                Options::new()
                    .copy_multi(vec![
                        MimeSource {
                            source: Source::Bytes(marker.to_vec().into()),
                            mime_type: MimeType::Specific(FORMAT.into()),
                        },
                        MimeSource {
                            source: Source::Bytes(text.as_bytes().into()),
                            mime_type: MimeType::Text,
                        },
                    ])
                    .map_err(|_| FAILED)
            }
        }
    }

    pub fn is_own(&mut self, marker: &[u8; 32]) -> Result<bool, &'static str> {
        match self {
            Self::Gnome(adapter) => adapter.is_own(marker),
            Self::X11(context) => {
                if !context
                    .available_formats()
                    .map_err(|_| FAILED)?
                    .iter()
                    .any(|f| f == FORMAT)
                {
                    return Ok(false);
                }
                let observed = context.get_buffer(FORMAT).map_err(|_| FAILED)?;
                Ok(observed.as_slice() == marker)
            }
            Self::Wayland => {
                use wl_clipboard_rs::paste::{ClipboardType, Error, MimeType, Seat, get_contents};
                let (pipe, _) = match get_contents(
                    ClipboardType::Regular,
                    Seat::Unspecified,
                    MimeType::Specific(FORMAT),
                ) {
                    Ok(result) => result,
                    Err(Error::ClipboardEmpty | Error::NoMimeType) => return Ok(false),
                    Err(_) => return Err(FAILED),
                };
                let observed = match crate::text::read_bytes_bounded_fd(
                    pipe,
                    32,
                    std::time::Duration::from_secs(1),
                ) {
                    Ok(bytes) => bytes,
                    Err(crate::text::TextError::TooLarge) => return Ok(false),
                    Err(_) => return Err(FAILED),
                };
                Ok(observed.as_slice() == marker)
            }
        }
    }
}

//! Plain-text clipboard access with explicit native backend selection.

pub const UNAVAILABLE: &str =
    "Clipboard unavailable. Check the graphical session and clipboard permissions.";

/// No Debug implementation: clipboard adapters may contain sensitive text.
pub struct TextClipboard {
    backend: Backend,
}

enum Backend {
    Native(arboard::Clipboard),
    #[cfg(target_os = "linux")]
    Wayland,
}

impl TextClipboard {
    pub fn new() -> Result<Self, &'static str> {
        #[cfg(target_os = "linux")]
        match linux_session(
            std::env::var_os("XDG_SESSION_TYPE").as_deref(),
            std::env::var_os("WAYLAND_DISPLAY").as_deref(),
            std::env::var_os("DISPLAY").as_deref(),
        )? {
            LinuxSession::Wayland => {
                // Validates compositor/data-control access without reading clipboard contents.
                wl_clipboard_rs::utils::is_primary_selection_supported().map_err(|_| {
                    "Native Wayland clipboard unavailable. A data-control protocol or desktop adapter is required; XWayland fallback is disabled."
                })?;
                return Ok(Self {
                    backend: Backend::Wayland,
                });
            }
            LinuxSession::X11 => {}
        }
        Ok(Self {
            backend: Backend::Native(arboard::Clipboard::new().map_err(|_| UNAVAILABLE)?),
        })
    }

    pub fn backend_name(&self) -> &'static str {
        match self.backend {
            #[cfg(target_os = "linux")]
            Backend::Wayland => "Wayland data-control",
            Backend::Native(_) => {
                #[cfg(target_os = "linux")]
                {
                    "X11 CLIPBOARD"
                }
                #[cfg(target_os = "windows")]
                {
                    "Windows clipboard"
                }
                #[cfg(target_os = "macos")]
                {
                    "macOS pasteboard"
                }
                #[cfg(not(any(target_os = "linux", target_os = "windows", target_os = "macos")))]
                {
                    "Native clipboard"
                }
            }
        }
    }

    pub fn set_text(&mut self, text: &str) -> Result<(), &'static str> {
        match &mut self.backend {
            Backend::Native(clipboard) => clipboard
                .set_text(text)
                .map_err(|_| "Clipboard write failed."),
            #[cfg(target_os = "linux")]
            Backend::Wayland => {
                use wl_clipboard_rs::copy::{MimeType, Options, Source};
                // Background serving keeps selection data available to independent readers.
                Options::new()
                    .copy(Source::Bytes(text.as_bytes().into()), MimeType::Text)
                    .map_err(|_| "Clipboard write failed.")
            }
        }
    }

    pub fn get_text(&mut self) -> Result<String, &'static str> {
        match &mut self.backend {
            Backend::Native(clipboard) => {
                clipboard.get_text().map_err(|_| "Clipboard read failed.")
            }
            #[cfg(target_os = "linux")]
            Backend::Wayland => {
                use std::io::Read;
                use wl_clipboard_rs::paste::{ClipboardType, MimeType, Seat, get_contents};
                let (mut pipe, _) =
                    get_contents(ClipboardType::Regular, Seat::Unspecified, MimeType::Text)
                        .map_err(|_| "Clipboard read failed.")?;
                let mut text = String::new();
                pipe.read_to_string(&mut text)
                    .map_err(|_| "Clipboard read failed.")?;
                Ok(text)
            }
        }
    }
}

#[cfg(target_os = "linux")]
#[derive(Debug, PartialEq, Eq)]
enum LinuxSession {
    X11,
    Wayland,
}

#[cfg(target_os = "linux")]
fn linux_session(
    session: Option<&std::ffi::OsStr>,
    wayland_display: Option<&std::ffi::OsStr>,
    display: Option<&std::ffi::OsStr>,
) -> Result<LinuxSession, &'static str> {
    let populated = |value: Option<&std::ffi::OsStr>| value.is_some_and(|s| !s.is_empty());
    if session == Some(std::ffi::OsStr::new("wayland")) || populated(wayland_display) {
        Ok(LinuxSession::Wayland)
    } else if populated(display) {
        Ok(LinuxSession::X11)
    } else {
        Err(UNAVAILABLE)
    }
}

#[cfg(all(test, target_os = "linux"))]
mod tests {
    use super::*;
    use std::ffi::OsStr;

    #[test]
    fn wayland_session_never_selects_xwayland() {
        for wayland in [None, Some(OsStr::new("")), Some(OsStr::new("wayland-0"))] {
            assert_eq!(
                linux_session(Some(OsStr::new("wayland")), wayland, Some(OsStr::new(":0"))),
                Ok(LinuxSession::Wayland)
            );
        }
    }

    #[test]
    fn wayland_socket_takes_priority_over_display() {
        assert_eq!(
            linux_session(None, Some(OsStr::new("wayland-0")), Some(OsStr::new(":0"))),
            Ok(LinuxSession::Wayland)
        );
    }

    #[test]
    fn x11_requires_a_display_and_headless_is_unavailable() {
        assert_eq!(
            linux_session(None, None, Some(OsStr::new(":0"))),
            Ok(LinuxSession::X11)
        );
        assert!(linux_session(None, Some(OsStr::new("")), Some(OsStr::new(""))).is_err());
        assert!(linux_session(None, None, None).is_err());
    }
}

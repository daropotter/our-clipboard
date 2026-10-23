//! Content-free native clipboard notifications. Observing a change never reads text.
use std::time::{Duration, Instant};

#[cfg(target_os = "linux")]
mod x11;

pub struct Watcher {
    #[cfg(target_os = "linux")]
    backend: x11::Watcher,
}

impl Watcher {
    pub fn new() -> Result<Self, &'static str> {
        #[cfg(target_os = "linux")]
        {
            if std::env::var_os("WAYLAND_DISPLAY").is_some_and(|s| !s.is_empty())
                || std::env::var_os("XDG_SESSION_TYPE").is_some_and(|s| s == "wayland")
            {
                return Err(
                    "Native Wayland notifications are not implemented yet; XWayland fallback is disabled.",
                );
            }
            Ok(Self {
                backend: x11::Watcher::new()?,
            })
        }
        #[cfg(not(target_os = "linux"))]
        Err("Native clipboard notifications are not implemented on this platform yet.")
    }

    pub fn backend_name(&self) -> &'static str {
        "X11 XFixes"
    }

    /// Wait for a native selection change, bounded by a local deadline.
    /// This drains X11 events; it does not poll clipboard contents.
    pub fn wait(&mut self, timeout: Duration) -> Result<bool, &'static str> {
        let deadline = Instant::now() + timeout;
        loop {
            #[cfg(target_os = "linux")]
            if self.backend.changed()? {
                return Ok(true);
            }
            if Instant::now() >= deadline {
                return Ok(false);
            }
            std::thread::sleep(
                Duration::from_millis(10).min(deadline.saturating_duration_since(Instant::now())),
            );
        }
    }
}

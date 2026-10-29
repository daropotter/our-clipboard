//! Content-free native clipboard notifications. Observing a change never reads text.
use std::{
    marker::PhantomData,
    rc::Rc,
    time::{Duration, Instant},
};

#[cfg(target_os = "linux")]
mod gnome;
#[cfg(target_os = "linux")]
mod wayland;
#[cfg(target_os = "linux")]
mod x11;

#[cfg(target_os = "macos")]
mod macos;
#[cfg(target_os = "windows")]
mod windows;

pub struct Watcher {
    backend: Backend,
    // Native message windows/pasteboards must stay on the creating thread.
    _thread_bound: PhantomData<Rc<()>>,
}

enum Backend {
    #[cfg(target_os = "linux")]
    X11(Box<x11::Watcher>),
    #[cfg(target_os = "linux")]
    Wayland(Box<wayland::Watcher>),
    #[cfg(target_os = "linux")]
    Gnome(gnome::Watcher),
    #[cfg(target_os = "windows")]
    Windows(windows::Watcher),
    #[cfg(target_os = "macos")]
    Macos(macos::Watcher),
}

impl Watcher {
    pub fn new() -> Result<Self, &'static str> {
        #[cfg(target_os = "linux")]
        {
            if std::env::var_os("WAYLAND_DISPLAY").is_some_and(|s| !s.is_empty())
                || std::env::var_os("XDG_SESSION_TYPE").is_some_and(|s| s == "wayland")
            {
                return Ok(Self {
                    backend: match wayland::Watcher::new() {
                        Ok(watcher) => Backend::Wayland(Box::new(watcher)),
                        Err(error) => Backend::Gnome(gnome::Watcher::new().map_err(|_| error)?),
                    },
                    _thread_bound: PhantomData,
                });
            }
            Ok(Self {
                backend: Backend::X11(Box::new(x11::Watcher::new()?)),
                _thread_bound: PhantomData,
            })
        }
        #[cfg(target_os = "windows")]
        {
            Ok(Self {
                backend: Backend::Windows(windows::Watcher::new()?),
                _thread_bound: PhantomData,
            })
        }
        #[cfg(target_os = "macos")]
        {
            Ok(Self {
                backend: Backend::Macos(macos::Watcher::new()),
                _thread_bound: PhantomData,
            })
        }
        #[cfg(not(any(target_os = "linux", target_os = "windows", target_os = "macos")))]
        Err("Native clipboard notifications are not implemented on this platform yet.")
    }

    pub(crate) fn new_data_control() -> Result<Self, &'static str> {
        #[cfg(target_os = "linux")]
        {
            Ok(Self {
                backend: Backend::Wayland(Box::new(wayland::Watcher::new()?)),
                _thread_bound: PhantomData,
            })
        }
        #[cfg(not(target_os = "linux"))]
        Err("Native Wayland data-control watcher requires Linux; no fallback.")
    }

    pub fn backend_name(&self) -> &'static str {
        match &self.backend {
            #[cfg(target_os = "linux")]
            Backend::X11(_) => "X11 XFixes",
            #[cfg(target_os = "linux")]
            Backend::Wayland(watcher) => watcher.backend_name(),
            #[cfg(target_os = "linux")]
            Backend::Gnome(_) => "GNOME Shell owner-change counter",
            #[cfg(target_os = "windows")]
            Backend::Windows(_) => "Windows WM_CLIPBOARDUPDATE",
            #[cfg(target_os = "macos")]
            Backend::Macos(_) => "macOS NSPasteboard.changeCount",
        }
    }

    /// Wait for a native selection change, bounded by a local deadline.
    /// This drains native events; it does not poll clipboard contents.
    pub fn wait(&mut self, timeout: Duration) -> Result<bool, &'static str> {
        let deadline = Instant::now() + timeout;
        loop {
            if match &mut self.backend {
                #[cfg(target_os = "linux")]
                Backend::X11(w) => w.changed()?,
                #[cfg(target_os = "linux")]
                Backend::Wayland(w) => w.changed()?,
                #[cfg(target_os = "linux")]
                Backend::Gnome(w) => w.changed()?,
                #[cfg(target_os = "windows")]
                Backend::Windows(w) => w.changed()?,
                #[cfg(target_os = "macos")]
                Backend::Macos(w) => w.changed(),
            } {
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

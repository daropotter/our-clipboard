//! Versioned local GNOME Shell extension adapter; no public network endpoint.
use zbus::blocking::{Connection, Proxy};
const DEST: &str = "org.gnome.Shell";
const PATH: &str = "/org/gnome/Shell/Extensions/OurClipboard";
const INTERFACE: &str = "org.gnome.Shell.Extensions.OurClipboard1";
const FAILED: &str = "GNOME clipboard adapter unavailable. Install and enable the compatible Our Clipboard GNOME Shell extension.";

pub struct Adapter {
    connection: Connection,
    generation: String,
}
impl Adapter {
    pub fn new() -> Result<Self, &'static str> {
        let connection = zbus::blocking::connection::Builder::session()
            .map_err(|_| FAILED)?
            .method_timeout(std::time::Duration::from_secs(2))
            .build()
            .map_err(|_| FAILED)?;
        let mut adapter = Self {
            connection,
            generation: String::new(),
        };
        let version: u32 = adapter.proxy()?.call("Version", &()).map_err(|_| FAILED)?;
        if version != 1 {
            return Err(FAILED);
        }
        let socket: String = adapter.proxy()?.call("Socket", &()).map_err(|_| FAILED)?;
        let requested = std::path::PathBuf::from(
            std::env::var_os("WAYLAND_DISPLAY").unwrap_or_else(|| "wayland-0".into()),
        );
        let requested = if requested.is_absolute() {
            requested
        } else {
            std::path::PathBuf::from(std::env::var_os("XDG_RUNTIME_DIR").ok_or(FAILED)?)
                .join(requested)
        };
        if requested.canonicalize().map_err(|_| FAILED)?
            != std::path::Path::new(&socket)
                .canonicalize()
                .map_err(|_| FAILED)?
        {
            return Err("GNOME clipboard adapter belongs to another Wayland session.");
        }
        let (_, _, generation): (u64, String, String) =
            adapter.proxy()?.call("GetState", &()).map_err(|_| FAILED)?;
        adapter.generation = generation;
        Ok(adapter)
    }
    fn proxy(&self) -> Result<Proxy<'_>, &'static str> {
        Proxy::new(&self.connection, DEST, PATH, INTERFACE).map_err(|_| FAILED)
    }
    pub fn state(&self) -> Result<(u64, String), &'static str> {
        let (revision, marker, generation): (u64, String, String) =
            self.proxy()?.call("GetState", &()).map_err(|_| FAILED)?;
        if generation != self.generation {
            return Err("GNOME clipboard adapter restarted; create a new session.");
        }
        Ok((revision, marker))
    }
    pub fn get_text(&self) -> Result<String, &'static str> {
        let (text, _, _, generation): (String, u64, String, String) = self
            .proxy()?
            .call("GetText", &())
            .map_err(|error| match error {
                zbus::Error::MethodError(ref name, _, _)
                    if name.as_str().ends_with(".TooLarge") =>
                {
                    crate::text::TextError::TooLarge.message()
                }
                _ => "GNOME clipboard read failed or changed during transfer.",
            })?;
        if generation != self.generation {
            return Err("GNOME clipboard adapter restarted; create a new session.");
        }
        crate::text::validate(&text).map_err(|error| error.message())?;
        Ok(text)
    }
    pub fn set_text(&self, text: &str) -> Result<(), &'static str> {
        self.state()?;
        self.proxy()?
            .call("SetText", &(text,))
            .map_err(|_| "GNOME clipboard write failed.")
    }
    pub fn apply(&self, text: &str, marker: &[u8; 32]) -> Result<(), &'static str> {
        self.state()?;
        self.proxy()?
            .call("ApplyText", &(text, encode_marker(marker)))
            .map_err(|_| "GNOME clipboard application failed.")
    }
    pub fn is_own(&self, marker: &[u8; 32]) -> Result<bool, &'static str> {
        Ok(self.state()?.1 == encode_marker(marker))
    }
}
fn encode_marker(marker: &[u8; 32]) -> String {
    use std::fmt::Write;
    let mut encoded = String::with_capacity(64);
    for byte in marker {
        write!(encoded, "{byte:02x}").expect("writing to String cannot fail");
    }
    encoded
}

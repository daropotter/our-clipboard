//! Local clipboard write markers, not network identities or authorization tokens.
pub const FORMAT: &str = "application/x-our-clipboard-origin-v1";
pub const FAILED: &str = "Could not apply or inspect the local clipboard origin marker.";

#[cfg(target_os = "linux")]
mod linux;
#[cfg(target_os = "linux")]
pub use linux::TaggedClipboard;
#[cfg(target_os = "windows")]
mod windows;
#[cfg(target_os = "windows")]
pub use windows::TaggedClipboard;
#[cfg(target_os = "macos")]
mod macos;
#[cfg(target_os = "macos")]
pub use macos::TaggedClipboard;

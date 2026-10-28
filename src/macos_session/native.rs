//! Only public framework functions; IOConsoleLocked is an experimental registry schema.
//! Sources/limitations: docs/OUTBOUND-PILOT.md. No clipboard, usernames or logs here.
use super::{Observation, Session};
use std::ffi::{c_int, c_void};
type Ref = *const c_void;
const NUMBER_SINT64: isize = 4;
const STRING_UTF8: u32 = 0x0800_0100;
#[link(name = "CoreGraphics", kind = "framework")]
unsafe extern "C" {
    fn CGSessionCopyCurrentDictionary() -> Ref;
}
#[link(name = "CoreFoundation", kind = "framework")]
unsafe extern "C" {
    fn CFRelease(value: Ref);
    fn CFGetTypeID(value: Ref) -> usize;
    fn CFDictionaryGetTypeID() -> usize;
    fn CFDictionaryGetValue(dictionary: Ref, key: Ref) -> Ref;
    #[cfg(test)]
    static kCFBooleanTrue: Ref;
    #[cfg(test)]
    static kCFBooleanFalse: Ref;
    fn CFBooleanGetTypeID() -> usize;
    fn CFBooleanGetValue(value: Ref) -> u8;
    fn CFNumberGetTypeID() -> usize;
    #[cfg(test)]
    fn CFNumberCreate(allocator: Ref, kind: isize, value: *const c_void) -> Ref;
    fn CFNumberGetValue(value: Ref, kind: isize, output: *mut c_void) -> u8;
    fn CFStringCreateWithBytes(
        allocator: Ref,
        bytes: *const u8,
        length: isize,
        encoding: u32,
        external: u8,
    ) -> Ref;
}
#[link(name = "IOKit", kind = "framework")]
unsafe extern "C" {
    fn IORegistryGetRootEntry(port: u32) -> u32;
    fn IORegistryEntryCreateCFProperty(entry: u32, key: Ref, allocator: Ref, options: u32) -> Ref;
    fn IOObjectRelease(object: u32) -> c_int;
}
struct Owned(Ref);
impl Owned {
    fn new(value: Ref) -> Option<Self> {
        if value.is_null() {
            None
        } else {
            Some(Self(value))
        }
    }
}
impl Drop for Owned {
    fn drop(&mut self) {
        // SAFETY: exclusively owns a nonnull Create/Copy-rule CF reference.
        unsafe {
            CFRelease(self.0);
        }
    }
}
struct Entry(u32);
impl Drop for Entry {
    fn drop(&mut self) {
        // SAFETY: owns the successful IORegistryGetRootEntry reference.
        unsafe {
            IOObjectRelease(self.0);
        }
    }
}
fn key(name: &str) -> Option<Owned> {
    // SAFETY: bytes remain valid for this synchronous copy; null allocator means default.
    Owned::new(unsafe {
        CFStringCreateWithBytes(
            std::ptr::null(),
            name.as_ptr(),
            name.len().try_into().ok()?,
            STRING_UTF8,
            0,
        )
    })
}
fn boolean(value: Ref) -> Option<bool> {
    // SAFETY: nonnull borrowed CF object, checked for exact CFBoolean type before access.
    if value.is_null() || unsafe { CFGetTypeID(value) != CFBooleanGetTypeID() } {
        return None;
    }
    Some(unsafe { CFBooleanGetValue(value) != 0 })
}
fn number(value: Ref) -> Option<u32> {
    // SAFETY: check exact CFNumber type, then request kCFNumberSInt64Type into i64 storage.
    if value.is_null() || unsafe { CFGetTypeID(value) != CFNumberGetTypeID() } {
        return None;
    }
    let mut n = 0i64;
    if unsafe { CFNumberGetValue(value, NUMBER_SINT64, (&mut n as *mut i64).cast()) } == 0 {
        return None;
    }
    n.try_into().ok()
}
fn property(dictionary: &Owned, name: &str) -> Option<Ref> {
    let key = key(name)?;
    // SAFETY: caller retains a validated dictionary, key is a live CFString;
    // returned object remains borrowed from dictionary until caller finishes parsing.
    let value = unsafe { CFDictionaryGetValue(dictionary.0, key.0) };
    (!value.is_null()).then_some(value)
}
fn session() -> Option<Session> {
    // SAFETY: public Copy-rule API may return null outside a Quartz GUI session.
    let dictionary = Owned::new(unsafe { CGSessionCopyCurrentDictionary() })?;
    if unsafe { CFGetTypeID(dictionary.0) != CFDictionaryGetTypeID() } {
        return None;
    }
    Some(Session {
        uid: number(property(&dictionary, "kCGSSessionUserIDKey")?)?,
        console: number(property(&dictionary, "kCGSSessionConsoleSetKey")?)?,
        on_console: boolean(property(&dictionary, "kCGSSessionOnConsoleKey")?)?,
        login_done: boolean(property(&dictionary, "kCGSessionLoginDoneKey")?)?,
        // A positive extra lock hint can only restrict access. Its absence is
        // not an unlock signal: explicit IOConsoleLocked=false is still required.
        locked_hint: match property(&dictionary, "CGSSessionScreenIsLocked") {
            Some(value) => Some(boolean(value)?),
            None => None,
        },
    })
}
fn locked() -> Option<bool> {
    let key = key("IOConsoleLocked")?;
    // SAFETY: zero is kIOMainPortDefault. Successful root entries are reference-counted.
    let root = unsafe { IORegistryGetRootEntry(0) };
    if root == 0 {
        return None;
    }
    let root = Entry(root);
    // SAFETY: live root/key; public Create-rule API returns an owned CF object or null.
    let value =
        Owned::new(unsafe { IORegistryEntryCreateCFProperty(root.0, key.0, std::ptr::null(), 0) })?;
    boolean(value.0)
}
pub(super) fn read() -> Option<Observation> {
    let before = session()?;
    let locked = locked()?;
    let after = session()?;
    Some(Observation {
        before,
        after,
        locked,
    })
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn null_and_wrong_native_types_are_unknown() {
        assert_eq!(boolean(std::ptr::null()), None);
        assert_eq!(number(std::ptr::null()), None);
        let string = key("false").unwrap();
        assert_eq!(boolean(string.0), None);
        assert_eq!(number(string.0), None);
    }
    #[test]
    fn explicit_native_booleans_and_uid_numbers_keep_exact_types() {
        // SAFETY: framework globals are permanent CFBoolean references.
        assert_eq!(boolean(unsafe { kCFBooleanFalse }), Some(false));
        assert_eq!(boolean(unsafe { kCFBooleanTrue }), Some(true));
        assert_eq!(number(unsafe { kCFBooleanFalse }), None);
        for (input, expected) in [
            (1000i64, Some(1000u32)),
            (-1, None),
            (i64::from(u32::MAX) + 1, None),
        ] {
            // SAFETY: the API copies one valid i64, with the same exact numeric type.
            let value = Owned::new(unsafe {
                CFNumberCreate(std::ptr::null(), NUMBER_SINT64, (&input as *const i64).cast())
            })
            .unwrap();
            assert_eq!(number(value.0), expected);
            assert_eq!(boolean(value.0), None);
        }
    }
    #[test]
    fn native_read_does_not_require_clipboard_or_assume_gui_session() {
        // CI can be outside a GUI login: None is safe, never interpreted as unlocked.
        let mut policy = super::super::Policy::default();
        let _state = policy.observe(read(), rustix::process::geteuid().as_raw());
    }
}

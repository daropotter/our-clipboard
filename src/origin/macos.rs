use super::{FAILED, FORMAT};
use objc2::{rc::Retained, rc::autoreleasepool};
use objc2_app_kit::{NSPasteboard, NSPasteboardTypeString};
use objc2_foundation::{NSData, NSString};

pub struct TaggedClipboard {
    pasteboard: Retained<NSPasteboard>,
}
impl TaggedClipboard {
    pub fn new(_: &str) -> Result<Self, &'static str> {
        Ok(Self {
            pasteboard: NSPasteboard::generalPasteboard(),
        })
    }
    pub fn apply(&mut self, text: &str, marker: &[u8; 32]) -> Result<(), &'static str> {
        autoreleasepool(|_| {
            self.pasteboard.clearContents();
            if !self.pasteboard.setData_forType(
                Some(&NSData::with_bytes(marker)),
                &NSString::from_str(FORMAT),
            ) {
                return Err(FAILED);
            }
            // The constant is provided by AppKit and lives for the process lifetime.
            if !self
                .pasteboard
                .setString_forType(&NSString::from_str(text), unsafe { NSPasteboardTypeString })
            {
                return Err(FAILED);
            }
            Ok(())
        })
    }
    pub fn is_own(&mut self, marker: &[u8; 32]) -> Result<bool, &'static str> {
        autoreleasepool(|_| {
            let Some(data) = self.pasteboard.dataForType(&NSString::from_str(FORMAT)) else {
                return Ok(false);
            };
            Ok(data.len() == 32 && data.to_vec().as_slice() == marker)
        })
    }
}

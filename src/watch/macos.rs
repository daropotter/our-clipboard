use objc2::rc::Retained;
use objc2_app_kit::NSPasteboard;

pub struct Watcher {
    pasteboard: Retained<NSPasteboard>,
    last_change: isize,
}

impl Watcher {
    pub fn new() -> Self {
        objc2::rc::autoreleasepool(|_| {
            let pasteboard = NSPasteboard::generalPasteboard();
            let last_change = pasteboard.changeCount();
            Self {
                pasteboard,
                last_change,
            }
        })
    }

    pub fn changed(&mut self) -> bool {
        // macOS exposes changeCount rather than a clipboard notification subscription.
        // Check only this counter; never request clipboard text.
        let current = self.pasteboard.changeCount();
        let changed = current != self.last_change;
        self.last_change = current;
        changed
    }
}

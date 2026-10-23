use clipboard_win::monitor::Monitor;

pub struct Watcher(Monitor);

impl Watcher {
    pub fn new() -> Result<Self, &'static str> {
        Monitor::new()
            .map(Self)
            .map_err(|_| "Windows clipboard notifications unavailable.")
    }

    pub fn changed(&mut self) -> Result<bool, &'static str> {
        self.0
            .try_recv()
            .map_err(|_| "Windows clipboard notification listener failed.")
    }
}

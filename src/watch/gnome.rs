use crate::gnome::Adapter;
use std::time::{Duration, Instant};

pub struct Watcher {
    adapter: Adapter,
    revision: u64,
    next_sample: Instant,
}
impl Watcher {
    pub fn new() -> Result<Self, &'static str> {
        let adapter = Adapter::new()?;
        let revision = adapter.state()?.0;
        Ok(Self {
            adapter,
            revision,
            next_sample: Instant::now(),
        })
    }
    pub fn changed(&mut self) -> Result<bool, &'static str> {
        if Instant::now() < self.next_sample {
            return Ok(false);
        }
        self.next_sample = Instant::now() + Duration::from_millis(100);
        let revision = self.adapter.state()?.0;
        let changed = revision != self.revision;
        self.revision = revision;
        Ok(changed)
    }
}

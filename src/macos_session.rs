//! Conservative caller-session gate; native IOKit/CoreGraphics source exists only on macOS.
#[cfg(target_os = "macos")]
mod native;
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum State {
    Active,
    Idle,
    Unknown,
}
const MAX_AGE_MS: u64 = 300;
#[derive(Clone, Copy, PartialEq, Eq)]
struct Session {
    uid: u32,
    console: Option<u32>,
    audit: u32,
    on_console: bool,
    login_done: bool,
    locked_hint: Option<bool>,
}
#[derive(Clone, Copy)]
struct Observation {
    before: Session,
    after: Session,
    locked: bool,
    process_before: u32,
    process_after: u32,
}
#[derive(Default)]
struct Policy {
    binding: Option<(u32, u32, Option<u32>)>,
}
impl Policy {
    fn observe(&mut self, sample: Option<Observation>, uid: u32) -> State {
        let Some(sample) = sample else {
            return State::Unknown;
        };
        if sample.before != sample.after
            || sample.after.uid != uid
            || sample.after.audit == 0
            || sample.after.audit == u32::MAX
            || sample.process_before != sample.after.audit
            || sample.process_after != sample.after.audit
        {
            return State::Unknown;
        }
        let current = (sample.after.uid, sample.after.audit, sample.after.console);
        if let Some(binding) = self.binding {
            if binding != current {
                return State::Unknown;
            }
        } else {
            self.binding = Some(current);
        }
        if !sample.after.on_console
            || !sample.after.login_done
            || sample.locked
            || sample.after.locked_hint == Some(true)
        {
            State::Idle
        } else {
            State::Active
        }
    }
}
#[derive(Default)]
struct Snapshot {
    sample: Option<(State, u64)>,
}
impl Snapshot {
    fn state(&self, now: u64) -> State {
        match self.sample {
            Some((state, at)) if now >= at && now - at <= MAX_AGE_MS => state,
            _ => State::Unknown,
        }
    }
}
#[cfg(target_os = "macos")]
pub(crate) struct Monitor {
    snapshot: std::sync::Arc<std::sync::Mutex<Snapshot>>,
    stop: std::sync::Arc<std::sync::atomic::AtomicBool>,
}
#[cfg(target_os = "macos")]
impl Monitor {
    pub fn new() -> Self {
        use crate::freshness::{BootClock, SystemBootClock};
        use std::sync::{
            Arc, Mutex,
            atomic::{AtomicBool, Ordering},
        };
        let snapshot = Arc::new(Mutex::new(Snapshot::default()));
        let stop = Arc::new(AtomicBool::new(false));
        let (output, done) = (snapshot.clone(), stop.clone());
        std::thread::spawn(move || {
            let mut policy = Policy::default();
            while !done.load(Ordering::Relaxed) {
                // Timestamp BEFORE native I/O. Suspend or blocked calls do not renew freshness.
                let at = SystemBootClock.now_ms().ok();
                let state = policy.observe(native::read(), rustix::process::geteuid().as_raw());
                if let Ok(mut output) = output.lock() {
                    output.sample = at.map(|at| (state, at));
                }
                std::thread::sleep(std::time::Duration::from_millis(75));
            }
        });
        Self { snapshot, stop }
    }
    pub fn state(&self) -> State {
        use crate::freshness::{BootClock, SystemBootClock};
        let Ok(now) = SystemBootClock.now_ms() else {
            return State::Unknown;
        };
        self.snapshot
            .lock()
            .map(|s| s.state(now))
            .unwrap_or(State::Unknown)
    }
}
#[cfg(target_os = "macos")]
impl Drop for Monitor {
    fn drop(&mut self) {
        self.stop.store(true, std::sync::atomic::Ordering::Relaxed);
    }
}
#[cfg(target_os = "macos")]
pub(crate) fn probe() -> State {
    use crate::freshness::{BootClock, SystemBootClock};
    let Ok(at) = SystemBootClock.now_ms() else {
        return State::Unknown;
    };
    let state = Policy::default().observe(native::read(), rustix::process::geteuid().as_raw());
    let Ok(now) = SystemBootClock.now_ms() else {
        return State::Unknown;
    };
    Snapshot {
        sample: Some((state, at)),
    }
    .state(now)
}
#[cfg(test)]
mod tests {
    use super::*;
    fn observation(locked: bool) -> Observation {
        let session = Session {
            uid: 1000,
            console: Some(1),
            audit: 42,
            on_console: true,
            login_done: true,
            locked_hint: None,
        };
        Observation {
            before: session,
            after: session,
            locked,
            process_before: 42,
            process_after: 42,
        }
    }
    #[test]
    fn startup_missing_lock_and_return_never_assume_unlocked() {
        let mut p = Policy::default();
        assert_eq!(p.observe(None, 1000), State::Unknown);
        assert_eq!(p.observe(Some(observation(true)), 1000), State::Idle);
        assert_eq!(p.observe(Some(observation(false)), 1000), State::Active);
        assert_eq!(p.observe(None, 1000), State::Unknown);
        assert_eq!(p.observe(Some(observation(true)), 1000), State::Idle);
        assert_eq!(p.observe(Some(observation(false)), 1000), State::Active);
    }
    #[test]
    fn positive_session_lock_hint_overrides_an_unlocked_registry() {
        let mut sample = observation(false);
        sample.before.locked_hint = Some(true);
        sample.after.locked_hint = Some(true);
        assert_eq!(Policy::default().observe(Some(sample), 1000), State::Idle);
    }
    #[test]
    fn foreign_incomplete_and_switched_sessions_do_not_activate() {
        let mut p = Policy::default();
        assert_eq!(p.observe(Some(observation(false)), 999), State::Unknown);
        for field in [0, 1] {
            let mut o = observation(false);
            if field == 0 {
                o.before.on_console = false;
                o.after.on_console = false;
            } else {
                o.before.login_done = false;
                o.after.login_done = false;
            }
            assert_eq!(p.observe(Some(o), 1000), State::Idle);
        }
        let mut racing = observation(false);
        racing.after.console = Some(2);
        assert_eq!(p.observe(Some(racing), 1000), State::Unknown);
        let mut changed = observation(false);
        changed.before.console = Some(2);
        changed.after.console = Some(2);
        assert_eq!(p.observe(Some(changed), 1000), State::Unknown);
    }
    #[test]
    fn absent_console_requires_a_matching_pinned_process_audit_session() {
        let mut sample = observation(false);
        sample.before.console = None;
        sample.after.console = None;
        let mut policy = Policy::default();
        assert_eq!(policy.observe(Some(sample), 1000), State::Active);
        sample.locked = true;
        assert_eq!(policy.observe(Some(sample), 1000), State::Idle);
        sample.locked = false;
        for audit in [0, u32::MAX, 43] {
            let mut changed = sample;
            changed.before.audit = audit;
            changed.after.audit = audit;
            changed.process_before = audit;
            changed.process_after = audit;
            assert_eq!(policy.observe(Some(changed), 1000), State::Unknown);
        }
        // Reject a foreign or racing kernel audit observation, even on first use.
        for (before, after) in [(43, 42), (42, 43), (43, 43)] {
            let mut changed = sample;
            changed.process_before = before;
            changed.process_after = after;
            assert_eq!(
                Policy::default().observe(Some(changed), 1000),
                State::Unknown
            );
        }
        sample.after.console = Some(1);
        sample.before.console = Some(1);
        assert_eq!(policy.observe(Some(sample), 1000), State::Unknown);
    }

    #[test]
    fn stalled_worker_suspend_and_clock_errors_stay_unknown() {
        let s = Snapshot {
            sample: Some((State::Active, 100)),
        };
        assert_eq!(s.state(100), State::Active);
        assert_eq!(s.state(401), State::Unknown);
        assert_eq!(s.state(100_000), State::Unknown);
        assert_eq!(s.state(99), State::Unknown);
        assert_eq!(Snapshot::default().state(100), State::Unknown);
    }
}

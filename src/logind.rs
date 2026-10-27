//! Opt-in X11 session gate. Unknown/stale logind observations never enable clipboard work.
use crate::freshness::{BootClock, SystemBootClock};
use std::{
    collections::HashMap,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
    thread,
    time::Duration,
};
use zbus::{
    blocking::{Connection, Proxy},
    zvariant::{OwnedObjectPath, OwnedValue},
};
const MANAGER: &str = "org.freedesktop.login1.Manager";
const SESSION: &str = "org.freedesktop.login1.Session";
const PATH: &str = "/org/freedesktop/login1";
const SAMPLE_MS: u64 = 75;
const MAX_AGE_MS: u64 = 300;
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum State {
    Active,
    Idle,
    Unknown,
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
fn state(
    properties: &HashMap<String, OwnedValue>,
    manager: &HashMap<String, OwnedValue>,
) -> Option<State> {
    let kind = <&str>::try_from(properties.get("Type")?).ok()?;
    let class = <&str>::try_from(properties.get("Class")?).ok()?;
    let active = bool::try_from(properties.get("Active")?).ok()?;
    let locked = bool::try_from(properties.get("LockedHint")?).ok()?;
    let remote = bool::try_from(properties.get("Remote")?).ok()?;
    let sleeping = bool::try_from(manager.get("PreparingForSleep")?).ok()?;
    Some(
        if kind == "x11" && class == "user" && active && !locked && !remote && !sleeping {
            State::Active
        } else {
            State::Idle
        },
    )
}
struct Source {
    connection: Connection,
    owner: String,
    session: OwnedObjectPath,
}
impl Source {
    fn new() -> zbus::Result<Self> {
        let connection = zbus::blocking::connection::Builder::system()?
            .method_timeout(Duration::from_millis(200))
            .build()?;
        let bus = Proxy::new(
            &connection,
            "org.freedesktop.DBus",
            "/org/freedesktop/DBus",
            "org.freedesktop.DBus",
        )?;
        let owner: String = bus.call("GetNameOwner", &("org.freedesktop.login1",))?;
        let manager = Proxy::new(&connection, owner.as_str(), PATH, MANAGER)?;
        // Resolve this process, never an environment-supplied session ID or another desktop.
        let session = manager.call("GetSessionByPID", &(std::process::id(),))?;
        drop(manager);
        drop(bus);
        Ok(Self {
            connection,
            owner,
            session,
        })
    }
    fn properties(&self, path: &str, interface: &str) -> zbus::Result<HashMap<String, OwnedValue>> {
        self.connection
            .call_method(
                Some(self.owner.as_str()),
                path,
                Some("org.freedesktop.DBus.Properties"),
                "GetAll",
                &(interface,),
            )?
            .body()
            .deserialize()
    }
    fn sample(&self) -> Option<State> {
        let session = self.properties(self.session.as_str(), SESSION).ok()?;
        let manager = self.properties(PATH, MANAGER).ok()?;
        state(&session, &manager)
    }
}
pub(crate) struct Monitor {
    snapshot: Arc<Mutex<Snapshot>>,
    stop: Arc<AtomicBool>,
}
impl Monitor {
    pub fn new() -> Self {
        let snapshot = Arc::new(Mutex::new(Snapshot::default()));
        let stop = Arc::new(AtomicBool::new(false));
        let (output, done) = (snapshot.clone(), stop.clone());
        thread::spawn(move || {
            // Pin one unique service owner and session path for this pilot. A service
            // replacement/disconnect needs a fresh pilot; it cannot silently unlock.
            let Ok(source) = Source::new() else {
                return;
            };
            while !done.load(Ordering::Relaxed) {
                let at = SystemBootClock.now_ms().ok();
                let observed = source.sample();
                if let Ok(mut output) = output.lock() {
                    output.sample = observed.zip(at);
                }
                thread::sleep(Duration::from_millis(SAMPLE_MS));
            }
        });
        Self { snapshot, stop }
    }
    pub fn state(&self) -> State {
        let Ok(now) = SystemBootClock.now_ms() else {
            return State::Unknown;
        };
        self.snapshot
            .lock()
            .map(|s| s.state(now))
            .unwrap_or(State::Unknown)
    }
}
impl Drop for Monitor {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    fn properties() -> (HashMap<String, OwnedValue>, HashMap<String, OwnedValue>) {
        use zbus::zvariant::Str;
        let mut s = HashMap::new();
        s.insert("Type".into(), Str::from("x11").into());
        s.insert("Class".into(), Str::from("user").into());
        for (name, value) in [("Active", true), ("LockedHint", false), ("Remote", false)] {
            s.insert(name.into(), value.into());
        }
        let m = HashMap::from([("PreparingForSleep".into(), false.into())]);
        (s, m)
    }
    #[test]
    fn only_complete_local_unlocked_x11_state_is_active() {
        let (mut s, mut m) = properties();
        assert_eq!(state(&s, &m), Some(State::Active));
        for (key, value) in [("Active", false), ("LockedHint", true), ("Remote", true)] {
            let (mut altered, m) = properties();
            altered.insert(key.into(), value.into());
            assert_eq!(state(&altered, &m), Some(State::Idle));
        }
        m.insert("PreparingForSleep".into(), true.into());
        assert_eq!(state(&s, &m), Some(State::Idle));
        s.remove("LockedHint");
        assert_eq!(state(&s, &m), None);
    }
    #[test]
    fn absent_wrong_typed_and_unsupported_session_properties_never_activate() {
        use zbus::zvariant::Str;
        for key in ["Type", "Class", "Active", "LockedHint", "Remote"] {
            let (mut s, m) = properties();
            s.remove(key);
            assert_eq!(state(&s, &m), None);
        }
        let (s, mut m) = properties();
        m.clear();
        assert_eq!(state(&s, &m), None);
        let (mut s, m) = properties();
        s.insert("Active".into(), Str::from("false").into());
        assert_eq!(state(&s, &m), None);
        for (key, value) in [("Type", "wayland"), ("Class", "greeter")] {
            let (mut s, m) = properties();
            s.insert(key.into(), Str::from(value).into());
            assert_eq!(state(&s, &m), Some(State::Idle));
        }
    }
    #[test]
    fn missing_stale_or_backward_clock_samples_are_unknown() {
        assert_eq!(Snapshot::default().state(100), State::Unknown);
        let s = Snapshot {
            sample: Some((State::Active, 100)),
        };
        assert_eq!(s.state(100), State::Active);
        assert_eq!(s.state(401), State::Unknown);
        assert_eq!(s.state(99), State::Unknown);
    }
}

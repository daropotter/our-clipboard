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
const ACTIVE_CONFIRM_MS: u64 = SAMPLE_MS;
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum State {
    Active,
    Idle,
    Unknown,
}
#[derive(Default)]
struct Snapshot {
    sample: Option<(State, u64)>,
    active_since: Option<u64>,
}
impl Snapshot {
    fn record(&mut self, observation: Option<(State, u64)>, now: u64) {
        // The old cache must still be fresh when the new query completes. A
        // slow query cannot bridge a stale interval using its earlier start time.
        if self
            .sample
            .is_none_or(|(_, at)| now < at || now - at > MAX_AGE_MS)
        {
            self.active_since = None;
        }
        let Some((observed, at)) = observation.filter(|(_, at)| {
            now >= *at
                && now - *at <= MAX_AGE_MS
                && self.sample.is_none_or(|(_, previous)| *at >= previous)
        }) else {
            *self = Self::default();
            return;
        };
        let reported = if observed == State::Active {
            let since = *self.active_since.get_or_insert(at);
            if at.saturating_sub(since) >= ACTIVE_CONFIRM_MS {
                State::Active
            } else {
                State::Unknown
            }
        } else {
            self.active_since = None;
            observed
        };
        self.sample = Some((reported, at));
    }
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
    let (uid, _): (u32, OwnedObjectPath) =
        properties.get("User")?.try_clone().ok()?.try_into().ok()?;
    if uid != rustix::process::geteuid().as_raw() {
        return None;
    }
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
    display: Option<x11rb::rust_connection::RustConnection>,
}
impl Source {
    fn new() -> Option<Self> {
        let connection = zbus::blocking::connection::Builder::system()
            .ok()?
            .method_timeout(Duration::from_millis(200))
            .build()
            .ok()?;
        let bus = Proxy::new(
            &connection,
            "org.freedesktop.DBus",
            "/org/freedesktop/DBus",
            "org.freedesktop.DBus",
        )
        .ok()?;
        let owner: String = bus
            .call("GetNameOwner", &("org.freedesktop.login1",))
            .ok()?;
        let manager = Proxy::new(&connection, owner.as_str(), PATH, MANAGER).ok()?;
        let direct: zbus::Result<OwnedObjectPath> =
            manager.call("GetSessionByPID", &(std::process::id(),));
        let (session, display) = match direct {
            Ok(path) => (path, None),
            Err(_) => {
                // User-manager/terminal children may have no logind session. Bind
                // to the actual local X11 socket peer, not XDG_SESSION_ID or a
                // guessed user's Display property. TCP/remote X11 has no Unix
                // peer credential and therefore cannot take this fallback.
                let (display, _) = x11rb::connect(None).ok()?;
                if rustix::net::sockopt::socket_domain(display.stream()).ok()?
                    != rustix::net::AddressFamily::UNIX
                {
                    return None;
                }
                let peer = rustix::net::sockopt::socket_peercred(display.stream()).ok()?;
                let pid = u32::try_from(peer.pid.as_raw_pid()).ok()?;
                let path = manager.call("GetSessionByPID", &(pid,)).ok()?;
                (path, Some(display))
            }
        };
        drop(manager);
        drop(bus);
        Some(Self {
            connection,
            owner,
            session,
            display,
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
        if let Some(display) = &self.display {
            use x11rb::connection::Connection as _;
            display.poll_for_event().ok()?;
        }
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
            let Some(source) = Source::new() else {
                return;
            };
            while !done.load(Ordering::Relaxed) {
                let at = SystemBootClock.now_ms().ok();
                let observed = source.sample();
                if let Ok(mut output) = output.lock() {
                    if let Ok(now) = SystemBootClock.now_ms() {
                        output.record(observed.zip(at), now);
                    } else {
                        *output = Snapshot::default();
                    }
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
/// Observe the same confirmed cache as the pilot without reading clipboard data.
pub(crate) fn probe() -> State {
    let monitor = Monitor::new();
    let deadline = std::time::Instant::now() + Duration::from_secs(1);
    loop {
        let state = monitor.state();
        if state != State::Unknown || std::time::Instant::now() >= deadline {
            return state;
        }
        thread::sleep(Duration::from_millis(10));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn properties() -> (HashMap<String, OwnedValue>, HashMap<String, OwnedValue>) {
        use zbus::zvariant::Str;
        let mut s = HashMap::new();
        s.insert(
            "User".into(),
            OwnedValue::try_from(zbus::zvariant::Value::from((
                rustix::process::geteuid().as_raw(),
                OwnedObjectPath::try_from("/org/freedesktop/login1/user/test").unwrap(),
            )))
            .unwrap(),
        );
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
        for key in ["User", "Type", "Class", "Active", "LockedHint", "Remote"] {
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
    fn another_users_session_is_never_accepted() {
        let (mut s, m) = properties();
        s.insert(
            "User".into(),
            OwnedValue::try_from(zbus::zvariant::Value::from((
                rustix::process::geteuid().as_raw().wrapping_add(1),
                OwnedObjectPath::try_from("/org/freedesktop/login1/user/other").unwrap(),
            )))
            .unwrap(),
        );
        assert_eq!(state(&s, &m), None);
    }
    #[test]
    #[ignore = "Reads actual login1/local X11 state without clipboard access"]
    fn live_local_session_binding_is_available() {
        let source = Source::new().expect("No unambiguous local logind/X11 binding available");
        assert!(
            source.sample().is_some(),
            "Local session properties were incomplete or foreign"
        );
    }
    #[test]
    fn one_active_sample_between_idle_observations_cannot_unlock() {
        let mut snapshot = Snapshot::default();
        snapshot.record(Some((State::Idle, 100)), 100);
        snapshot.record(Some((State::Active, 175)), 175);
        assert_eq!(snapshot.state(175), State::Unknown);
        snapshot.record(Some((State::Idle, 250)), 250);
        assert_eq!(snapshot.state(250), State::Idle);
    }
    #[test]
    fn active_confirmation_takes_time_but_idle_is_immediate() {
        let mut snapshot = Snapshot::default();
        snapshot.record(Some((State::Active, 100)), 100);
        assert_eq!(snapshot.state(100), State::Unknown);
        snapshot.record(Some((State::Active, 174)), 174);
        assert_eq!(snapshot.state(174), State::Unknown);
        snapshot.record(Some((State::Active, 175)), 175);
        assert_eq!(snapshot.state(175), State::Active);
        snapshot.record(Some((State::Idle, 176)), 176);
        assert_eq!(snapshot.state(176), State::Idle);
    }
    #[test]
    fn missing_or_unknown_observations_require_new_active_confirmation() {
        for observation in [None, Some((State::Unknown, 200)), Some((State::Idle, 200))] {
            let mut snapshot = Snapshot::default();
            snapshot.record(Some((State::Active, 100)), 100);
            snapshot.record(Some((State::Active, 175)), 175);
            assert_eq!(snapshot.state(175), State::Active);
            snapshot.record(observation, 200);
            assert_ne!(snapshot.state(200), State::Active);
            snapshot.record(Some((State::Active, 275)), 275);
            assert_eq!(snapshot.state(275), State::Unknown);
            snapshot.record(Some((State::Active, 350)), 350);
            assert_eq!(snapshot.state(350), State::Active);
        }
    }
    #[test]
    fn stale_late_future_or_regressed_samples_cannot_bridge_active_confirmation() {
        for (observation, now) in [
            ((State::Active, 500), 500), // A gap longer than the cache lifetime.
            ((State::Active, 150), 250), // Query start moved backwards.
            ((State::Active, 500), 801), // Query completed too late.
            ((State::Active, 900), 850), // Completion clock moved backwards.
            ((State::Active, 350), 500), // Start gap is fresh, completion gap is stale.
        ] {
            let mut snapshot = Snapshot::default();
            snapshot.record(Some((State::Active, 100)), 100);
            snapshot.record(Some((State::Active, 175)), 175);
            snapshot.record(Some(observation), now);
            assert_ne!(snapshot.state(now), State::Active);
            snapshot.record(Some((State::Active, 1000)), 1000);
            assert_eq!(snapshot.state(1000), State::Unknown);
            snapshot.record(Some((State::Active, 1075)), 1075);
            assert_eq!(snapshot.state(1075), State::Active);
        }
    }
    #[test]
    fn missing_stale_or_backward_clock_samples_are_unknown() {
        assert_eq!(Snapshot::default().state(100), State::Unknown);
        let mut s = Snapshot::default();
        s.record(Some((State::Active, 25)), 25);
        s.record(Some((State::Active, 100)), 100);
        assert_eq!(s.state(100), State::Active);
        assert_eq!(s.state(401), State::Unknown);
        assert_eq!(s.state(99), State::Unknown);
    }
}

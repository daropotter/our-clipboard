//! Test-only login1 service for private container D-Bus instances. Never run on a system bus.
#[cfg(target_os = "linux")]
mod fixture {
    use std::{path::PathBuf, thread, time::Duration};
    use zbus::{fdo, zvariant::OwnedObjectPath};
    #[derive(Clone)]
    struct State(PathBuf);
    impl State {
        fn read(&self) -> String {
            std::fs::read_to_string(&self.0).unwrap_or_else(|_| "unknown".into())
        }
    }
    struct Manager(State);
    #[zbus::interface(name = "org.freedesktop.login1.Manager")]
    impl Manager {
        #[zbus(name = "GetSessionByPID")]
        fn get_session_by_pid(&self, pid: u32) -> fdo::Result<OwnedObjectPath> {
            let expected = std::fs::read_to_string(self.0.0.with_extension("pid"))
                .ok()
                .and_then(|p| p.parse::<u32>().ok());
            if expected != Some(pid) {
                return Err(fdo::Error::Failed("No session for this PID".into()));
            }
            if pid == 0 {
                return Err(fdo::Error::InvalidArgs("Explicit PID required".into()));
            }
            Ok("/org/freedesktop/login1/session/test".try_into().unwrap())
        }
        #[zbus(property)]
        fn preparing_for_sleep(&self) -> bool {
            self.0.read() == "sleep"
        }
    }
    struct Session(State);
    #[zbus::interface(name = "org.freedesktop.login1.Session")]
    impl Session {
        #[zbus(property)]
        fn user(&self) -> (u32, OwnedObjectPath) {
            (
                rustix::process::geteuid().as_raw(),
                "/org/freedesktop/login1/user/test".try_into().unwrap(),
            )
        }
        #[zbus(property, name = "Type")]
        fn kind(&self) -> &str {
            "x11"
        }
        #[zbus(property)]
        fn class(&self) -> &str {
            "user"
        }
        #[zbus(property)]
        fn active(&self) -> bool {
            self.0.read() != "inactive"
        }
        #[zbus(property)]
        fn remote(&self) -> bool {
            false
        }
        #[zbus(property)]
        fn locked_hint(&self) -> fdo::Result<bool> {
            match self.0.read().as_str() {
                "unknown" => Err(fdo::Error::Failed("Test state unavailable".into())),
                "locked" => Ok(true),
                _ => Ok(false),
            }
        }
    }
    pub fn run() -> Result<(), Box<dyn std::error::Error>> {
        let args: Vec<_> = std::env::args_os().collect();
        if args.len() != 3 {
            return Err("Expected state and readiness paths".into());
        }
        let state = State(args[1].clone().into());
        let _connection = zbus::blocking::connection::Builder::system()?
            .serve_at("/org/freedesktop/login1", Manager(state.clone()))?
            .serve_at("/org/freedesktop/login1/session/test", Session(state))?
            .name("org.freedesktop.login1")?
            .build()?;
        std::fs::write(&args[2], [])?;
        loop {
            thread::sleep(Duration::from_secs(1));
        }
    }
}
#[cfg(target_os = "linux")]
fn main() {
    if fixture::run().is_err() {
        eprintln!("Private login1 fixture failed; details omitted.");
        std::process::exit(2);
    }
}
#[cfg(not(target_os = "linux"))]
fn main() {
    eprintln!("Private login1 fixture requires Linux.");
}

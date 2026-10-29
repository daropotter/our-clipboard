//! Content-free, opt-in JSON Lines on the pilot's preserved stdout descriptor.
use super::*;
use serde::Serialize;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub(super) enum State {
    Starting,
    WaitingForPeerOffer,
    Connecting,
    Synchronizing,
    Disconnected,
    Idle,
    Ready,
    Paused,
    Ended,
    Failed,
}
impl State {
    pub(super) fn session(
        connected: bool,
        synchronized: bool,
        idle: bool,
        clipboard: bool,
    ) -> Self {
        if !connected {
            Self::Disconnected
        } else if idle {
            Self::Idle
        } else if synchronized && clipboard {
            Self::Ready
        } else {
            Self::Synchronizing
        }
    }
}
#[derive(Clone, Copy, Serialize)]
#[serde(rename_all = "snake_case")]
pub(super) enum Event {
    LocalCopySelected,
    LocalApplied,
    PeerBufferedIdle,
    PeerApplicationConfirmed,
    LocalEventExpired,
}
#[derive(Serialize)]
#[serde(untagged)]
enum Record {
    State { version: u8, state: State },
    Event { version: u8, event: Event },
}
pub(super) struct Reporter<W: Write = File> {
    output: Option<W>,
    last_state: Option<State>,
}
impl<W: Write> Reporter<W> {
    pub(super) fn new(output: Option<W>) -> Self {
        Self {
            output,
            last_state: None,
        }
    }
    fn write(&mut self, record: Record) -> Result<()> {
        if let Some(output) = &mut self.output {
            serde_json::to_writer(&mut *output, &record).map_err(|_| Error::Io)?;
            output.write_all(b"\n").map_err(|_| Error::Io)?;
            output.flush().map_err(|_| Error::Io)?;
        }
        Ok(())
    }
    pub(super) fn state(&mut self, state: State) -> Result<()> {
        if self.last_state != Some(state) {
            self.write(Record::State { version: 1, state })?;
            self.last_state = Some(state);
        }
        Ok(())
    }
    pub(super) fn event(&mut self, event: Event) -> Result<()> {
        self.write(Record::Event { version: 1, event })
    }
    pub(super) fn finish(&mut self, success: bool) -> Result<()> {
        if !success {
            self.state(State::Failed)
        } else if self.last_state == Some(State::Paused) {
            Ok(())
        } else {
            self.state(State::Ended)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn readiness_requires_transport_handshake_active_mode_and_native_session() {
        assert_eq!(
            State::session(false, true, false, true),
            State::Disconnected
        );
        assert_eq!(
            State::session(true, false, false, true),
            State::Synchronizing
        );
        assert_eq!(
            State::session(true, true, false, false),
            State::Synchronizing
        );
        assert_eq!(State::session(true, true, true, true), State::Idle);
        assert_eq!(State::session(true, true, false, true), State::Ready);
    }
    #[test]
    fn state_changes_are_deduplicated_and_events_remain_distinct() {
        let mut reporter = Reporter::new(Some(Vec::new()));
        reporter.state(State::Starting).unwrap();
        reporter.state(State::Starting).unwrap();
        reporter.state(State::Ready).unwrap();
        reporter.event(Event::LocalApplied).unwrap();
        reporter.event(Event::PeerApplicationConfirmed).unwrap();
        reporter.finish(true).unwrap();
        let records: Vec<serde_json::Value> = String::from_utf8(reporter.output.unwrap())
            .unwrap()
            .lines()
            .map(|line| serde_json::from_str(line).unwrap())
            .collect();
        assert_eq!(
            records,
            [
                serde_json::json!({"version":1,"state":"starting"}),
                serde_json::json!({"version":1,"state":"ready"}),
                serde_json::json!({"version":1,"event":"local_applied"}),
                serde_json::json!({"version":1,"event":"peer_application_confirmed"}),
                serde_json::json!({"version":1,"state":"ended"}),
            ]
        );
    }
    #[test]
    fn pause_is_terminal_and_output_errors_are_not_success() {
        let mut reporter = Reporter::new(Some(Vec::new()));
        reporter.state(State::Paused).unwrap();
        reporter.finish(true).unwrap();
        assert_eq!(reporter.last_state, Some(State::Paused));
        reporter.finish(false).unwrap();
        assert_eq!(reporter.last_state, Some(State::Failed));
        let mut broken = Reporter::new(Some(std::io::Cursor::new([0u8; 0])));
        assert_eq!(broken.state(State::Starting), Err(Error::Io));
        assert_eq!(broken.last_state, None);
    }
}

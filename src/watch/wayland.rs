//! Native data-control subscriptions. Offers are discarded without requesting data.
use rustix::event::{PollFd, PollFlags, Timespec, poll};
use wayland_client::{
    Connection, Dispatch, EventQueue, Proxy, QueueHandle, delegate_noop, event_created_child,
    protocol::{wl_registry, wl_seat::WlSeat},
};
use wayland_protocols::ext::data_control::v1::client::{
    ext_data_control_device_v1::{self, ExtDataControlDeviceV1 as ExtDevice},
    ext_data_control_manager_v1::ExtDataControlManagerV1 as ExtManager,
    ext_data_control_offer_v1::ExtDataControlOfferV1 as ExtOffer,
};
use wayland_protocols_wlr::data_control::v1::client::{
    zwlr_data_control_device_v1::{self, ZwlrDataControlDeviceV1 as WlrDevice},
    zwlr_data_control_manager_v1::ZwlrDataControlManagerV1 as WlrManager,
    zwlr_data_control_offer_v1::ZwlrDataControlOfferV1 as WlrOffer,
};

const UNAVAILABLE: &str = "Native Wayland notifications unavailable. A data-control protocol and a seat are required; XWayland fallback is disabled.";
const FAILED: &str = "Native Wayland notification connection failed.";

pub struct Watcher {
    connection: Connection,
    queue: EventQueue<State>,
    state: State,
    name: &'static str,
}

#[derive(Default)]
struct State {
    ext: Option<ExtManager>,
    wlr: Option<WlrManager>,
    seat: Option<WlSeat>,
    ext_offers: Vec<ExtOffer>,
    wlr_offers: Vec<WlrOffer>,
    pending: u64,
    failed: bool,
}

impl Watcher {
    pub fn new() -> Result<Self, &'static str> {
        let connection = Connection::connect_to_env().map_err(|_| UNAVAILABLE)?;
        let mut queue = connection.new_event_queue();
        let qh = queue.handle();
        let mut state = State::default();
        connection.display().get_registry(&qh, ());
        queue.roundtrip(&mut state).map_err(|_| UNAVAILABLE)?;
        let seat = state.seat.as_ref().ok_or(UNAVAILABLE)?;
        // Prefer the standardized protocol when both are advertised.
        let name = if let Some(manager) = &state.ext {
            manager.get_data_device(seat, &qh, ());
            "Wayland ext-data-control"
        } else if let Some(manager) = &state.wlr {
            manager.get_data_device(seat, &qh, ());
            "Wayland wlr-data-control"
        } else {
            return Err(UNAVAILABLE);
        };
        queue.roundtrip(&mut state).map_err(|_| UNAVAILABLE)?;
        if state.failed {
            return Err(UNAVAILABLE);
        }
        // The compositor sends the existing selection on subscription. It is a baseline.
        state.pending = 0;
        Ok(Self {
            connection,
            queue,
            state,
            name,
        })
    }

    pub fn backend_name(&self) -> &'static str {
        self.name
    }

    pub fn changed(&mut self) -> Result<bool, &'static str> {
        self.queue
            .dispatch_pending(&mut self.state)
            .map_err(|_| FAILED)?;
        self.connection.flush().map_err(|_| FAILED)?;
        if let Some(guard) = self.queue.prepare_read() {
            let mut fds = [PollFd::from_borrowed_fd(
                guard.connection_fd(),
                PollFlags::IN,
            )];
            let ready = poll(
                &mut fds,
                Some(&Timespec {
                    tv_sec: 0,
                    tv_nsec: 0,
                }),
            )
            .map_err(|_| FAILED)?;
            if ready > 0 {
                guard.read().map_err(|_| FAILED)?;
            }
        }
        self.queue
            .dispatch_pending(&mut self.state)
            .map_err(|_| FAILED)?;
        if self.state.failed {
            return Err(FAILED);
        }
        if self.state.pending > 0 {
            self.state.pending -= 1;
            Ok(true)
        } else {
            Ok(false)
        }
    }
}

impl Dispatch<wl_registry::WlRegistry, ()> for State {
    fn event(
        state: &mut Self,
        registry: &wl_registry::WlRegistry,
        event: wl_registry::Event,
        _: &(),
        _: &Connection,
        qh: &QueueHandle<Self>,
    ) {
        if let wl_registry::Event::Global {
            name,
            interface,
            version,
        } = event
        {
            if interface == ExtManager::interface().name && state.ext.is_none() {
                state.ext = Some(registry.bind(name, 1, qh, ()));
            } else if interface == WlrManager::interface().name && state.wlr.is_none() {
                state.wlr = Some(registry.bind(name, version.min(2), qh, ()));
            } else if interface == WlSeat::interface().name && state.seat.is_none() {
                state.seat = Some(registry.bind(name, version.min(2), qh, ()));
            }
        }
    }
}

delegate_noop!(State: ignore WlSeat);
delegate_noop!(State: ignore ExtManager);
delegate_noop!(State: ignore WlrManager);
delegate_noop!(State: ignore ExtOffer);
delegate_noop!(State: ignore WlrOffer);

impl Dispatch<ExtDevice, ()> for State {
    fn event(
        state: &mut Self,
        _: &ExtDevice,
        event: ext_data_control_device_v1::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        use ext_data_control_device_v1::Event;
        if matches!(&event, Event::Selection { .. }) {
            state.pending = state.pending.saturating_add(1);
        }
        match event {
            Event::DataOffer { id } => {
                state.ext_offers.push(id);
                if state.ext_offers.len() > 16 {
                    state.failed = true;
                }
            }
            Event::Selection { id } | Event::PrimarySelection { id } => {
                state.ext_offers.retain(|offer| {
                    if id.as_ref() == Some(offer) {
                        offer.destroy();
                        false
                    } else {
                        true
                    }
                });
                // Destroy offer metadata without reading text; PRIMARY is not counted.
            }
            Event::Finished => state.failed = true,
            _ => {}
        }
    }
    event_created_child!(State, ExtDevice, [0 => (ExtOffer, ())]);
}

impl Dispatch<WlrDevice, ()> for State {
    fn event(
        state: &mut Self,
        _: &WlrDevice,
        event: zwlr_data_control_device_v1::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        use zwlr_data_control_device_v1::Event;
        if matches!(&event, Event::Selection { .. }) {
            state.pending = state.pending.saturating_add(1);
        }
        match event {
            Event::DataOffer { id } => {
                state.wlr_offers.push(id);
                if state.wlr_offers.len() > 16 {
                    state.failed = true;
                }
            }
            Event::Selection { id } | Event::PrimarySelection { id } => {
                state.wlr_offers.retain(|offer| {
                    if id.as_ref() == Some(offer) {
                        offer.destroy();
                        false
                    } else {
                        true
                    }
                });
            }
            Event::Finished => state.failed = true,
            _ => {}
        }
    }
    event_created_child!(State, WlrDevice, [0 => (WlrOffer, ())]);
}

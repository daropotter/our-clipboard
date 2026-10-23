use x11rb::{
    connection::Connection,
    protocol::{
        Event,
        xfixes::{ConnectionExt as _, SelectionEventMask},
        xproto::ConnectionExt as _,
    },
    rust_connection::RustConnection,
};

pub struct Watcher {
    connection: RustConnection,
}

impl Watcher {
    pub fn new() -> Result<Self, &'static str> {
        Self::connect().map_err(
            |_| "X11 clipboard notifications unavailable. Check DISPLAY and XFixes support.",
        )
    }

    fn connect() -> Result<Self, Box<dyn std::error::Error>> {
        let (connection, screen) = x11rb::connect(None)?;
        connection.xfixes_query_version(5, 0)?.reply()?;
        let clipboard = connection.intern_atom(false, b"CLIPBOARD")?.reply()?.atom;
        let root = connection.setup().roots[screen].root;
        connection
            .xfixes_select_selection_input(
                root,
                clipboard,
                SelectionEventMask::SET_SELECTION_OWNER
                    | SelectionEventMask::SELECTION_WINDOW_DESTROY
                    | SelectionEventMask::SELECTION_CLIENT_CLOSE,
            )?
            .check()?;
        // A checked request synchronizes subscription before the CLI announces readiness.
        Ok(Self { connection })
    }

    pub fn changed(&self) -> Result<bool, &'static str> {
        while let Some(event) = self
            .connection
            .poll_for_event()
            .map_err(|_| "X11 clipboard notification connection failed.")?
        {
            match event {
                Event::XfixesSelectionNotify(_) => return Ok(true),
                Event::Error(_) => return Err("X11 clipboard notification request failed."),
                _ => {}
            }
        }
        Ok(false)
    }
}

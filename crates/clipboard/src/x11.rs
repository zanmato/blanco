//! The X11 CLIPBOARD selection. Adapted from GPUI's X11 clipboard, itself
//! adapted from arboard (Apache-2.0 OR MIT): a hidden window owns the
//! selection, and a thread answers other programs' requests for it with the
//! representation they ask for. Reading stays with GPUI, which asks the X
//! server and so reaches this window like any other owner.

use std::collections::HashMap;
use std::sync::{Arc, Mutex, OnceLock, RwLock};

use anyhow::{Context as _, Result};
use x11rb::{
    COPY_DEPTH_FROM_PARENT, COPY_FROM_PARENT,
    connection::Connection as _,
    protocol::{
        Event,
        xproto::{
            Atom, AtomEnum, ConnectionExt as _, CreateWindowAux, EventMask, PropMode,
            SELECTION_NOTIFY_EVENT, SelectionNotifyEvent, SelectionRequestEvent, Time, WindowClass,
        },
    },
    rust_connection::RustConnection,
    wrapper::ConnectionExt as _,
};

use crate::{Representation, TEXT_PLAIN};

x11rb::atom_manager! {
    Atoms: AtomCookies {
        CLIPBOARD,
        TARGETS,
        ATOM,
        UTF8_STRING,
        UTF8_MIME_0: b"text/plain;charset=utf-8",
        UTF8_MIME_1: b"text/plain;charset=UTF-8",
        TEXT_MIME_UNKNOWN: b"text/plain",
    }
}

struct Server {
    connection: RustConnection,
    window: u32,
    atoms: Atoms,
    /// What the selection holds while this window owns it, by target.
    contents: RwLock<Option<HashMap<Atom, Arc<Vec<u8>>>>>,
}

static SERVER: OnceLock<Mutex<Option<Arc<Server>>>> = OnceLock::new();

pub(crate) fn write(representations: Vec<Representation>) -> Result<()> {
    let server = server()?;
    let mut contents = HashMap::new();
    for representation in representations {
        let bytes = Arc::new(representation.bytes);
        if representation.mime_type == TEXT_PLAIN {
            for target in [
                server.atoms.UTF8_STRING,
                server.atoms.UTF8_MIME_0,
                server.atoms.UTF8_MIME_1,
                server.atoms.TEXT_MIME_UNKNOWN,
            ] {
                contents.insert(target, bytes.clone());
            }
        } else {
            let target = server
                .connection
                .intern_atom(false, representation.mime_type.as_bytes())?
                .reply()?
                .atom;
            contents.insert(target, bytes);
        }
    }
    *server
        .contents
        .write()
        .map_err(|_| anyhow::anyhow!("clipboard contents lock poisoned"))? = Some(contents);
    // ICCCM 2.6.1.3: assert ownership again whenever the data changes.
    server.connection.set_selection_owner(
        server.window,
        server.atoms.CLIPBOARD,
        Time::CURRENT_TIME,
    )?;
    server.connection.flush()?;
    let owner = server
        .connection
        .get_selection_owner(server.atoms.CLIPBOARD)?
        .reply()?
        .owner;
    anyhow::ensure!(owner == server.window, "another client kept the clipboard");
    Ok(())
}

/// The process's selection server, started on first use and started again
/// if its thread has stopped.
fn server() -> Result<Arc<Server>> {
    let mut slot = SERVER
        .get_or_init(|| Mutex::new(None))
        .lock()
        .map_err(|_| anyhow::anyhow!("clipboard server lock poisoned"))?;
    if let Some(server) = slot.as_ref() {
        return Ok(server.clone());
    }
    let server = Arc::new(Server::connect()?);
    std::thread::Builder::new()
        .name("Clipboard".to_owned())
        .spawn({
            let server = server.clone();
            move || {
                if let Err(error) = serve_requests(&server) {
                    tracing::error!("The X11 clipboard server stopped: {error:#}");
                }
                if let Ok(mut slot) = SERVER.get_or_init(|| Mutex::new(None)).lock() {
                    *slot = None;
                }
            }
        })
        .context("starting the clipboard thread")?;
    *slot = Some(server.clone());
    Ok(server)
}

impl Server {
    fn connect() -> Result<Self> {
        let (connection, screen_number) =
            RustConnection::connect(None).context("connecting to the X server")?;
        let screen = connection
            .setup()
            .roots
            .get(screen_number)
            .context("the X server has no screen")?;
        let window = connection.generate_id()?;
        connection.create_window(
            COPY_DEPTH_FROM_PARENT,
            window,
            screen.root,
            0,
            0,
            1,
            1,
            0,
            WindowClass::COPY_FROM_PARENT,
            COPY_FROM_PARENT,
            &CreateWindowAux::new()
                .event_mask(EventMask::PROPERTY_CHANGE | EventMask::STRUCTURE_NOTIFY),
        )?;
        connection.flush()?;
        let atoms = Atoms::new(&connection)?.reply()?;
        Ok(Self {
            connection,
            window,
            atoms,
            contents: RwLock::new(None),
        })
    }

    fn handle_selection_request(&self, event: SelectionRequestEvent) -> Result<()> {
        let property = if event.selection != self.atoms.CLIPBOARD {
            None
        } else if event.target == self.atoms.TARGETS {
            let contents = self
                .contents
                .read()
                .map_err(|_| anyhow::anyhow!("clipboard contents lock poisoned"))?;
            let mut targets = vec![self.atoms.TARGETS];
            targets.extend(contents.iter().flat_map(|contents| contents.keys()));
            self.connection.change_property32(
                PropMode::REPLACE,
                event.requestor,
                event.property,
                self.atoms.ATOM,
                &targets,
            )?;
            Some(event.property)
        } else {
            let contents = self
                .contents
                .read()
                .map_err(|_| anyhow::anyhow!("clipboard contents lock poisoned"))?;
            match contents
                .as_ref()
                .and_then(|contents| contents.get(&event.target))
            {
                Some(bytes) => {
                    self.connection.change_property8(
                        PropMode::REPLACE,
                        event.requestor,
                        event.property,
                        event.target,
                        bytes,
                    )?;
                    Some(event.property)
                }
                // Lost the selection since the request was made, or a
                // target this copy does not have.
                None => None,
            }
        };
        self.connection.send_event(
            false,
            event.requestor,
            EventMask::NO_EVENT,
            SelectionNotifyEvent {
                response_type: SELECTION_NOTIFY_EVENT,
                sequence: event.sequence,
                time: event.time,
                requestor: event.requestor,
                selection: event.selection,
                target: event.target,
                property: property.unwrap_or_else(|| AtomEnum::NONE.into()),
            },
        )?;
        self.connection.flush()?;
        Ok(())
    }
}

fn serve_requests(server: &Server) -> Result<()> {
    loop {
        match server.connection.wait_for_event()? {
            Event::DestroyNotify(_) => return Ok(()),
            Event::SelectionClear(event) if event.selection == server.atoms.CLIPBOARD => {
                // Another program copied; the copy is theirs to serve now.
                *server
                    .contents
                    .write()
                    .map_err(|_| anyhow::anyhow!("clipboard contents lock poisoned"))? = None;
            }
            Event::SelectionRequest(event) => server.handle_selection_request(event)?,
            _ => {}
        }
    }
}

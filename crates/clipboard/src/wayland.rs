//! The Wayland clipboard, on the connection GPUI opened. A client may only
//! take the selection with the serial of a recent input event on one of its
//! surfaces, and a second connection has no surfaces. Sharing GPUI's
//! `wl_display`, with a seat bound on a queue of our own, delivers the same
//! key and button events, and with them their serials. Writing follows
//! GPUI's `write_to_clipboard`: a data source offering each MIME type, set
//! as the selection, answered from a thread of our own.

use std::collections::HashMap;
use std::ffi::c_void;
use std::io::Write as _;
use std::os::fd::OwnedFd;
use std::ptr::NonNull;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Arc, Mutex, OnceLock};

use anyhow::{Context as _, Result};
use wayland_backend::client::Backend;
use wayland_client::{
    Connection, Dispatch, EventQueue, QueueHandle, WEnum, event_created_child,
    protocol::{
        wl_data_device::{self, WlDataDevice},
        wl_data_device_manager::WlDataDeviceManager,
        wl_data_offer::WlDataOffer,
        wl_data_source::{self, WlDataSource},
        wl_keyboard::{self, WlKeyboard},
        wl_pointer::{self, WlPointer},
        wl_registry::{self, WlRegistry},
        wl_seat::{self, WlSeat},
    },
};

use crate::{Representation, TEXT_PLAIN};

/// The names a plain text representation is offered under, as GPUI offers
/// its own text.
const TEXT_MIME_TYPES: [&str; 3] = ["text/plain;charset=utf-8", "UTF8_STRING", "text/plain"];

pub(crate) struct Clipboard {
    connection: Connection,
    queue_handle: QueueHandle<State>,
    data_device_manager: WlDataDeviceManager,
    data_device: WlDataDevice,
    /// The serial of the latest key press, click or keyboard focus.
    serial: Arc<AtomicU32>,
}

/// The clipboard for the one display GPUI connects to.
static CLIPBOARD: OnceLock<Mutex<Option<Arc<Clipboard>>>> = OnceLock::new();

pub(crate) fn init(display: NonNull<c_void>) -> Result<Arc<Clipboard>> {
    let mut slot = CLIPBOARD
        .get_or_init(|| Mutex::new(None))
        .lock()
        .map_err(|_| anyhow::anyhow!("clipboard lock poisoned"))?;
    if let Some(clipboard) = slot.as_ref() {
        return Ok(clipboard.clone());
    }
    let clipboard = Arc::new(Clipboard::connect(display)?);
    *slot = Some(clipboard.clone());
    Ok(clipboard)
}

pub(crate) fn write(display: NonNull<c_void>, representations: Vec<Representation>) -> Result<()> {
    init(display)?.write(representations)
}

impl Clipboard {
    fn connect(display: NonNull<c_void>) -> Result<Self> {
        // SAFETY: the pointer is the `wl_display` of a GPUI window, which
        // GPUI keeps connected for the life of the process. A backend made
        // from a foreign display never disconnects it.
        let backend = unsafe { Backend::from_foreign_display(display.as_ptr().cast()) };
        let connection = Connection::from_backend(backend);
        let mut queue: EventQueue<State> = connection.new_event_queue();
        let queue_handle = queue.handle();
        let serial = Arc::new(AtomicU32::new(0));
        let mut state = State {
            serial: serial.clone(),
            seat: None,
            data_device_manager: None,
            keyboard: None,
            pointer: None,
        };
        connection.display().get_registry(&queue_handle, ());
        // The first round trip lists the globals, the second brings the
        // seat's capabilities.
        queue.roundtrip(&mut state)?;
        queue.roundtrip(&mut state)?;
        let seat = state.seat.clone().context("the compositor has no seat")?;
        let data_device_manager = state
            .data_device_manager
            .clone()
            .context("the compositor has no data device manager")?;
        let data_device = data_device_manager.get_data_device(&seat, &queue_handle, ());
        connection.flush()?;

        std::thread::Builder::new()
            .name("Wayland clipboard".to_owned())
            .spawn(move || {
                loop {
                    if let Err(error) = queue.blocking_dispatch(&mut state) {
                        tracing::error!("The Wayland clipboard stopped: {error}");
                        break;
                    }
                }
            })
            .context("starting the clipboard thread")?;

        Ok(Self {
            connection,
            queue_handle,
            data_device_manager,
            data_device,
            serial,
        })
    }

    fn write(&self, representations: Vec<Representation>) -> Result<()> {
        let serial = self.serial.load(Ordering::Acquire);
        anyhow::ensure!(
            serial != 0,
            "no key press or click has been seen to take the clipboard with"
        );
        let mut contents = HashMap::new();
        let mut offers = Vec::new();
        for representation in representations {
            let bytes = Arc::new(representation.bytes);
            if representation.mime_type == TEXT_PLAIN {
                for mime_type in TEXT_MIME_TYPES {
                    contents.insert(mime_type.to_string(), bytes.clone());
                    offers.push(mime_type.to_string());
                }
            } else {
                contents.insert(representation.mime_type.clone(), bytes);
                offers.push(representation.mime_type);
            }
        }
        let source = self
            .data_device_manager
            .create_data_source(&self.queue_handle, Contents(Arc::new(contents)));
        for mime_type in offers {
            source.offer(mime_type);
        }
        self.data_device.set_selection(Some(&source), serial);
        self.connection.flush()?;
        Ok(())
    }
}

/// What one data source offers, by MIME type.
struct Contents(Arc<HashMap<String, Arc<Vec<u8>>>>);

struct State {
    serial: Arc<AtomicU32>,
    seat: Option<WlSeat>,
    data_device_manager: Option<WlDataDeviceManager>,
    keyboard: Option<WlKeyboard>,
    pointer: Option<WlPointer>,
}

impl State {
    fn record_serial(&self, serial: u32) {
        self.serial.store(serial, Ordering::Release);
    }
}

impl Dispatch<WlRegistry, ()> for State {
    fn event(
        state: &mut Self,
        registry: &WlRegistry,
        event: wl_registry::Event,
        _: &(),
        _: &Connection,
        queue_handle: &QueueHandle<Self>,
    ) {
        let wl_registry::Event::Global {
            name,
            interface,
            version,
        } = event
        else {
            return;
        };
        match interface.as_str() {
            "wl_seat" if state.seat.is_none() => {
                state.seat = Some(registry.bind(name, version.min(5), queue_handle, ()));
            }
            "wl_data_device_manager" if state.data_device_manager.is_none() => {
                state.data_device_manager =
                    Some(registry.bind(name, version.min(3), queue_handle, ()));
            }
            _ => {}
        }
    }
}

impl Dispatch<WlSeat, ()> for State {
    fn event(
        state: &mut Self,
        seat: &WlSeat,
        event: wl_seat::Event,
        _: &(),
        _: &Connection,
        queue_handle: &QueueHandle<Self>,
    ) {
        let wl_seat::Event::Capabilities {
            capabilities: WEnum::Value(capabilities),
        } = event
        else {
            return;
        };
        if capabilities.contains(wl_seat::Capability::Keyboard) && state.keyboard.is_none() {
            state.keyboard = Some(seat.get_keyboard(queue_handle, ()));
        }
        if capabilities.contains(wl_seat::Capability::Pointer) && state.pointer.is_none() {
            state.pointer = Some(seat.get_pointer(queue_handle, ()));
        }
    }
}

impl Dispatch<WlKeyboard, ()> for State {
    fn event(
        state: &mut Self,
        _: &WlKeyboard,
        event: wl_keyboard::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        match event {
            wl_keyboard::Event::Enter { serial, .. } => state.record_serial(serial),
            wl_keyboard::Event::Key {
                serial,
                state: WEnum::Value(wl_keyboard::KeyState::Pressed),
                ..
            } => state.record_serial(serial),
            _ => {}
        }
    }
}

impl Dispatch<WlPointer, ()> for State {
    fn event(
        state: &mut Self,
        _: &WlPointer,
        event: wl_pointer::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        if let wl_pointer::Event::Button {
            serial,
            state: WEnum::Value(wl_pointer::ButtonState::Pressed),
            ..
        } = event
        {
            state.record_serial(serial);
        }
    }
}

impl Dispatch<WlDataDeviceManager, ()> for State {
    fn event(
        _: &mut Self,
        _: &WlDataDeviceManager,
        _: <WlDataDeviceManager as wayland_client::Proxy>::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
    }
}

impl Dispatch<WlDataDevice, ()> for State {
    fn event(
        _: &mut Self,
        _: &WlDataDevice,
        event: wl_data_device::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        // GPUI's own data device reads the clipboard and takes drops; the
        // offers this one is handed are let go at once.
        match event {
            wl_data_device::Event::Selection { id: Some(offer) }
            | wl_data_device::Event::Enter {
                id: Some(offer), ..
            } => offer.destroy(),
            _ => {}
        }
    }

    event_created_child!(State, WlDataDevice, [
        wl_data_device::EVT_DATA_OFFER_OPCODE => (WlDataOffer, ()),
    ]);
}

impl Dispatch<WlDataOffer, ()> for State {
    fn event(
        _: &mut Self,
        _: &WlDataOffer,
        _: <WlDataOffer as wayland_client::Proxy>::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
    }
}

impl Dispatch<WlDataSource, Contents> for State {
    fn event(
        _: &mut Self,
        source: &WlDataSource,
        event: wl_data_source::Event,
        contents: &Contents,
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        match event {
            wl_data_source::Event::Send { mime_type, fd } => match contents.0.get(&mime_type) {
                Some(bytes) => send(bytes.clone(), fd),
                None => tracing::warn!("Asked for clipboard type {mime_type} not offered"),
            },
            wl_data_source::Event::Cancelled => source.destroy(),
            _ => {}
        }
    }
}

/// Write `bytes` to the pipe a paste reads from. The reader may be GPUI on
/// the main thread, which waits for the data, so the queue's thread must not
/// block on it either: each paste gets a thread of its own.
fn send(bytes: Arc<Vec<u8>>, fd: OwnedFd) {
    let spawned = std::thread::Builder::new()
        .name("Clipboard send".to_owned())
        .spawn(move || {
            let mut file = std::fs::File::from(fd);
            if let Err(error) = file.write_all(&bytes) {
                tracing::warn!("Failed to send the clipboard: {error}");
            }
        });
    if let Err(error) = spawned {
        tracing::error!("Failed to start sending the clipboard: {error}");
    }
}

use std::collections::HashMap;
use std::io::Read;
use std::os::fd::AsFd;
use std::sync::atomic::{AtomicI64, Ordering};
use std::sync::{Arc, Mutex, OnceLock, PoisonError, mpsc};
use std::thread;

use phonelink_clipboard::pasteboard::{ClipboardPasteboard, ClipboardPasteboardData, ClipboardPasteboardWrite};
use phonelink_engine::edge::EdgeError;
use wayland_client::protocol::{wl_registry, wl_seat};
use wayland_client::{Connection, Dispatch, EventQueue, Proxy, QueueHandle, backend::ObjectId, event_created_child};

use super::{READ_TIMEOUT, edge_err, mime_data, mimes_for, own_source_mime, send_bytes, utis_of};

#[derive(Default)]
struct Selection {
    offer: Option<Offer>,
    mimes: Vec<String>,
}

struct Writer {
    manager: Manager,
    device: Device,
    queue: QueueHandle<WaylandState>,
    source: Mutex<Option<Source>>,
}

struct Shared {
    own_mime: String,
    count: AtomicI64,
    selection: Mutex<Selection>,
    writer: OnceLock<Writer>,
}

struct WaylandState {
    shared: Arc<Shared>,
    seat: Option<wl_seat::WlSeat>,
    manager: Option<Manager>,
    offered: HashMap<ObjectId, Vec<String>>,
}

type SourceData = Arc<HashMap<String, Arc<Vec<u8>>>>;

pub struct WaylandPasteboard {
    connection: Connection,
    shared: Arc<Shared>,
}

impl WaylandPasteboard {
    pub fn connect() -> Result<Self, String> {
        let connection = Connection::connect_to_env().map_err(|e| e.to_string())?;
        let mut queue: EventQueue<WaylandState> = connection.new_event_queue();
        let handle = queue.handle();
        connection.display().get_registry(&handle, ());

        let shared = Arc::new(Shared {
            own_mime: own_source_mime(),
            count: AtomicI64::new(0),
            selection: Mutex::default(),
            writer: OnceLock::new(),
        });
        let mut state = WaylandState { shared: shared.clone(), seat: None, manager: None, offered: HashMap::new() };
        queue.roundtrip(&mut state).map_err(|e| e.to_string())?;

        let (Some(seat), Some(manager)) = (state.seat.clone(), state.manager.clone()) else {
            return Err(format!("the compositor offers no seat or {MANAGER_INTERFACE}"));
        };
        let device = manager.get_data_device(&seat, &handle, ());
        let _ = shared.writer.set(Writer { manager, device, queue: handle, source: Mutex::new(None) });

        thread::spawn(move || {
            loop {
                if let Err(e) = queue.blocking_dispatch(&mut state) {
                    eprintln!("the clipboard watcher stopped: {e}");
                    return;
                }
            }
        });

        Ok(Self { connection, shared })
    }

    fn read(&self, offer: &Offer, mime: &str) -> Result<Vec<u8>, EdgeError> {
        let (mut reader, writer) = std::io::pipe().map_err(edge_err)?;
        offer.receive(mime.to_owned(), writer.as_fd());
        self.connection.flush().map_err(edge_err)?;
        drop(writer);

        let (sender, receiver) = mpsc::channel();
        thread::spawn(move || {
            let mut bytes = Vec::new();
            let _ = sender.send(reader.read_to_end(&mut bytes).map(|_| bytes));
        });
        receiver
            .recv_timeout(READ_TIMEOUT)
            .map_err(|_| edge_err("the clipboard owner did not send its data in time"))?
            .map_err(edge_err)
    }
}

impl ClipboardPasteboard for WaylandPasteboard {
    fn change_count(&self) -> Result<i64, EdgeError> {
        Ok(self.shared.count.load(Ordering::SeqCst))
    }

    fn types(&self) -> Result<Vec<String>, EdgeError> {
        let selection = self.shared.selection.lock().unwrap_or_else(PoisonError::into_inner);
        Ok(utis_of(&selection.mimes))
    }

    fn data(&self, uti_name: String) -> Result<Option<Vec<u8>>, EdgeError> {
        let (offer, mime) = {
            let selection = self.shared.selection.lock().unwrap_or_else(PoisonError::into_inner);
            let Some(offer) = selection.offer.clone() else { return Ok(None) };
            let Some(mime) = mimes_for(&uti_name).iter().find(|m| selection.mimes.iter().any(|o| o == *m)) else {
                return Ok(None);
            };
            (offer, *mime)
        };
        let bytes = self.read(&offer, mime)?;
        Ok((!bytes.is_empty()).then_some(bytes))
    }

    fn write(&self, representations: Vec<ClipboardPasteboardData>) -> Result<ClipboardPasteboardWrite, EdgeError> {
        let writer = self.shared.writer.get().ok_or_else(|| edge_err("not connected"))?;
        let data = mime_data(representations);
        let mimes: Vec<String> = data.keys().cloned().chain([self.shared.own_mime.clone()]).collect();

        let source = writer.manager.create_data_source(&writer.queue, Arc::new(data));
        for mime in mimes {
            source.offer(mime);
        }
        writer.device.set_selection(Some(&source));
        let previous = writer.source.lock().unwrap_or_else(PoisonError::into_inner).replace(source);
        if let Some(previous) = previous {
            previous.destroy();
        }
        self.connection.flush().map_err(edge_err)?;

        let change_count = self.shared.count.fetch_add(1, Ordering::SeqCst) + 1;
        Ok(ClipboardPasteboardWrite { change_count, failure: None })
    }
}

impl Dispatch<wl_registry::WlRegistry, ()> for WaylandState {
    fn event(
        state: &mut Self,
        registry: &wl_registry::WlRegistry,
        event: wl_registry::Event,
        _: &(),
        _: &Connection,
        handle: &QueueHandle<Self>,
    ) {
        if let wl_registry::Event::Global { name, interface, .. } = event {
            match interface.as_str() {
                "wl_seat" if state.seat.is_none() => state.seat = Some(registry.bind(name, 1, handle, ())),
                MANAGER_INTERFACE => state.manager = Some(registry.bind(name, 1, handle, ())),
                _ => {}
            }
        }
    }
}

impl Dispatch<wl_seat::WlSeat, ()> for WaylandState {
    fn event(_: &mut Self, _: &wl_seat::WlSeat, _: wl_seat::Event, _: &(), _: &Connection, _: &QueueHandle<Self>) {}
}

impl Dispatch<Manager, ()> for WaylandState {
    fn event(
        _: &mut Self,
        _: &Manager,
        _: <Manager as Proxy>::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
    }
}

impl Dispatch<Device, ()> for WaylandState {
    fn event(
        state: &mut Self,
        _: &Device,
        event: device_v1::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        match event {
            device_v1::Event::DataOffer { id } => {
                state.offered.insert(id.id(), Vec::new());
            }
            device_v1::Event::Selection { id } => {
                let mimes = id.as_ref().and_then(|offer| state.offered.remove(&offer.id())).unwrap_or_default();
                let own = mimes.contains(&state.shared.own_mime);
                let previous = {
                    let mut selection = state.shared.selection.lock().unwrap_or_else(PoisonError::into_inner);
                    std::mem::replace(&mut *selection, Selection { offer: id, mimes })
                };
                if let Some(offer) = previous.offer {
                    offer.destroy();
                }
                if !own {
                    state.shared.count.fetch_add(1, Ordering::SeqCst);
                }
            }
            device_v1::Event::PrimarySelection { id: Some(offer) } => {
                state.offered.remove(&offer.id());
                offer.destroy();
            }
            _ => {}
        }
    }

    event_created_child!(WaylandState, Device, [
        device_v1::EVT_DATA_OFFER_OPCODE => (Offer, ()),
    ]);
}

impl Dispatch<Offer, ()> for WaylandState {
    fn event(
        state: &mut Self,
        offer: &Offer,
        event: offer_v1::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        if let offer_v1::Event::Offer { mime_type } = event
            && let Some(mimes) = state.offered.get_mut(&offer.id())
        {
            mimes.push(mime_type);
        }
    }
}

impl Dispatch<Source, SourceData> for WaylandState {
    fn event(
        _: &mut Self,
        source: &Source,
        event: source_v1::Event,
        data: &SourceData,
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        match event {
            source_v1::Event::Send { mime_type, fd } => {
                if let Some(bytes) = data.get(&mime_type).cloned() {
                    thread::spawn(move || send_bytes(fd, &bytes));
                }
            }
            source_v1::Event::Cancelled => source.destroy(),
            _ => {}
        }
    }
}

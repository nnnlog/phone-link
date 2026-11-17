//! The clipboard through a Wayland data-control protocol, which reaches the selection without keyboard
//! focus: `ext-data-control-v1`, or wlroots' older `wlr-data-control-unstable-v1`, which has the same
//! requests and events under other names ([`DataControl`]). A thread of its own dispatches the
//! compositor's events: the offers, the current selection, and the requests for the data of a write.

mod events;
mod ext;
mod wlr;

use std::collections::HashMap;
use std::io::{ErrorKind, Read};
use std::os::fd::{AsFd, BorrowedFd, OwnedFd};
use std::sync::atomic::{AtomicI64, Ordering};
use std::sync::{Arc, Mutex, OnceLock, mpsc};
use std::thread;

use phonelink_clipboard::pasteboard::{ClipboardPasteboard, ClipboardPasteboardData, ClipboardPasteboardWrite};
use phonelink_core::sync::MutexExt;
use phonelink_engine::edge::EdgeError;
use wayland_client::protocol::wl_seat;
use wayland_client::{ConnectError, Connection, DispatchError, EventQueue, Proxy, QueueHandle};

use super::mimes::{URI_LIST, first_offered, mime_data, utf8_text, utis_with_files};
use super::{READ_TIMEOUT, edge_err};
use events::WaylandState;
pub(super) use ext::Ext;
pub(super) use wlr::Wlr;

#[cfg(test)]
mod tests;

static NEXT_SOURCE_ID: AtomicI64 = AtomicI64::new(0);

/// A MIME type of the app's own, unique to one write and offered with it: a selection that offers
/// it is the app's own write.
fn own_source_mime() -> String {
    format!(
        "application/x-phonelink-source;pid={};id={}",
        std::process::id(),
        NEXT_SOURCE_ID.fetch_add(1, Ordering::SeqCst)
    )
}

/// How much of an offer's pipe is read at a time.
const READ_PART: usize = 64 * 1024;

/// Why the clipboard was not reached through a data-control protocol.
#[derive(Debug, thiserror::Error)]
pub enum WaylandError {
    #[error("{0}")]
    Connect(#[from] ConnectError),
    #[error("{0}")]
    Roundtrip(#[from] DispatchError),
    /// The compositor announced no seat, or not the manager of this interface.
    #[error("the compositor offers no seat or {0}")]
    NoDataControl(&'static str),
}

/// One data-control protocol: its objects, the requests the clipboard makes of them, and their events
/// in the form both protocols share.
pub(super) trait DataControl: Sized + Send + Sync + 'static {
    /// The interface of the manager, as the registry announces it.
    const MANAGER_INTERFACE: &'static str;
    /// The opcode of the device's `data_offer` event, which creates an offer.
    const DATA_OFFER_OPCODE: u16;

    type Manager: Proxy + Send + Sync + 'static;
    type Device: Proxy + Send + Sync + 'static;
    type Offer: Proxy + Send + Sync + 'static;
    type Source: Proxy + Send + Sync + 'static;

    fn get_data_device(
        manager: &Self::Manager,
        seat: &wl_seat::WlSeat,
        queue: &QueueHandle<WaylandState<Self>>,
    ) -> Self::Device;
    fn create_data_source(
        manager: &Self::Manager,
        queue: &QueueHandle<WaylandState<Self>>,
        contents: SourceContents,
    ) -> Self::Source;
    /// Makes `source` the clipboard's selection.
    fn set_selection(device: &Self::Device, source: &Self::Source);
    /// Offers the data of `source` as `mime`.
    fn offer(source: &Self::Source, mime: String);
    fn destroy_source(source: &Self::Source);
    /// Asks the selection's owner to write the data of `offer` as `mime` to `fd`.
    fn receive(offer: &Self::Offer, mime: String, fd: BorrowedFd<'_>);
    fn destroy_offer(offer: &Self::Offer);
    fn device_event(event: <Self::Device as Proxy>::Event) -> DeviceEvent<Self::Offer>;
    /// The MIME type an offer's event lists, when it lists one.
    fn offered_mime(event: <Self::Offer as Proxy>::Event) -> Option<String>;
    fn source_event(event: <Self::Source as Proxy>::Event) -> SourceEvent;
}

/// A device's event, in the form both protocols share.
pub(super) enum DeviceEvent<Offer> {
    /// A new offer, whose MIME types follow as its own events.
    DataOffer(Offer),
    /// The clipboard's selection is now this offer, or none.
    Selection(Option<Offer>),
    /// The primary selection is now this offer, or none.
    PrimarySelection(Option<Offer>),
    Other,
}

/// A source's event, in the form both protocols share.
pub(super) enum SourceEvent {
    /// A client asks for the data as `mime_type`, to be written to `fd`.
    Send {
        mime_type: String,
        fd: OwnedFd,
    },
    /// Another selection replaced the source.
    Cancelled,
    Other,
}

/// The data a write offers, by MIME type: the user data of its source.
pub(super) type SourceContents = Arc<HashMap<String, Arc<Vec<u8>>>>;

// Each kind of object has user data of its own type, so that the dispatch of the managers, the devices
// and the offers of either protocol are told apart by type.

/// The user data of the manager.
pub(super) struct ManagerUserData;
/// The user data of the device.
pub(super) struct DeviceUserData;
/// The user data of an offer.
pub(super) struct OfferUserData;

/// The current selection: its offer, and the MIME types the offer listed.
struct Selection<P: DataControl> {
    offer: Option<P::Offer>,
    mimes: Vec<String>,
}

impl<P: DataControl> Default for Selection<P> {
    fn default() -> Self {
        Self { offer: None, mimes: Vec::new() }
    }
}

/// What a write needs: the manager that makes its source, the seat's device that sets it, and the
/// source of the last write.
struct Writer<P: DataControl> {
    manager: P::Manager,
    device: P::Device,
    queue: QueueHandle<WaylandState<P>>,
    source: Mutex<Option<P::Source>>,
}

/// What the event thread and the pasteboard share: the app's own MIME type, the change count, the
/// current selection, and the writer once the device is made.
struct ClipboardState<P: DataControl> {
    own_mime: String,
    count: AtomicI64,
    selection: Mutex<Selection<P>>,
    writer: OnceLock<Writer<P>>,
}

/// What the thread that reads an offer's pipe hands on.
enum Received {
    Part(Vec<u8>),
    End,
    Failed(std::io::Error),
}

/// The clipboard through the data-control protocol `P`.
pub(super) struct WaylandPasteboard<P: DataControl> {
    connection: Connection,
    clipboard: Arc<ClipboardState<P>>,
}

impl<P: DataControl> WaylandPasteboard<P> {
    pub(super) fn connect() -> Result<Self, WaylandError> {
        let connection = Connection::connect_to_env()?;
        let mut queue: EventQueue<WaylandState<P>> = connection.new_event_queue();
        let handle = queue.handle();
        connection.display().get_registry(&handle, ());

        let clipboard = Arc::new(ClipboardState {
            own_mime: own_source_mime(),
            count: AtomicI64::new(0),
            selection: Mutex::default(),
            writer: OnceLock::new(),
        });
        let mut state =
            WaylandState { clipboard: clipboard.clone(), seat: None, manager: None, offered: HashMap::new() };
        queue.roundtrip(&mut state)?;

        let (Some(seat), Some(manager)) = (state.seat.clone(), state.manager.clone()) else {
            return Err(WaylandError::NoDataControl(P::MANAGER_INTERFACE));
        };
        let device = P::get_data_device(&manager, &seat, &handle);
        let _ = clipboard.writer.set(Writer { manager, device, queue: handle, source: Mutex::new(None) });

        thread::spawn(move || {
            loop {
                if let Err(e) = queue.blocking_dispatch(&mut state) {
                    tracing::warn!(error = %e, "the clipboard watcher stopped");
                    return;
                }
            }
        });

        Ok(Self { connection, clipboard })
    }

    /// The offer's data as `mime`; `None` when the owner lets the read timeout pass before it sends the
    /// next part or closes the pipe.
    fn read(&self, offer: &P::Offer, mime: &str) -> Result<Option<Vec<u8>>, EdgeError> {
        let (mut reader, writer) = std::io::pipe().map_err(edge_err)?;
        P::receive(offer, mime.to_owned(), writer.as_fd());
        self.connection.flush().map_err(edge_err)?;
        drop(writer);

        let (sender, receiver) = mpsc::channel();
        thread::spawn(move || {
            let mut part = vec![0; READ_PART];
            loop {
                let received = match reader.read(&mut part) {
                    Ok(0) => Received::End,
                    Ok(len) => Received::Part(part[..len].to_vec()),
                    Err(e) if e.kind() == ErrorKind::Interrupted => continue,
                    Err(e) => Received::Failed(e),
                };
                let last = !matches!(received, Received::Part(_));
                if sender.send(received).is_err() || last {
                    return;
                }
            }
        });
        let mut data = Vec::new();
        loop {
            match receiver.recv_timeout(READ_TIMEOUT) {
                Ok(Received::Part(part)) => data.extend_from_slice(&part),
                Ok(Received::End) => return Ok(Some(data)),
                Ok(Received::Failed(e)) => return Err(edge_err(e)),
                Err(_) => {
                    tracing::info!("the clipboard owner did not send its data in time");
                    return Ok(None);
                }
            }
        }
    }
}

impl<P: DataControl> ClipboardPasteboard for WaylandPasteboard<P> {
    fn change_count(&self) -> Result<i64, EdgeError> {
        Ok(self.clipboard.count.load(Ordering::SeqCst))
    }

    fn types(&self) -> Result<Vec<String>, EdgeError> {
        let (offer, mimes) = {
            let selection = self.clipboard.selection.lock_or_recover();
            (selection.offer.clone(), selection.mimes.clone())
        };
        utis_with_files(&mimes, || match &offer {
            Some(offer) => self.read(offer, URI_LIST),
            None => Ok(None),
        })
    }

    fn data(&self, uti_name: String) -> Result<Option<Vec<u8>>, EdgeError> {
        let (offer, mime) = {
            let selection = self.clipboard.selection.lock_or_recover();
            let Some(offer) = selection.offer.clone() else { return Ok(None) };
            let Some(mime) = first_offered(&uti_name, &selection.mimes) else { return Ok(None) };
            (offer, mime.to_owned())
        };
        Ok(self.read(&offer, &mime)?.map(|bytes| utf8_text(&mime, bytes)))
    }

    fn write(&self, representations: Vec<ClipboardPasteboardData>) -> Result<ClipboardPasteboardWrite, EdgeError> {
        let writer = self.clipboard.writer.get().ok_or_else(|| edge_err("not connected"))?;
        let (data, failure) = mime_data(representations);
        let mimes: Vec<String> = data.keys().cloned().chain([self.clipboard.own_mime.clone()]).collect();

        let source = P::create_data_source(&writer.manager, &writer.queue, Arc::new(data));
        for mime in mimes {
            P::offer(&source, mime);
        }
        P::set_selection(&writer.device, &source);
        let previous = writer.source.lock_or_recover().replace(source);
        if let Some(previous) = previous {
            P::destroy_source(&previous);
        }
        self.connection.flush().map_err(edge_err)?;

        let change_count = self.clipboard.count.fetch_add(1, Ordering::SeqCst) + 1;
        Ok(ClipboardPasteboardWrite { change_count, failure })
    }
}

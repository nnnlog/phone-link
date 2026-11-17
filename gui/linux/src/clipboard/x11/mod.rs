//! The clipboard through the X11 `CLIPBOARD` selection (ICCCM), which `XWayland` also serves on a
//! Wayland compositor without a data-control protocol: a window of the app's own owns the selection
//! for a write and asks the owner to convert it for a read, and `XFixes` tells when another client
//! takes the selection.

mod owner;

use std::collections::HashMap;
use std::sync::atomic::{AtomicI64, AtomicUsize, Ordering};
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Instant;

use owner::Transfer;
use phonelink_clipboard::pasteboard::{ClipboardPasteboard, ClipboardPasteboardData, ClipboardPasteboardWrite};
use phonelink_core::sync::MutexExt;
use phonelink_engine::edge::EdgeError;
use x11rb::connection::Connection as _;
use x11rb::errors::{ConnectError, ConnectionError, ReplyError, ReplyOrIdError};
use x11rb::protocol::Event;
use x11rb::protocol::xfixes::{ConnectionExt as _, SelectionEventMask};
use x11rb::protocol::xproto::{
    Atom, AtomEnum, ConnectionExt as _, CreateWindowAux, EventMask, Property, Window, WindowClass,
};
use x11rb::rust_connection::RustConnection;
use x11rb::{COPY_DEPTH_FROM_PARENT, COPY_FROM_PARENT, CURRENT_TIME, NONE};

use super::mimes::{URI_LIST, first_offered, mime_data, utf8_text, utis_with_files};
use super::{READ_TIMEOUT, edge_err};

#[cfg(test)]
mod tests;

/// A failure of the X11 connection or of a request on it.
#[derive(Debug, thiserror::Error)]
pub enum X11Error {
    #[error("X11: {0}")]
    Connect(#[from] ConnectError),
    #[error("X11: {0}")]
    Connection(#[from] ConnectionError),
    #[error("X11: {0}")]
    Reply(#[from] ReplyError),
    #[error("X11: {0}")]
    NewId(#[from] ReplyOrIdError),
}

enum ReaderEvent {
    /// The owner answered a conversion: the target it converted, and the property it wrote (`NONE` when
    /// it refused).
    Converted { target: Atom, property: Atom },
    /// A transfer property of the app's window has a new value: a part of a long (`INCR`) answer.
    NewValue(Atom),
}

/// How many window properties reads take in turn.
const TRANSFER_PROPERTIES: usize = 4;

struct Atoms {
    clipboard: Atom,
    targets: Atom,
    incr: Atom,
    /// The properties of the app's window that owners write their answers to, one read after another,
    /// so that an answer to an earlier read that timed out is not taken for the one in flight.
    transfers: [Atom; TRANSFER_PROPERTIES],
}

/// The app's clipboard window on its X11 connection, and what its event thread and the reads share.
struct ClipboardWindow {
    connection: RustConnection,
    window: Window,
    atoms: Atoms,
    count: AtomicI64,
    next_transfer: AtomicUsize,
    owned: Mutex<HashMap<Atom, Arc<Vec<u8>>>>,
    reader: Mutex<Option<Sender<ReaderEvent>>>,
    atom_names: Mutex<HashMap<Atom, String>>,
    transfers: Mutex<HashMap<(Window, Atom), Transfer>>,
}

/// The largest answer written at once; a longer one goes in parts of this size.
const INCR_CHUNK: usize = 256 * 1024;

/// The clipboard as the X11 `CLIPBOARD` selection.
pub struct X11Pasteboard {
    clipboard: Arc<ClipboardWindow>,
    read_lock: Mutex<()>,
}

/// The atom named `name`, made when there is none.
fn intern(connection: &RustConnection, name: &str) -> Result<Atom, X11Error> {
    Ok(connection.intern_atom(false, name.as_bytes())?.reply()?.atom)
}

/// The first event `wanted` picks from `events` within the read timeout; `None` when none comes in time.
/// Other events (answers to earlier reads) are passed over.
fn wait_for<T>(events: &Receiver<ReaderEvent>, wanted: impl Fn(&ReaderEvent) -> Option<T>) -> Option<T> {
    let deadline = Instant::now() + READ_TIMEOUT;
    loop {
        let event = events.recv_timeout(deadline.saturating_duration_since(Instant::now())).ok()?;
        if let Some(value) = wanted(&event) {
            return Some(value);
        }
    }
}

impl X11Pasteboard {
    pub fn connect() -> Result<Self, X11Error> {
        let (connection, screen) = x11rb::connect(None)?;
        let root = connection.setup().roots[screen].root;
        let window = connection.generate_id()?;
        connection.create_window(
            COPY_DEPTH_FROM_PARENT,
            window,
            root,
            0,
            0,
            1,
            1,
            0,
            WindowClass::INPUT_OUTPUT,
            COPY_FROM_PARENT,
            &CreateWindowAux::new().event_mask(EventMask::PROPERTY_CHANGE),
        )?;
        let mut transfers = [NONE; TRANSFER_PROPERTIES];
        for (index, transfer) in transfers.iter_mut().enumerate() {
            *transfer = intern(&connection, &format!("PHONELINK_CLIPBOARD_{index}"))?;
        }

        let atoms = Atoms {
            clipboard: intern(&connection, "CLIPBOARD")?,
            targets: intern(&connection, "TARGETS")?,
            incr: intern(&connection, "INCR")?,
            transfers,
        };
        connection.xfixes_query_version(5, 0)?.reply()?;
        connection.xfixes_select_selection_input(
            window,
            atoms.clipboard,
            SelectionEventMask::SET_SELECTION_OWNER
                | SelectionEventMask::SELECTION_WINDOW_DESTROY
                | SelectionEventMask::SELECTION_CLIENT_CLOSE,
        )?;
        connection.flush()?;

        let clipboard = Arc::new(ClipboardWindow {
            connection,
            window,
            atoms,
            count: AtomicI64::new(0),
            next_transfer: AtomicUsize::new(0),
            owned: Mutex::default(),
            reader: Mutex::default(),
            atom_names: Mutex::default(),
            transfers: Mutex::default(),
        });
        let events = clipboard.clone();
        thread::spawn(move || {
            if let Err(e) = events.dispatch() {
                tracing::warn!(error = %e, "the X11 clipboard watcher stopped");
            }
        });
        Ok(Self { clipboard, read_lock: Mutex::new(()) })
    }

    /// The clipboard converted to `target`; `None` when the owner refuses it or does not answer in time.
    fn convert(&self, target: Atom) -> Result<Option<Vec<u8>>, EdgeError> {
        let _reading = self.read_lock.lock_or_recover();
        let turn = self.clipboard.next_transfer.fetch_add(1, Ordering::Relaxed) % TRANSFER_PROPERTIES;
        let property = self.clipboard.atoms.transfers[turn];
        let (sender, receiver) = mpsc::channel();
        *self.clipboard.reader.lock_or_recover() = Some(sender);
        let result = self.clipboard.read(target, property, &receiver);
        *self.clipboard.reader.lock_or_recover() = None;
        result
    }

    fn offered_mimes(&self) -> Result<Vec<String>, EdgeError> {
        let Some(bytes) = self.convert(self.clipboard.atoms.targets)? else { return Ok(Vec::new()) };
        let atoms: Vec<Atom> = bytes.as_chunks::<4>().0.iter().map(|chunk| u32::from_ne_bytes(*chunk)).collect();
        atoms.into_iter().map(|atom| self.clipboard.atom_name(atom)).collect()
    }

    fn read_mime(&self, mime: &str) -> Result<Option<Vec<u8>>, EdgeError> {
        let target = intern(&self.clipboard.connection, mime).map_err(edge_err)?;
        self.convert(target)
    }
}

impl ClipboardWindow {
    fn dispatch(&self) -> Result<(), X11Error> {
        loop {
            match self.connection.wait_for_event()? {
                Event::XfixesSelectionNotify(event) if event.selection == self.atoms.clipboard => {
                    if event.owner != self.window {
                        self.owned.lock_or_recover().clear();
                        self.count.fetch_add(1, Ordering::SeqCst);
                    }
                }
                Event::SelectionRequest(request) => self.serve(&request)?,
                Event::SelectionNotify(event)
                    if event.requestor == self.window && event.selection == self.atoms.clipboard =>
                {
                    self.notify_reader(ReaderEvent::Converted { target: event.target, property: event.property });
                }
                Event::PropertyNotify(event)
                    if event.window == self.window
                        && self.atoms.transfers.contains(&event.atom)
                        && event.state == Property::NEW_VALUE =>
                {
                    self.notify_reader(ReaderEvent::NewValue(event.atom));
                }
                Event::PropertyNotify(event) if event.state == Property::DELETE => {
                    self.continue_transfer(event.window, event.atom)?;
                }
                _ => {}
            }
        }
    }

    fn notify_reader(&self, event: ReaderEvent) {
        if let Some(reader) = self.reader.lock_or_recover().as_ref() {
            let _ = reader.send(event);
        }
    }

    fn take_property(&self, property: Atom) -> Result<(Atom, Vec<u8>), EdgeError> {
        let reply = self
            .connection
            .get_property(true, self.window, property, AtomEnum::ANY, 0, u32::MAX)
            .map_err(edge_err)?
            .reply()
            .map_err(edge_err)?;
        Ok((reply.type_, reply.value))
    }

    /// Asks the owner to convert the clipboard to `target` into `property`, and reads its answer: only
    /// one that names both, and a long answer part by part, each part within the read timeout.
    fn read(&self, target: Atom, property: Atom, events: &Receiver<ReaderEvent>) -> Result<Option<Vec<u8>>, EdgeError> {
        // A value left there by a late answer to an earlier read would pass for this read's.
        self.connection.delete_property(self.window, property).map_err(edge_err)?;
        self.connection
            .convert_selection(self.window, self.atoms.clipboard, target, property, CURRENT_TIME)
            .map_err(edge_err)?;
        self.connection.flush().map_err(edge_err)?;

        let answered = wait_for(events, |event| match *event {
            ReaderEvent::Converted { target: t, property: p } if t == target && (p == property || p == NONE) => Some(p),
            _ => None,
        });
        let Some(answered) = answered else {
            tracing::info!("the clipboard owner did not answer in time");
            return Ok(None);
        };
        if answered == NONE {
            return Ok(None);
        }

        let (kind, bytes) = self.take_property(property)?;
        if kind != self.atoms.incr {
            return Ok(Some(bytes));
        }
        self.connection.flush().map_err(edge_err)?;
        let mut data = Vec::new();
        loop {
            let part =
                wait_for(events, |event| matches!(*event, ReaderEvent::NewValue(p) if p == property).then_some(()));
            if part.is_none() {
                tracing::info!("the clipboard owner stopped sending a long answer");
                return Ok(None);
            }
            let (_, chunk) = self.take_property(property)?;
            self.connection.flush().map_err(edge_err)?;
            if chunk.is_empty() {
                return Ok(Some(data));
            }
            data.extend_from_slice(&chunk);
        }
    }

    fn atom_name(&self, atom: Atom) -> Result<String, EdgeError> {
        if let Some(name) = self.atom_names.lock_or_recover().get(&atom) {
            return Ok(name.clone());
        }
        let reply = self.connection.get_atom_name(atom).map_err(edge_err)?.reply().map_err(edge_err)?;
        let name = String::from_utf8_lossy(&reply.name).into_owned();
        self.atom_names.lock_or_recover().insert(atom, name.clone());
        Ok(name)
    }
}

impl ClipboardPasteboard for X11Pasteboard {
    fn change_count(&self) -> Result<i64, EdgeError> {
        Ok(self.clipboard.count.load(Ordering::SeqCst))
    }

    fn types(&self) -> Result<Vec<String>, EdgeError> {
        utis_with_files(&self.offered_mimes()?, || self.read_mime(URI_LIST))
    }

    fn data(&self, uti_name: String) -> Result<Option<Vec<u8>>, EdgeError> {
        let offered = self.offered_mimes()?;
        let Some(mime) = first_offered(&uti_name, &offered) else { return Ok(None) };
        Ok(self.read_mime(mime)?.map(|bytes| utf8_text(mime, bytes)))
    }

    fn write(&self, representations: Vec<ClipboardPasteboardData>) -> Result<ClipboardPasteboardWrite, EdgeError> {
        let (data, failure) = mime_data(representations);
        let mut owned = HashMap::new();
        for (mime, bytes) in data {
            owned.insert(intern(&self.clipboard.connection, &mime).map_err(edge_err)?, bytes);
        }

        *self.clipboard.owned.lock_or_recover() = owned;
        let connection = &self.clipboard.connection;
        connection
            .set_selection_owner(self.clipboard.window, self.clipboard.atoms.clipboard, CURRENT_TIME)
            .map_err(edge_err)?;
        connection.flush().map_err(edge_err)?;

        let change_count = self.clipboard.count.fetch_add(1, Ordering::SeqCst) + 1;
        Ok(ClipboardPasteboardWrite { change_count, failure })
    }
}

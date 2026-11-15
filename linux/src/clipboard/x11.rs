use std::collections::HashMap;
use std::sync::atomic::{AtomicI64, Ordering};
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, Mutex, PoisonError};
use std::thread;
use std::time::Instant;

use phonelink_clipboard::pasteboard::{ClipboardPasteboard, ClipboardPasteboardData, ClipboardPasteboardWrite};
use phonelink_engine::edge::EdgeError;
use x11rb::connection::Connection as _;
use x11rb::protocol::Event;
use x11rb::protocol::xfixes::{ConnectionExt as _, SelectionEventMask};
use x11rb::protocol::xproto::{
    Atom, AtomEnum, ChangeWindowAttributesAux, ConnectionExt as _, CreateWindowAux, EventMask, PropMode, Property,
    SELECTION_NOTIFY_EVENT, SelectionNotifyEvent, SelectionRequestEvent, Window, WindowClass,
};
use x11rb::rust_connection::RustConnection;
use x11rb::wrapper::ConnectionExt as _;
use x11rb::{COPY_DEPTH_FROM_PARENT, COPY_FROM_PARENT, CURRENT_TIME, NONE};

use super::{READ_TIMEOUT, edge_err, first_offered, mime_data, utis_of};

enum ReaderEvent {
    Converted(Atom),
    NewValue,
}

struct Atoms {
    clipboard: Atom,
    targets: Atom,
    incr: Atom,
    transfer: Atom,
}

struct Shared {
    connection: RustConnection,
    window: Window,
    atoms: Atoms,
    count: AtomicI64,
    owned: Mutex<HashMap<Atom, Arc<Vec<u8>>>>,
    reader: Mutex<Option<Sender<ReaderEvent>>>,
    atom_names: Mutex<HashMap<Atom, String>>,
    transfers: Mutex<HashMap<(Window, Atom), Transfer>>,
}

struct Transfer {
    target: Atom,
    bytes: Arc<Vec<u8>>,
    sent: usize,
}

const INCR_CHUNK: usize = 256 * 1024;

pub struct X11Pasteboard {
    shared: Arc<Shared>,
    read_lock: Mutex<()>,
}

fn x11_err(e: impl std::fmt::Display) -> String {
    format!("X11: {e}")
}

fn intern(connection: &RustConnection, name: &str) -> Result<Atom, String> {
    Ok(connection.intern_atom(false, name.as_bytes()).map_err(x11_err)?.reply().map_err(x11_err)?.atom)
}

impl X11Pasteboard {
    pub fn connect() -> Result<Self, String> {
        let (connection, screen) = x11rb::connect(None).map_err(x11_err)?;
        let root = connection.setup().roots[screen].root;
        let window = connection.generate_id().map_err(x11_err)?;
        connection
            .create_window(
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
            )
            .map_err(x11_err)?;
        let atoms = Atoms {
            clipboard: intern(&connection, "CLIPBOARD")?,
            targets: intern(&connection, "TARGETS")?,
            incr: intern(&connection, "INCR")?,
            transfer: intern(&connection, "PHONELINK_CLIPBOARD")?,
        };
        connection.xfixes_query_version(5, 0).map_err(x11_err)?.reply().map_err(x11_err)?;
        connection
            .xfixes_select_selection_input(
                window,
                atoms.clipboard,
                SelectionEventMask::SET_SELECTION_OWNER
                    | SelectionEventMask::SELECTION_WINDOW_DESTROY
                    | SelectionEventMask::SELECTION_CLIENT_CLOSE,
            )
            .map_err(x11_err)?;
        connection.flush().map_err(x11_err)?;

        let shared = Arc::new(Shared {
            connection,
            window,
            atoms,
            count: AtomicI64::new(0),
            owned: Mutex::default(),
            reader: Mutex::default(),
            atom_names: Mutex::default(),
            transfers: Mutex::default(),
        });
        let events = shared.clone();
        thread::spawn(move || {
            if let Err(e) = events.dispatch() {
                eprintln!("the X11 clipboard watcher stopped: {e}");
            }
        });
        Ok(Self { shared, read_lock: Mutex::new(()) })
    }

    fn convert(&self, target: Atom) -> Result<Option<Vec<u8>>, EdgeError> {
        let _reading = self.read_lock.lock().unwrap_or_else(PoisonError::into_inner);
        let (sender, receiver) = mpsc::channel();
        *self.shared.reader.lock().unwrap_or_else(PoisonError::into_inner) = Some(sender);
        let result = self.shared.read(target, &receiver);
        *self.shared.reader.lock().unwrap_or_else(PoisonError::into_inner) = None;
        result
    }

    fn offered_mimes(&self) -> Result<Vec<String>, EdgeError> {
        let Some(bytes) = self.convert(self.shared.atoms.targets)? else { return Ok(Vec::new()) };
        let atoms: Vec<Atom> = bytes.as_chunks::<4>().0.iter().map(|chunk| u32::from_ne_bytes(*chunk)).collect();
        atoms.into_iter().map(|atom| self.shared.atom_name(atom)).collect()
    }
}

impl Shared {
    fn dispatch(&self) -> Result<(), String> {
        loop {
            match self.connection.wait_for_event().map_err(x11_err)? {
                Event::XfixesSelectionNotify(event) if event.selection == self.atoms.clipboard => {
                    if event.owner != self.window {
                        self.owned.lock().unwrap_or_else(PoisonError::into_inner).clear();
                        self.count.fetch_add(1, Ordering::SeqCst);
                    }
                }
                Event::SelectionRequest(request) => self.serve(&request)?,
                Event::SelectionNotify(event) if event.requestor == self.window => {
                    self.notify_reader(ReaderEvent::Converted(event.property));
                }
                Event::PropertyNotify(event)
                    if event.window == self.window
                        && event.atom == self.atoms.transfer
                        && event.state == Property::NEW_VALUE =>
                {
                    self.notify_reader(ReaderEvent::NewValue);
                }
                Event::PropertyNotify(event) if event.state == Property::DELETE => {
                    self.continue_transfer(event.window, event.atom)?;
                }
                _ => {}
            }
        }
    }

    fn notify_reader(&self, event: ReaderEvent) {
        if let Some(reader) = self.reader.lock().unwrap_or_else(PoisonError::into_inner).as_ref() {
            let _ = reader.send(event);
        }
    }

    fn serve(&self, request: &SelectionRequestEvent) -> Result<(), String> {
        let property = if request.property == NONE { request.target } else { request.property };
        let owned = self.owned.lock().unwrap_or_else(PoisonError::into_inner).clone();
        let answered = if request.target == self.atoms.targets {
            let atoms: Vec<Atom> = owned.keys().copied().chain([self.atoms.targets]).collect();
            self.connection
                .change_property32(PropMode::REPLACE, request.requestor, property, AtomEnum::ATOM, &atoms)
                .map_err(x11_err)?;
            true
        } else if let Some(bytes) = owned.get(&request.target) {
            if bytes.len() > INCR_CHUNK {
                self.start_transfer(request.requestor, property, request.target, bytes.clone())?;
            } else {
                self.connection
                    .change_property8(PropMode::REPLACE, request.requestor, property, request.target, bytes)
                    .map_err(x11_err)?;
            }
            true
        } else {
            false
        };
        let notify = SelectionNotifyEvent {
            response_type: SELECTION_NOTIFY_EVENT,
            sequence: 0,
            time: request.time,
            requestor: request.requestor,
            selection: request.selection,
            target: request.target,
            property: if answered { property } else { NONE },
        };
        self.connection.send_event(false, request.requestor, EventMask::NO_EVENT, notify).map_err(x11_err)?;
        self.connection.flush().map_err(x11_err)
    }

    fn start_transfer(
        &self,
        requestor: Window,
        property: Atom,
        target: Atom,
        bytes: Arc<Vec<u8>>,
    ) -> Result<(), String> {
        let length = u32::try_from(bytes.len()).unwrap_or(u32::MAX);
        self.connection
            .change_window_attributes(
                requestor,
                &ChangeWindowAttributesAux::new().event_mask(EventMask::PROPERTY_CHANGE),
            )
            .map_err(x11_err)?;
        self.connection
            .change_property32(PropMode::REPLACE, requestor, property, self.atoms.incr, &[length])
            .map_err(x11_err)?;
        self.transfers
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .insert((requestor, property), Transfer { target, bytes, sent: 0 });
        Ok(())
    }

    fn continue_transfer(&self, requestor: Window, property: Atom) -> Result<(), String> {
        let mut transfers = self.transfers.lock().unwrap_or_else(PoisonError::into_inner);
        let Some(transfer) = transfers.get_mut(&(requestor, property)) else { return Ok(()) };
        let end = (transfer.sent + INCR_CHUNK).min(transfer.bytes.len());
        let chunk = &transfer.bytes[transfer.sent..end];
        self.connection
            .change_property8(PropMode::REPLACE, requestor, property, transfer.target, chunk)
            .map_err(x11_err)?;
        if chunk.is_empty() {
            transfers.remove(&(requestor, property));
            self.connection
                .change_window_attributes(requestor, &ChangeWindowAttributesAux::new().event_mask(EventMask::NO_EVENT))
                .map_err(x11_err)?;
        } else {
            transfer.sent = end;
        }
        self.connection.flush().map_err(x11_err)
    }

    fn take_property(&self) -> Result<(Atom, Vec<u8>), EdgeError> {
        let reply = self
            .connection
            .get_property(true, self.window, self.atoms.transfer, AtomEnum::ANY, 0, u32::MAX)
            .map_err(edge_err)?
            .reply()
            .map_err(edge_err)?;
        Ok((reply.type_, reply.value))
    }

    fn read(&self, target: Atom, events: &Receiver<ReaderEvent>) -> Result<Option<Vec<u8>>, EdgeError> {
        self.connection
            .convert_selection(self.window, self.atoms.clipboard, target, self.atoms.transfer, CURRENT_TIME)
            .map_err(edge_err)?;
        self.connection.flush().map_err(edge_err)?;

        let deadline = Instant::now() + READ_TIMEOUT;
        let wait = |events: &Receiver<ReaderEvent>| {
            events
                .recv_timeout(deadline.saturating_duration_since(Instant::now()))
                .map_err(|_| edge_err("the clipboard owner did not answer in time"))
        };
        loop {
            match wait(events)? {
                ReaderEvent::Converted(property) if property == NONE => return Ok(None),
                ReaderEvent::Converted(_) => break,
                ReaderEvent::NewValue => {}
            }
        }

        let (kind, bytes) = self.take_property()?;
        if kind != self.atoms.incr {
            return Ok(Some(bytes));
        }
        self.connection.flush().map_err(edge_err)?;
        let mut data = Vec::new();
        loop {
            if !matches!(wait(events)?, ReaderEvent::NewValue) {
                continue;
            }
            let (_, chunk) = self.take_property()?;
            self.connection.flush().map_err(edge_err)?;
            if chunk.is_empty() {
                return Ok(Some(data));
            }
            data.extend_from_slice(&chunk);
        }
    }

    fn atom_name(&self, atom: Atom) -> Result<String, EdgeError> {
        if let Some(name) = self.atom_names.lock().unwrap_or_else(PoisonError::into_inner).get(&atom) {
            return Ok(name.clone());
        }
        let reply = self.connection.get_atom_name(atom).map_err(edge_err)?.reply().map_err(edge_err)?;
        let name = String::from_utf8_lossy(&reply.name).into_owned();
        self.atom_names.lock().unwrap_or_else(PoisonError::into_inner).insert(atom, name.clone());
        Ok(name)
    }
}

impl ClipboardPasteboard for X11Pasteboard {
    fn change_count(&self) -> Result<i64, EdgeError> {
        Ok(self.shared.count.load(Ordering::SeqCst))
    }

    fn types(&self) -> Result<Vec<String>, EdgeError> {
        Ok(utis_of(&self.offered_mimes()?))
    }

    fn data(&self, uti_name: String) -> Result<Option<Vec<u8>>, EdgeError> {
        let offered = self.offered_mimes()?;
        let Some(mime) = first_offered(&uti_name, &offered) else { return Ok(None) };
        let target = intern(&self.shared.connection, mime).map_err(edge_err)?;
        Ok(self.convert(target)?.filter(|bytes| !bytes.is_empty()))
    }

    fn write(&self, representations: Vec<ClipboardPasteboardData>) -> Result<ClipboardPasteboardWrite, EdgeError> {
        let mut owned = HashMap::new();
        for (mime, bytes) in mime_data(representations) {
            owned.insert(intern(&self.shared.connection, &mime).map_err(edge_err)?, bytes);
        }
        *self.shared.owned.lock().unwrap_or_else(PoisonError::into_inner) = owned;
        let connection = &self.shared.connection;
        connection
            .set_selection_owner(self.shared.window, self.shared.atoms.clipboard, CURRENT_TIME)
            .map_err(edge_err)?;
        connection.flush().map_err(edge_err)?;

        let change_count = self.shared.count.fetch_add(1, Ordering::SeqCst) + 1;
        Ok(ClipboardPasteboardWrite { change_count, failure: None })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use phonelink_clipboard::pasteboard::uti;
    use std::time::Duration;

    #[test]
    #[ignore = "needs an X11 display and replaces its clipboard"]
    fn round_trips_through_the_live_x11_clipboard() {
        let ours = X11Pasteboard::connect().unwrap();
        let other = X11Pasteboard::connect().unwrap();
        let original = other.data(uti::PLAIN_TEXT.to_owned()).unwrap();
        let count = other.change_count().unwrap();

        let written = ours
            .write(vec![ClipboardPasteboardData { uti: uti::PLAIN_TEXT.to_owned(), data: b"phonelink".to_vec() }])
            .unwrap();
        thread::sleep(Duration::from_millis(300));
        assert_eq!(ours.change_count().unwrap(), written.change_count);
        assert!(other.change_count().unwrap() > count);
        assert!(other.types().unwrap().contains(&uti::PLAIN_TEXT.to_owned()));
        assert_eq!(other.data(uti::PLAIN_TEXT.to_owned()).unwrap().as_deref(), Some(&b"phonelink"[..]));

        if let Some(original) = original {
            ours.write(vec![ClipboardPasteboardData { uti: uti::PLAIN_TEXT.to_owned(), data: original }]).unwrap();
            thread::sleep(Duration::from_millis(300));
        }
    }
}

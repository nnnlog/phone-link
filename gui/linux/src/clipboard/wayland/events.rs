//! The event thread's state and its handling of the compositor's events: the registry's globals, the
//! offers and their MIME types, the selection, and the requests for the data of the app's write.

use std::collections::HashMap;
use std::fs::File;
use std::io::Write;
use std::os::fd::OwnedFd;
use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::thread;

use phonelink_core::sync::MutexExt;
use wayland_client::backend::{ObjectData, ObjectId};
use wayland_client::protocol::{wl_registry, wl_seat};
use wayland_client::{Connection, Dispatch, Proxy, QueueHandle};

use super::{
    ClipboardState, DataControl, DeviceEvent, DeviceUserData, ManagerUserData, OfferUserData, Selection,
    SourceContents, SourceEvent,
};

/// The event thread's state: the shared clipboard, the globals the registry announced, and the MIME
/// types of the offers whose selection has not come yet.
pub(in crate::clipboard) struct WaylandState<P: DataControl> {
    pub(super) clipboard: Arc<ClipboardState<P>>,
    pub(super) seat: Option<wl_seat::WlSeat>,
    pub(super) manager: Option<P::Manager>,
    pub(super) offered: HashMap<ObjectId, Vec<String>>,
}

impl<P: DataControl> Dispatch<wl_registry::WlRegistry, ()> for WaylandState<P> {
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
                interface if interface == P::MANAGER_INTERFACE => {
                    state.manager = Some(registry.bind(name, 1, handle, ManagerUserData));
                }
                _ => {}
            }
        }
    }
}

impl<P: DataControl> Dispatch<wl_seat::WlSeat, ()> for WaylandState<P> {
    fn event(_: &mut Self, _: &wl_seat::WlSeat, _: wl_seat::Event, _: &(), _: &Connection, _: &QueueHandle<Self>) {}
}

impl<P: DataControl> Dispatch<P::Manager, ManagerUserData> for WaylandState<P> {
    fn event(
        _: &mut Self,
        _: &P::Manager,
        _: <P::Manager as Proxy>::Event,
        _: &ManagerUserData,
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
    }
}

impl<P: DataControl> Dispatch<P::Device, DeviceUserData> for WaylandState<P> {
    fn event(
        state: &mut Self,
        _: &P::Device,
        event: <P::Device as Proxy>::Event,
        _: &DeviceUserData,
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        match P::device_event(event) {
            DeviceEvent::DataOffer(offer) => {
                state.offered.insert(offer.id(), Vec::new());
            }
            DeviceEvent::Selection(id) => {
                let mimes = id.as_ref().and_then(|offer| state.offered.remove(&offer.id())).unwrap_or_default();
                let own = mimes.contains(&state.clipboard.own_mime);
                let previous = {
                    let mut selection = state.clipboard.selection.lock_or_recover();
                    std::mem::replace(&mut *selection, Selection { offer: id, mimes })
                };
                if let Some(offer) = previous.offer {
                    P::destroy_offer(&offer);
                }
                if !own {
                    state.clipboard.count.fetch_add(1, Ordering::SeqCst);
                }
            }
            DeviceEvent::PrimarySelection(Some(offer)) => {
                state.offered.remove(&offer.id());
                P::destroy_offer(&offer);
            }
            DeviceEvent::PrimarySelection(None) | DeviceEvent::Other => {}
        }
    }

    fn event_created_child(opcode: u16, queue: &QueueHandle<Self>) -> Arc<dyn ObjectData> {
        if opcode == P::DATA_OFFER_OPCODE {
            return queue.make_data::<P::Offer, _>(OfferUserData);
        }
        panic!(
            "Missing event_created_child specialization for event opcode {} of {}",
            opcode,
            <P::Device as Proxy>::interface().name
        );
    }
}

impl<P: DataControl> Dispatch<P::Offer, OfferUserData> for WaylandState<P> {
    fn event(
        state: &mut Self,
        offer: &P::Offer,
        event: <P::Offer as Proxy>::Event,
        _: &OfferUserData,
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        if let Some(mime_type) = P::offered_mime(event)
            && let Some(mimes) = state.offered.get_mut(&offer.id())
        {
            mimes.push(mime_type);
        }
    }
}

impl<P: DataControl> Dispatch<P::Source, SourceContents> for WaylandState<P> {
    fn event(
        _: &mut Self,
        source: &P::Source,
        event: <P::Source as Proxy>::Event,
        contents: &SourceContents,
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        match P::source_event(event) {
            SourceEvent::Send { mime_type, fd } => {
                if let Some(bytes) = contents.get(&mime_type).cloned() {
                    thread::spawn(move || send_bytes(fd, &bytes));
                }
            }
            SourceEvent::Cancelled => P::destroy_source(source),
            SourceEvent::Other => {}
        }
    }
}

/// Writes `bytes` to `fd`, a client's pipe, and closes it.
fn send_bytes(fd: OwnedFd, bytes: &[u8]) {
    if let Err(e) = File::from(fd).write_all(bytes) {
        tracing::warn!(error = %e, "clipboard data was not sent");
    }
}

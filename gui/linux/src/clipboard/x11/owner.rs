//! The app's own selection on the X11 clipboard: the answers to other clients' requests to convert
//! it, a long answer in parts (ICCCM's `INCR`).

use std::sync::Arc;

use phonelink_core::sync::MutexExt;
use x11rb::NONE;
use x11rb::connection::Connection as _;
use x11rb::protocol::xproto::{
    Atom, AtomEnum, ChangeWindowAttributesAux, ConnectionExt as _, EventMask, PropMode, SELECTION_NOTIFY_EVENT,
    SelectionNotifyEvent, SelectionRequestEvent, Window,
};
use x11rb::wrapper::ConnectionExt as _;

use super::{ClipboardWindow, INCR_CHUNK, X11Error};

/// A long answer on its way to a requestor, part by part.
pub(super) struct Transfer {
    target: Atom,
    bytes: Arc<Vec<u8>>,
    sent: usize,
}

impl ClipboardWindow {
    /// Answers another client's request to convert the app's selection, writing the answer to the
    /// requestor's property: a long one in parts.
    pub(super) fn serve(&self, request: &SelectionRequestEvent) -> Result<(), X11Error> {
        let property = if request.property == NONE { request.target } else { request.property };
        let owned = self.owned.lock_or_recover().clone();
        let answered = if request.target == self.atoms.targets {
            let atoms: Vec<Atom> = owned.keys().copied().chain([self.atoms.targets]).collect();
            self.connection.change_property32(
                PropMode::REPLACE,
                request.requestor,
                property,
                AtomEnum::ATOM,
                &atoms,
            )?;
            true
        } else if let Some(bytes) = owned.get(&request.target) {
            if bytes.len() > INCR_CHUNK {
                self.start_transfer(request.requestor, property, request.target, bytes.clone())?;
            } else {
                self.connection.change_property8(
                    PropMode::REPLACE,
                    request.requestor,
                    property,
                    request.target,
                    bytes,
                )?;
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
        self.connection.send_event(false, request.requestor, EventMask::NO_EVENT, notify)?;
        Ok(self.connection.flush()?)
    }

    fn start_transfer(
        &self,
        requestor: Window,
        property: Atom,
        target: Atom,
        bytes: Arc<Vec<u8>>,
    ) -> Result<(), X11Error> {
        let length = u32::try_from(bytes.len()).unwrap_or(u32::MAX);
        self.connection.change_window_attributes(
            requestor,
            &ChangeWindowAttributesAux::new().event_mask(EventMask::PROPERTY_CHANGE),
        )?;
        self.connection.change_property32(PropMode::REPLACE, requestor, property, self.atoms.incr, &[length])?;
        self.transfers.lock_or_recover().insert((requestor, property), Transfer { target, bytes, sent: 0 });
        Ok(())
    }

    /// Writes the next part of a long answer once the requestor deleted the last one, and ends the
    /// transfer with an empty part.
    pub(super) fn continue_transfer(&self, requestor: Window, property: Atom) -> Result<(), X11Error> {
        let mut transfers = self.transfers.lock_or_recover();
        let Some(transfer) = transfers.get_mut(&(requestor, property)) else { return Ok(()) };
        let end = (transfer.sent + INCR_CHUNK).min(transfer.bytes.len());
        let chunk = &transfer.bytes[transfer.sent..end];
        self.connection.change_property8(PropMode::REPLACE, requestor, property, transfer.target, chunk)?;

        if chunk.is_empty() {
            transfers.remove(&(requestor, property));
            self.connection.change_window_attributes(
                requestor,
                &ChangeWindowAttributesAux::new().event_mask(EventMask::NO_EVENT),
            )?;
        } else {
            transfer.sent = end;
        }

        Ok(self.connection.flush()?)
    }
}

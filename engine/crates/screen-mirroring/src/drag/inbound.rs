//! The drag workflow's inbound messages.

use super::{DROP_TARGET_NOT_SUPPORTED, ItemStatus, Next, Phase, PhoneDragEvent, WindowDrag, cancel_blobs, reason};
use crate::wire::scope;
use crate::{ScreenMirroringEvent, wire};
use phonelink_core::sync::MutexExt;
use phonelink_core::types::new_guid_lower;
use std::sync::Arc;
use tracing::{debug, info, warn};

impl WindowDrag {
    /// One `/drag/*` message from the phone.
    pub fn on_message(self: &Arc<Self>, target: &str, body: &[u8]) {
        match target {
            scope::DRAG_REQUEST_ITEM => self.on_request_item(body),
            scope::DRAG_START_PHONE => self.on_start_phone(body),
            scope::DRAG_CLIP_DATA => self.on_clip_data(body),
            scope::DRAG_COMPLETE => self.on_complete(body),
            scope::DRAG_CANCEL => self.on_cancel(body),
            other => warn!(raw_target = other, "drag message without a handler; dropped"),
        }
    }

    /// `/drag/requestItem`: the phone pulls one file of the Mac drag.
    fn on_request_item(self: &Arc<Self>, body: &[u8]) {
        match wire::parse_drag_request_item(body) {
            Ok(id) => self.request_item(id),
            Err(e) => warn!(error = %e, "malformed requestItem; dropped"),
        }
    }

    /// `/drag/startPhone`: the phone starts a drag, taken only when none is under way.
    fn on_start_phone(&self, body: &[u8]) {
        if let Err(e) = wire::parse_drag_start_phone(body) {
            return warn!(error = %e, "malformed startPhone; dropped");
        }

        let mut st = self.state.lock_or_panic();
        st.activity = true;
        if st.phase == Phase::Ready {
            self.step(&mut st, PhoneDragEvent::StartPhone);
        } else {
            debug!(drag_phase = ?st.phase, "startPhone during a drag; ignored");
        }
    }

    /// `/drag/clipData`: the dragged items, kept when the drag starts with them.
    fn on_clip_data(&self, body: &[u8]) {
        let clip = match wire::parse_clip_data(body) {
            Ok(clip) => clip,
            Err(e) => return warn!(error = %e, "malformed clipData; dropped"),
        };

        let mut st = self.state.lock_or_panic();
        st.activity = false;
        if let Next::To(Phase::DragStarted) = self.step(&mut st, PhoneDragEvent::ClipData) {
            st.pending = Some(clip);
        }
    }

    /// `/drag/complete`, with the phone's drop target.
    fn on_complete(&self, body: &[u8]) {
        let target = match wire::parse_drag_complete(body) {
            Ok(target) => target,
            Err(e) => return warn!(error = %e, "malformed complete; dropped"),
        };

        let mut st = self.state.lock_or_panic();
        st.activity = false;
        let not_supported = target.as_deref() == Some(DROP_TARGET_NOT_SUPPORTED);

        match st.phase {
            Phase::Ready => {
                if not_supported {
                    info!(session = %self.session_id, "mac drag not accepted by the phone app");
                    let _ =
                        self.events.send(ScreenMirroringEvent::DragNotAccepted { session_id: self.session_id.clone() });
                }
            }
            Phase::Drop if not_supported => {}
            _ => {
                self.step(&mut st, PhoneDragEvent::Complete);
            }
        }
    }

    /// `/drag/cancel`, with the phone's reason.
    fn on_cancel(&self, body: &[u8]) {
        match wire::parse_drag_cancel(body) {
            Ok(r) => self.phone_cancel(r),
            Err(e) => warn!(error = %e, "malformed cancel; dropped"),
        }
    }

    pub(super) fn phone_cancel(&self, r: i32) {
        let mut st = self.state.lock_or_panic();
        st.activity = false;

        if r == reason::DOWNLOAD_ACTIVE {
            if let Some(t) = st.to_phone.as_mut() {
                t.effect_none = true;
            }
        } else {
            self.clear_outgoing(&mut st);
        }

        if st.phase == Phase::Ready {
            return;
        }

        if r == reason::USER_CANCELLED {
            let body = wire::drag_cancel(&new_guid_lower(), reason::USER_CANCELLED.into());
            self.send(&st, scope::DRAG_CANCEL, &body);
            if let (Some(d), Some((_, blob))) = (&st.from_phone, &st.channels) {
                let receiving = d.items.iter().filter(|i| *i.status.borrow() == ItemStatus::Receiving);
                cancel_blobs(blob, receiving.map(|i| i.item.data_id.as_bytes()));
            }
            self.step(&mut st, PhoneDragEvent::UserCancelled);
        } else {
            self.step(&mut st, PhoneDragEvent::PhoneCancel);
        }
    }
}

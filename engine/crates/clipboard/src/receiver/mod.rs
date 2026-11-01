//! The PC's `/clipboard` resource, serving inbound `GET /clipboard`.
//!
//! It covers the answer rows, the content answer and image re-encoding, and correlation ids (rows
//! 5–10 echo the request's id or a fresh GUID).
//!
//! The answer rows, which comments in this feature cite by number, are checked in order and the
//! first match answers:
//! 1. The request is not a `GET`: `INVALID_DEVICE_RESOURCE_REQUEST_TYPE`.
//! 2. The path is not `/clipboard…`: `INVALID_DEVICE_RESOURCE_REQUEST_PATH`.
//! 3. The request has no payload: `UNRECOGNIZED_PAYLOAD`.
//! 4. The payload does not parse as a `ClipboardRequestMessage` (a strict proto3 decode):
//!    `UNRECOGNIZED_PAYLOAD`. Phone Link on Windows does not answer it.
//! 5. The request type is neither `CONTENT` nor `STATUS`: `INVALID_CLIPBOARD_REQUEST_TYPE`.
//! 6. The sender is not a phone of the account: `INVALID_CONTENT`.
//! 7. The global switch is off or the phone lacks `CLIPBOARD`: `FEATURE_DISABLE`.
//! 8. The phone is not enabled or its toggle is off: `FEATURE_OFF`.
//! 9. A `STATUS` request: `FEATURE_ON`.
//! 10. A `CONTENT` request: the content answer, from the pasteboard as it is now.
//!
//! Every request gets at most one answer, a `ClipboardResponseMessage` that the DRM layer wraps in
//! `DeviceResourceManagerResponseMessage { payload, response_type = SUCCESS }` on
//! `/internal/response`. No answer when the pasteboard holds nothing recognized, the request has no
//! `_requestId`, or the feature has stopped. The checks never consult the PC's lock state or the
//! phone's app version.

use std::sync::{Arc, Weak};

use async_trait::async_trait;
use prost::Message;
use prost_types::Timestamp;
use tracing::{debug, info, warn};

use super::image::{ReencodeError, reencode_png};
use super::pasteboard::{ClipKind, ReadError, classify, read_html, read_image, read_plain_text};
use super::wire::{self, RESOURCE_PATH};
use super::{Clipboard, blocking, has_capability, listed_capabilities};
use phonelink_core::types::{DcgClient, new_guid_lower};
use phonelink_features::FeatureError;
use phonelink_features::RegistrationError;
use phonelink_proto::clipboard::v1::{
    ClipboardItem, ClipboardItemType, ClipboardRequestMessage, ClipboardRequestType, ClipboardResponseStatus,
};
use phonelink_proto::platform_sdk::v1::ProtoDeviceResourceRequestType;
use phonelink_transport::platform::{DrmReceiver, DrmReceiverError, DrmRequest};

/// Registers the receiver of `/clipboard` (matched without its `?query`).
pub(super) fn register(clipboard: &Arc<Clipboard>) -> Result<(), FeatureError> {
    let receiver = Arc::new(ClipboardReceiver(Arc::downgrade(clipboard)));
    clipboard
        .drm
        .register_receiver(RESOURCE_PATH, receiver)
        .map_err(|source| FeatureError::from(RegistrationError::ReceiverAt { path: RESOURCE_PATH, source }))
}

/// Holds the feature weakly: the DRM layer keeps it.
struct ClipboardReceiver(Weak<Clipboard>);

#[async_trait]
impl DrmReceiver for ClipboardReceiver {
    async fn receive(&self, request: DrmRequest) -> Result<Option<Vec<u8>>, DrmReceiverError> {
        match self.0.upgrade() {
            Some(clipboard) => clipboard.answer(request).await,
            None => Ok(None),
        }
    }
}

fn status(status: ClipboardResponseStatus, correlation_id: Option<&str>) -> Result<Option<Vec<u8>>, DrmReceiverError> {
    Ok(Some(wire::status_answer(status, correlation_id)))
}

/// Rows 1–5, which need nothing but the request: `Ok` with the request type (`CONTENT` or `STATUS`)
/// and the answer's correlation id (the request's, or a fresh GUID), or `Err` with the status answer.
pub(super) fn check_request(request: &DrmRequest) -> Result<(ClipboardRequestType, String), Vec<u8>> {
    use ClipboardResponseStatus as S;

    // Row 1.
    if request.request_type != ProtoDeviceResourceRequestType::Get {
        warn!(partner = %request.source, request_type = ?request.request_type, "/clipboard request is not a GET; answered INVALID_DEVICE_RESOURCE_REQUEST_TYPE");
        return Err(wire::status_answer(S::InvalidDeviceResourceRequestType, None));
    }

    // Row 2 (unreachable: only `/clipboard…` paths reach this receiver).
    if !request.path.starts_with(RESOURCE_PATH) {
        return Err(wire::status_answer(S::InvalidDeviceResourceRequestPath, None));
    }

    // Row 3.
    if request.payload.is_empty() {
        warn!(partner = %request.source, "/clipboard request without payload; answered UNRECOGNIZED_PAYLOAD");
        return Err(wire::status_answer(S::UnrecognizedPayload, None));
    }

    // Row 4: a payload that does not parse is answered.
    let message = ClipboardRequestMessage::decode(request.payload.as_slice()).map_err(|e| {
        warn!(partner = %request.source, error = %e, "/clipboard payload is not a ClipboardRequestMessage; answered UNRECOGNIZED_PAYLOAD");
        wire::status_answer(S::UnrecognizedPayload, None)
    })?;
    let correlation_id = if message.correlation_id.is_empty() { new_guid_lower() } else { message.correlation_id };

    // Row 5.
    match ClipboardRequestType::try_from(message.request_type) {
        Ok(t @ (ClipboardRequestType::Content | ClipboardRequestType::Status)) => Ok((t, correlation_id)),
        _ => {
            warn!(partner = %request.source, request_type = message.request_type, "/clipboard request is neither CONTENT nor STATUS; answered INVALID_CLIPBOARD_REQUEST_TYPE");
            Err(wire::status_answer(S::InvalidClipboardRequestType, Some(&correlation_id)))
        }
    }
}

impl Clipboard {
    /// The answer to one request, by the first matching row — none once the feature is stopped, also
    /// when it stops while the answer is being built.
    pub(super) async fn answer(self: &Arc<Self>, request: DrmRequest) -> Result<Option<Vec<u8>>, DrmReceiverError> {
        let source = request.source.clone();
        let answer = self.answer_by_row(request).await?;
        if answer.is_some() && self.is_stopped() {
            debug!(partner = %source, "feature stopped; /clipboard request not answered");
            return Ok(None);
        }
        Ok(answer)
    }

    async fn answer_by_row(self: &Arc<Self>, request: DrmRequest) -> Result<Option<Vec<u8>>, DrmReceiverError> {
        let source = request.source.clone();
        if !self.can_answer(&request) {
            return Ok(None);
        }

        let (request_type, correlation_id) = match check_request(&request) {
            Ok(checked) => checked,
            Err(answer) => return Ok(Some(answer)),
        };

        if let Some(answer) = self.state_row(&source, request_type, Some(correlation_id.as_str()))? {
            return Ok(Some(answer));
        }

        // Row 10: the pasteboard as it is now; `None` (logged) when it gets no answer.
        let me = self.clone();
        Ok(blocking("pasteboard read", move || me.content_answer(&correlation_id)).await.flatten())
    }

    /// The two conditions under which a request gets no answer at all (the feature stopped, no
    /// `_requestId`); `false` (logged) when one holds.
    fn can_answer(&self, request: &DrmRequest) -> bool {
        let source = &request.source;
        if self.is_stopped() {
            debug!(partner = %source, "feature stopped; /clipboard request not answered");
            return false;
        }
        if request.request_id.is_none() {
            warn!(partner = %source, raw_path = %request.path, "/clipboard request without _requestId cannot be answered; dropped");
            return false;
        }
        true
    }

    /// Rows 6–9, which read the account and the settings: the status answer of the first matching
    /// row, `None` when the request reaches row 10.
    fn state_row(
        &self,
        source: &DcgClient,
        request_type: ClipboardRequestType,
        cid: Option<&str>,
    ) -> Result<Option<Vec<u8>>, DrmReceiverError> {
        // Row 6.
        let Some(peer) = self.peers.peer(source) else {
            warn!(partner = %source, "/clipboard request from a phone unknown to the account; answered INVALID_CONTENT");
            return status(ClipboardResponseStatus::InvalidContent, cid);
        };

        let store_error =
            |e: phonelink_features::StoreError| DrmReceiverError(format!("clipboard settings unreadable: {e}"));

        // Row 7.
        if !self.store.global_switch().map_err(store_error)? || !has_capability(listed_capabilities(&peer.info)) {
            return status(ClipboardResponseStatus::FeatureDisable, cid);
        }

        // Row 8.
        if !self.switches.is_enabled(&peer.client).map_err(store_error)?
            || !self.store.toggle(&peer.client).map_err(store_error)?
        {
            return status(ClipboardResponseStatus::FeatureOff, cid);
        }

        // Row 9.
        if request_type == ClipboardRequestType::Status {
            return status(ClipboardResponseStatus::FeatureOn, cid);
        }

        Ok(None)
    }

    /// The content answer: the item of the highest recognized kind, `INVALID_CONTENT` when it cannot be
    /// read, `None` (no answer, logged) when nothing is recognized or the types do not read. Blocking.
    fn content_answer(&self, correlation_id: &str) -> Option<Vec<u8>> {
        // 1. The answer's build time.
        let created_time = wire::timestamp(self.clock.now());
        let limit = self.config.text_read_limit;

        let types = match self.pasteboard.types() {
            Ok(types) => types,
            Err(e) => {
                warn!(error = %e, "pasteboard types unreadable; the content request gets no answer");
                return None;
            }
        };

        // 2. Classify as for announcing.
        let Some(kind) = classify(&types) else {
            debug!(raw_types = ?types, "nothing recognized on the pasteboard; the content request gets no answer");
            return None;
        };

        let item = match kind {
            ClipKind::Text => self.plain_text_item(limit, created_time),
            ClipKind::Html => self.html_item(limit, created_time),
            ClipKind::Image => self.image_item(created_time),
        };

        Some(match item {
            Some(item) => wire::content_answer(item, correlation_id),
            None => wire::status_answer(ClipboardResponseStatus::InvalidContent, Some(correlation_id)),
        })
    }

    /// The `TEXT_PLAIN` item of the pasteboard, `None` (logged) when it cannot be read. Blocking.
    fn plain_text_item(&self, limit: usize, created_time: Timestamp) -> Option<ClipboardItem> {
        match read_plain_text(&*self.pasteboard, limit) {
            Ok(text) => Some(wire::text_item(ClipboardItemType::TextPlain, text, created_time)),
            Err(e @ ReadError::TooLong { .. }) => {
                info!(error = %e, "copied text over the read limit; answered INVALID_CONTENT");
                None
            }
            Err(e) => {
                warn!(error = %e, "copied text not read; answered INVALID_CONTENT");
                None
            }
        }
    }

    /// The `TEXT_HTML` item of the pasteboard, or its `TEXT_PLAIN` item when the HTML is empty or
    /// unreadable; `None` (logged) when neither can be read. Blocking.
    fn html_item(&self, limit: usize, created_time: Timestamp) -> Option<ClipboardItem> {
        match read_html(&*self.pasteboard, limit) {
            Ok(markup) if !markup.is_empty() => {
                Some(wire::text_item(ClipboardItemType::TextHtml, markup, created_time))
            }
            html => {
                match html {
                    Ok(_) => debug!("HTML empty; the plain text is sent"),
                    Err(e) => debug!(error = %e, "HTML not read; the plain text is sent"),
                }
                self.html_fallback_item(limit, created_time)
            }
        }
    }

    /// The `TEXT_PLAIN` item sent in place of HTML, `None` (logged) when it cannot be read. Blocking.
    fn html_fallback_item(&self, limit: usize, created_time: Timestamp) -> Option<ClipboardItem> {
        match read_plain_text(&*self.pasteboard, limit) {
            Ok(text) => Some(wire::text_item(ClipboardItemType::TextPlain, text, created_time)),
            Err(e @ ReadError::TooLong { .. }) => {
                info!(error = %e, "copied HTML has no plain text within the read limit; answered INVALID_CONTENT");
                None
            }
            Err(e) => {
                warn!(error = %e, "copied HTML has no readable plain text; answered INVALID_CONTENT");
                None
            }
        }
    }

    /// The `IMAGE` item of the pasteboard, re-encoded to fit the limit; `None` (logged) when it
    /// cannot be read or re-encoded. Blocking.
    fn image_item(&self, created_time: Timestamp) -> Option<ClipboardItem> {
        let image = match read_image(&*self.pasteboard) {
            Ok(image) => image,
            Err(e) => {
                warn!(error = %e, "copied image not read; answered INVALID_CONTENT");
                return None;
            }
        };

        match reencode_png(&*self.codec, &image, self.config.image_byte_limit, self.config.image_max_passes) {
            Ok(png) => Some(wire::image_item(png, created_time)),
            Err(e @ ReencodeError::TooLarge { .. }) => {
                info!(error = %e, "copied image over the size limit after downscaling; answered INVALID_CONTENT");
                None
            }
            Err(e) => {
                warn!(error = %e, "copied image not re-encoded; answered INVALID_CONTENT");
                None
            }
        }
    }
}

#[cfg(test)]
mod tests;

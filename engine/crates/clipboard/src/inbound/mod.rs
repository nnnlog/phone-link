//! Phone → PC: the change announcement (inbound tag 9), the content pull (outbound `GET /clipboard`
//! CONTENT) and the handling of its response.
//!
//! The MSAEP layer has already parsed the envelope, answered `{}`, found the context source, matched
//! tag 9 and dropped a repeated `message_id`. Each accepted announcement starts its own pull; pulls
//! are neither serialized nor ordered, so the last write to complete wins.
//!
//! The processing steps of an announcement, which comments here cite by number:
//! 1. The phone is the partner of the connection the message arrived on, and of the account.
//! 2. Its gates are open: the global switch, the phone's enabled switch, `CLIPBOARD`, its toggle.
//! 3. The message parses from the payload's `additional`.
//! 4. Its status is `CLIPBOARD_CHANGE`.
//! 5. The content pull, with the announcement's correlation id.

use std::sync::{Arc, Weak};
use std::time::Duration;

use prost::Message;
use tokio_util::sync::CancellationToken;
use tracing::{debug, info, warn};

use super::image::pasteboard_image;
use super::pasteboard::{ClipKind, ClipboardPasteboardData, representation, uti};
use super::wire::{self, RESOURCE_PATH};
use super::{Clipboard, ClipboardEvent, blocking, html};
use phonelink_core::edge::EdgeError;
use phonelink_core::sync::MutexExt;
use phonelink_core::types::{DcgClient, TraceContext};
use phonelink_proto::clipboard::v1::{
    ClipboardErrorType, ClipboardItem, ClipboardItemType, ClipboardResponseMessage, ClipboardResponseStatus,
};
use phonelink_proto::platform_sdk::v1::ProtoDeviceResourceRequestType;
use phonelink_transport::auth::PeerDevice;
use phonelink_transport::platform::{DrmAddress, DrmSendOutcome, DrmSendRequest, Publication, PublicationObserver};

/// The tag-9 subscriber. Holds the feature weakly: the MSAEP layer keeps it.
pub(super) struct AnnouncementObserver(pub(super) Weak<Clipboard>);

impl PublicationObserver for AnnouncementObserver {
    fn on_publication(&self, publication: &Publication) {
        if let Some(clipboard) = self.0.upgrade() {
            clipboard.on_announcement(publication);
        }
    }
}

impl Clipboard {
    /// Processing steps 1–5 of an inbound announcement. Runs on the router's task: it only reads
    /// the gates and spawns the pull.
    fn on_announcement(self: &Arc<Self>, publication: &Publication) {
        if self.is_stopped() {
            return;
        }
        let Some(peer) = self.announcing_phone(publication) else {
            return;
        };
        let Some(message) = change_announcement(&peer, publication) else {
            return;
        };

        // 5. The pull, with the announcement's correlation id.
        debug!(phone = %peer.client, correlation_id = %message.correlation_id, "phone clipboard changed; pulling");
        let me = self.clone();
        tokio::spawn(async move { me.pull(peer.client, message.correlation_id).await });
    }

    /// Steps 1–2: the phone the announcement came from, when it is known and its gates are open.
    fn announcing_phone(&self, publication: &Publication) -> Option<PeerDevice> {
        let message_id = &publication.message.message_id;

        // 1. The phone is the partner of the connection the message arrived on.
        let Some(peer) = self.peers.peer(&publication.source) else {
            warn!(partner = %publication.source, %message_id, "announcement from a phone unknown to the account; dropped");
            return None;
        };

        // 2. Global switch, phone enabled, `CLIPBOARD`, toggle — no clipboard-level reply otherwise.
        match self.phone_gates_open(&peer) {
            Ok(true) => Some(peer),
            Ok(false) => {
                debug!(phone = %peer.client, %message_id, "announcement dropped by the gates");
                None
            }
            Err(e) => {
                warn!(phone = %peer.client, error = %e, "settings unreadable; announcement dropped");
                None
            }
        }
    }

    /// The content pull: `GET /clipboard` `{ CONTENT, correlation_id }` through the request wrapper,
    /// with a forced session start, no cancellation of its own (only the feature's shutdown abandons
    /// it) and no timeout but the platform request's. Any failure is logged; nothing is written and
    /// nothing retried.
    async fn pull(self: Arc<Self>, phone: DcgClient, correlation_id: String) {
        if self.is_stopped() {
            debug!(%phone, "feature stopped; content pull not sent");
            return;
        }
        let Some((peer, phone_enabled)) = self.pull_target(&phone) else {
            return;
        };

        let request =
            content_pull(&peer, &correlation_id, phone_enabled, self.config.request_timeout, self.request_cancel());
        let Some(outcome) = self.send_request(request).await else {
            debug!(%phone, "feature stopped; content pull not sent");
            return;
        };

        match outcome {
            DrmSendOutcome::Success { payload } => self.apply_content(&peer.client, payload).await,
            outcome => warn!(%phone, %correlation_id, ?outcome, "content pull failed; nothing written"),
        }
    }

    /// The phone a content pull goes to and its enabled switch, `None` (logged) when it left the
    /// account's device list or its switch is unreadable.
    fn pull_target(&self, phone: &DcgClient) -> Option<(PeerDevice, bool)> {
        // The phone must (still) be in the current account's device list.
        let Some(peer) = self.peers.peer(phone) else {
            debug!(%phone, "phone left the account's device list; content pull not sent");
            return None;
        };

        match self.switches.is_enabled(&peer.client) {
            Ok(enabled) => Some((peer, enabled)),
            Err(e) => {
                warn!(%phone, error = %e, "phone switch unreadable; content pull not sent");
                None
            }
        }
    }

    /// Handles the content answer of a pull: writes its item to the pasteboard.
    async fn apply_content(self: &Arc<Self>, phone: &DcgClient, payload: Vec<u8>) {
        let Some(item) = content_item(phone, &payload) else {
            return;
        };

        let me = self.clone();
        let written = blocking("pasteboard write", move || me.write_item(item)).await;
        match written {
            Some(Ok(Some(kind))) => {
                info!(%phone, ?kind, "phone clipboard written to the pasteboard");
                self.emit(ClipboardEvent::Received { phone: phone.clone(), kind });
            }
            Some(Ok(None)) | None => {}
            Some(Err(e)) => warn!(%phone, error = %e, "phone clip not written to the pasteboard"),
        }
    }

    /// Writes one item by its `item_type`; `Ok(None)` when the item fails its condition (absent or
    /// empty `text` / `image_bytes`, another type) — an empty clip never clears the pasteboard.
    /// Records the change count of the write for echo suppression, also of a write the pasteboard
    /// stopped part-way (it changed the pasteboard all the same). Blocking.
    fn write_item(&self, item: ClipboardItem) -> Result<Option<ClipKind>, ClipWriteError> {
        let Some((kind, representations)) = self.item_representations(item)? else {
            return Ok(None);
        };
        if self.is_stopped() {
            return Ok(None);
        }

        let mut watch = self.watch.lock_or_panic();
        let written = self.pasteboard.write(representations).map_err(ClipWriteError::Pasteboard)?;
        watch.own_write = Some(written.change_count);
        if let Some(failure) = written.failure {
            return Err(ClipWriteError::StoppedPartWay(failure));
        }
        Ok(Some(kind))
    }

    /// The kind and pasteboard representations of one item by its `item_type`; `Ok(None)` when the
    /// item fails its condition. Blocking (an image is decoded).
    fn item_representations(&self, item: ClipboardItem) -> Result<Option<ItemWrite>, ClipWriteError> {
        match ClipboardItemType::try_from(item.item_type) {
            Ok(ClipboardItemType::TextPlain) => Ok(plain_text_write(item.text)),
            Ok(ClipboardItemType::TextHtml) => Ok(html_write(item.text)),
            Ok(ClipboardItemType::Image) => self.image_write(item.image_bytes),
            Ok(ClipboardItemType::Unspecified) | Err(_) => {
                warn!(item_type = item.item_type, "item of an unsupported type; nothing written");
                Ok(None)
            }
        }
    }

    /// An `IMAGE` item's write: PNG and TIFF of its decoded bytes. Blocking.
    fn image_write(&self, image_bytes: Option<Vec<u8>>) -> Result<Option<ItemWrite>, ClipWriteError> {
        let Some(bytes) = image_bytes.filter(|b| !b.is_empty()) else {
            warn!("IMAGE item without bytes; nothing written");
            return Ok(None);
        };

        // Decoded first: an image that does not decode writes nothing. No limit, no resize.
        let (png, tiff) = pasteboard_image(&*self.codec, &bytes).map_err(ClipWriteError::Image)?;
        Ok(Some((ClipKind::Image, vec![representation(uti::PNG, png), representation(uti::TIFF, tiff)])))
    }
}

/// Why a received item was not written to the pasteboard.
#[derive(Debug, thiserror::Error)]
enum ClipWriteError {
    #[error("pasteboard write: {0}")]
    Pasteboard(#[source] EdgeError),
    /// The pasteboard refused a representation; the ones before it were written.
    #[error("pasteboard write stopped part-way (its change count is recorded): {0}")]
    StoppedPartWay(String),
    #[error("the received image does not decode or convert: {0}")]
    Image(#[source] EdgeError),
}

/// What one item writes: its kind and its pasteboard representations.
type ItemWrite = (ClipKind, Vec<ClipboardPasteboardData>);

/// A `TEXT_PLAIN` item's write.
fn plain_text_write(text: Option<String>) -> Option<ItemWrite> {
    let Some(text) = text.filter(|t| !t.is_empty()) else {
        warn!("TEXT_PLAIN item without text; nothing written");
        return None;
    };
    Some((ClipKind::Text, vec![representation(uti::PLAIN_TEXT, text.into_bytes())]))
}

/// A `TEXT_HTML` item's write: the markup, and its plain text when that is not empty.
fn html_write(text: Option<String>) -> Option<ItemWrite> {
    let Some(markup) = text.filter(|t| !t.is_empty()) else {
        warn!("TEXT_HTML item without text; nothing written");
        return None;
    };

    let plain = html::plain_text(&markup);
    let mut representations = vec![representation(uti::HTML, markup.into_bytes())];
    if !plain.is_empty() {
        representations.push(representation(uti::PLAIN_TEXT, plain.into_bytes()));
    }
    Some((ClipKind::Html, representations))
}

/// Steps 3–4 of an announcement: its message, when it parses and announces `CLIPBOARD_CHANGE`.
fn change_announcement(peer: &PeerDevice, publication: &Publication) -> Option<ClipboardResponseMessage> {
    let message_id = &publication.message.message_id;

    // 3. The message is in `additional`; one only in `data` parses as empty here.
    let message = match ClipboardResponseMessage::decode(publication.payload.additional.as_slice()) {
        Ok(message) => message,
        Err(e) => {
            warn!(phone = %peer.client, %message_id, error = %e, "announcement does not parse; dropped");
            return None;
        }
    };

    // 4. Only CLIPBOARD_CHANGE is acted on.
    if message.status != ClipboardResponseStatus::ClipboardChange as i32 {
        debug!(phone = %peer.client, status = message.status, "announcement is not CLIPBOARD_CHANGE; dropped");
        return None;
    }
    Some(message)
}

/// The one item of a pull's content answer, `None` (logged) when the answer is empty, does not
/// parse, has a status other than `OK` or not exactly one item.
fn content_item(phone: &DcgClient, payload: &[u8]) -> Option<ClipboardItem> {
    if payload.is_empty() {
        warn!(%phone, "content answer without payload; nothing written");
        return None;
    }

    let response = match ClipboardResponseMessage::decode(payload) {
        Ok(response) => response,
        Err(e) => {
            warn!(%phone, error = %e, "content answer does not parse; nothing written");
            return None;
        }
    };

    if response.status != ClipboardResponseStatus::Ok as i32 {
        log_status(phone, &response);
        return None;
    }
    let Ok([item]) = <[ClipboardItem; 1]>::try_from(response.clipboard_item) else {
        warn!(%phone, "content answer is OK without exactly one item; nothing written");
        return None;
    };
    Some(item)
}

/// The content pull through the request wrapper: `GET /clipboard` (path-addressed) with `{ CONTENT,
/// correlation_id }`, a forced session start, the platform request timeout, and no cancellation of
/// its own (`cancel` is the feature's shutdown).
pub(super) fn content_pull(
    peer: &PeerDevice,
    correlation_id: &str,
    phone_enabled: bool,
    timeout: Duration,
    cancel: CancellationToken,
) -> DrmSendRequest {
    DrmSendRequest {
        target: peer.client.clone(),
        address: DrmAddress::Path(RESOURCE_PATH.to_owned()),
        request_type: ProtoDeviceResourceRequestType::Get,
        payload: wire::content_request(correlation_id),
        client_version: peer.info.client_version.clone(),
        phone_enabled,
        force_session_start: true,
        timeout,
        trace: TraceContext::new_root(Vec::new()),
        cancel,
    }
}

/// Logs a content answer's status other than `OK`, which writes nothing (for `INVALID_CONTENT`,
/// `error_type` and `error_detail` select the text).
fn log_status(phone: &DcgClient, response: &ClipboardResponseMessage) {
    let status = ClipboardResponseStatus::try_from(response.status).map_or("unknown", |s| s.as_str_name());
    if response.status == ClipboardResponseStatus::InvalidContent as i32 {
        let kind = match ClipboardErrorType::try_from(response.error_type) {
            Ok(ClipboardErrorType::Reject) => "Reject",
            Ok(ClipboardErrorType::Fail) => "Fail",
            _ => "Unspecified",
        };
        warn!(%phone, kind, raw_error_detail = %response.error_detail, "phone answered INVALID_CONTENT; nothing written");
    } else {
        warn!(%phone, status, code = response.status, "phone answered the content pull with a status other than OK; nothing written");
    }
}

#[cfg(test)]
mod tests;

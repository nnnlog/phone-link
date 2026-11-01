//! Outbound `/chat/v1` requests and the messages permission prompt: the keys common to every
//! `/chat/v1` request, the sync request, the media download request, mark conversation as read, and
//! the messages permission request.
//!
//! Every request is a legacy App request with default options except those sent "user-initiated";
//! there is no messaging-level retry. Requests a trigger sends "at once" from an inbound message or
//! a UI callback are spawned, never awaited there.

use std::collections::BTreeSet;
use std::sync::Arc;

use tokio_util::sync::CancellationToken;
use tracing::{debug, error, info, warn};

use phonelink_app::consts::permission;
use phonelink_app::{AppError, AppLayer, AppOutcome, AppRequest, AppResponse, RequestOptions};
use phonelink_core::codec::valueset::ValueSet;
use phonelink_core::types::{DcgClient, TraceContext};

use super::caps::Predicates;
use super::consts::{CONTENT_TYPE, RCS_UNSUPPORTED_MESSAGE_TYPES, ROUTE, key, request_type, sub};
use super::store::{self, PcSettings};
use super::views::{self, PartState};
use super::{Inner, Messaging, MessagingError, MessagingEvent};
use phonelink_core::sync::MutexExt;

/// The six `configuration` keys every messaging request (and the connect message) carries.
pub(super) fn configuration(p: &Predicates) -> ValueSet {
    let mut c = ValueSet::new()
        .with(key::MESSAGING_SYNC, true)
        .with(key::MESSAGING_MASS_TEXT, true)
        .with(key::MESSAGING_SEND, p.send())
        .with(key::MESSAGING_RCS_RECEIVE, p.rcs_receive())
        .with(key::MESSAGING_RCS_SEND, p.rcs_send());

    if p.rcs_receive() {
        c.insert(key::MESSAGING_RCS_UNSUPPORTED_MESSAGE_TYPES, RCS_UNSUPPORTED_MESSAGE_TYPES.to_vec());
    }
    c
}

/// A `/chat/v1` legacy request: `correlationVector` (the request's trace correlation id),
/// `contentType`, the request's own keys in order, then `configuration`.
pub(super) fn chat_request(
    keys: ValueSet,
    p: &Predicates,
    user_initiated: bool,
    trigger: &str,
    scenario: &str,
) -> AppRequest {
    let trace = TraceContext::new_root(Vec::new());
    let mut payload = ValueSet::new()
        .with(key::CORRELATION_VECTOR, trace.correlation_id.clone())
        .with(key::CONTENT_TYPE, CONTENT_TYPE);

    for (k, v) in keys.iter() {
        payload.insert(k, v.clone());
    }
    payload.insert(key::CONFIGURATION, configuration(p));

    let options = if user_initiated { RequestOptions::user_initiated() } else { RequestOptions::default() };
    AppRequest::legacy(ROUTE, payload).with_options(options.named(trigger, scenario)).with_trace(trace)
}

/// The sync request (no `requestType`).
pub(super) fn sync_request(p: &Predicates, settings: &PcSettings, user_initiated: bool, trigger: &str) -> AppRequest {
    let mut subcontent = vec![sub::MESSAGES, sub::MESSAGES_MMS, sub::CONVERSATIONS, sub::SUBSCRIPTIONS];
    if settings.receive_mms && settings.auto_download_mms {
        subcontent.push(sub::MESSAGES_MMS_MEDIA);
    }
    if p.rcs_receive() {
        subcontent.extend([sub::MESSAGES_RCS_CHAT, sub::MESSAGES_RCS_FILE_TRANSFER, sub::CONVERSATIONS_RCS]);
    }

    chat_request(ValueSet::new().with(key::SUBCONTENT_TYPES, subcontent), p, user_initiated, trigger, "SyncAllMessages")
}

/// A part of a shown message: what a media download names (an MMS part or an RCS file) and where
/// the part's bytes are read from ([`Messaging::part_bytes`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum PartRef {
    /// An MMS part, by part id.
    MmsPart(i64),
    /// An RCS file, by file-transfer id.
    RcsFile(i64),
    /// Attachment `index` of a pending send (never downloaded: its bytes are stored when it is sent).
    SentPart { send_id: i64, index: u32 },
}

/// What a media download request asks for: one MMS part or RCS file.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(super) enum Media {
    MmsPart(i64),
    RcsFile(i64),
}

fn media_request(p: &Predicates, media: Media) -> AppRequest {
    let mut keys = ValueSet::new().with(key::SUBCONTENT_TYPES, vec![sub::MESSAGES_MMS_MEDIA]);
    match media {
        Media::MmsPart(id) => keys.insert(key::MMS_PART_IDS, vec![id]),
        Media::RcsFile(id) => keys.insert(key::MESSAGING_RCS_FILE_TRANSFER_MEDIA_IDS, vec![id]),
    }
    keys.insert(key::REQUESTED_ITEMS, 1i32);
    chat_request(keys, p, false, "DownloadAttachment", "DownloadAttachment")
}

pub(super) fn mark_as_read_request(p: &Predicates, thread_id: i64, timestamp_ms: i64, trigger: &str) -> AppRequest {
    let keys = ValueSet::new()
        .with(key::REQUEST_TYPE, request_type::MARK_CONVERSATION_AS_READ)
        .with(key::THREAD_ID, thread_id)
        .with(key::TIMESTAMP, timestamp_ms);
    chat_request(keys, p, true, trigger, "MarkAsRead")
}

fn log_sync_request(trigger: &str, user_initiated: bool, sent: &Result<AppResponse, AppError>) {
    match sent {
        Ok(response) => info!(trigger, user_initiated, outcome = ?response.outcome, "messaging sync request finished"),
        Err(e) => warn!(trigger, error = %e, "messaging sync request not sent"),
    }
}

/// The permission state an outcome leaves (the sync request's mapping): `Some(granted)` or unchanged.
pub(super) fn permission_from_outcome(outcome: &AppOutcome) -> Option<bool> {
    match outcome {
        AppOutcome::Success | AppOutcome::Accepted => Some(true),
        AppOutcome::Denied | AppOutcome::PermanentlyDenied | AppOutcome::NeedsPermission => Some(false),
        _ => None,
    }
}

impl Inner {
    /// Applies a sync, send or prompt outcome to the phone messages permission; a grant also marks
    /// messaging "set up once".
    pub(super) fn apply_permission_outcome(&self, app: &AppLayer, phone: &DcgClient, outcome: &AppOutcome) {
        let Some(granted) = permission_from_outcome(outcome) else {
            debug!(?outcome, "messages permission unchanged by the outcome");
            return;
        };

        app.update_phone_record(|r| r.set_permission(permission::MESSAGES, granted));
        if granted {
            if let Err(e) = self
                .db(phone)
                .and_then(|db| db.transaction(|tx| store::set_setup_completed_once(tx).map_err(Into::into)))
            {
                error!(error = %e, "messaging set-up-once flag not stored; the prompt may show again");
            }
        }
    }

    /// Sends the sync request and applies its outcome.
    async fn run_sync_request(&self, app: &AppLayer, phone: &DcgClient, user_initiated: bool, trigger: &str) {
        let (p, settings) = match app.phone_record() {
            Some(record) => match self
                .predicates(phone, &record)
                .and_then(|p| Ok((p, self.db(phone)?.read(store::phone_state)?.settings)))
            {
                Ok(v) => v,
                Err(e) => {
                    error!(error = %e, "messaging sync request not sent; the store failed");
                    return;
                }
            },
            None => return,
        };

        let sent = app.request(sync_request(&p, &settings, user_initiated, trigger)).await;
        log_sync_request(trigger, user_initiated, &sent);
        if let Ok(response) = sent {
            self.apply_permission_outcome(app, phone, &response.outcome);
        }
    }

    /// Sync trigger (b): the sync request, not user-initiated, without waiting for it.
    pub(super) fn spawn_sync_request(self: &Arc<Self>, user_initiated: bool, trigger: &'static str) {
        let Ok(app) = self.app() else { return };
        let Some(phone) = app.selected_phone() else { return };
        let inner = Arc::clone(self);
        self.runtime.spawn(async move {
            inner.run_sync_request(&app, &phone, user_initiated, trigger).await;
        });
    }

    /// Sync trigger (a): after the permission prompt, when the permission is granted and no such sync
    /// is running — the sync request with the contacts sync request, both user-initiated.
    pub(super) async fn permission_flow_sync(&self, app: &AppLayer, phone: &DcgClient) {
        if !self.try_begin_updating(phone) {
            debug!("messages permission granted; a sync is already running");
            return;
        }

        self.emit(MessagingEvent::SyncStateChanged {
            phone: phone.clone(),
            state: super::SyncState {
                up_to_date: self.sync_state(phone).map(|s| s.up_to_date).unwrap_or(true),
                syncing: true,
            },
        });

        match &self.contacts {
            Some(contacts) => contacts.request_sync(true),
            None => warn!("/contacts/v2 of the messages permission flow not sent; no contact directory"),
        }

        self.run_sync_request(app, phone, true, "MessagesDevicePermission").await;
        self.end_updating(phone);
    }

    // --------------------------------------------------------------------------------------------
    // Mark conversation as read
    // --------------------------------------------------------------------------------------------

    /// The check every mark-as-read trigger runs: nothing unless the phone is connected and the
    /// conversation has PC-unread or phone-unread messages; then the PC clears its own unread count
    /// and, when there are phone-unread messages and the phone supports it, sends `requestType` 3.
    pub(super) async fn mark_as_read(self: Arc<Self>, phone: DcgClient, thread_id: i64, trigger: &'static str) {
        if !self.sessions.is_connected(&phone) {
            return;
        }

        let Ok(app) = self.app() else { return };
        let Some(record) = app.phone_record().filter(|r| r.phone() == phone) else { return };

        let result = self.db(&phone).and_then(|db| {
            let counts = db.read(|c| store::conversation_counts(c, thread_id))?;
            if counts.pc_unread > 0 {
                db.transaction(|tx| store::mark_read_on_pc(tx, thread_id).map_err(super::StoreError::from))?;
            }
            Ok((counts, self.predicates(&phone, &record)?))
        });
        let (counts, p) = match result {
            Ok(v) => v,
            Err(e) => {
                error!(error = %e, "mark as read not applied; the store failed");
                return;
            }
        };

        if counts.pc_unread == 0 && counts.phone_unread == 0 {
            return;
        }

        if counts.pc_unread > 0 {
            self.emit(MessagingEvent::ContentChanged { phone: phone.clone(), threads: vec![thread_id] });
        }

        if counts.phone_unread > 0 && p.mark_conversation_as_read() {
            let request = mark_as_read_request(&p, thread_id, counts.latest_ms, trigger);
            // The response is ignored; there is no retry.
            if let Err(e) = app.request(request).await {
                warn!(trigger, error = %e, "mark-as-read request not sent");
            }
        }
    }

    fn spawn_mark_as_read(self: &Arc<Self>, phone: DcgClient, thread_id: i64, trigger: &'static str) {
        self.runtime.spawn(Arc::clone(self).mark_as_read(phone, thread_id, trigger));
    }

    /// Whether the app window is focused, as the UI last reported it.
    fn window_focused(&self) -> bool {
        self.run.lock_or_panic().ui.window_focused
    }

    /// The 500 ms trigger: a newer trigger (or losing focus) cancels it; it runs only when the app
    /// window is focused when the delay ends.
    fn delayed_mark_as_read(self: &Arc<Self>, phone: DcgClient, thread_id: i64, trigger: &'static str) {
        let token = CancellationToken::new();
        if let Some(old) = self.run.lock_or_panic().ui.delayed.replace(token.clone()) {
            old.cancel();
        }

        let inner = Arc::clone(self);
        let delay = self.config.mark_as_read_delay;
        let shutdown = self.shutdown.clone();
        self.runtime.spawn(async move {
            tokio::select! {
                _ = token.cancelled() => {}
                _ = shutdown.cancelled() => {}
                _ = tokio::time::sleep(delay) => {
                    if !token.is_cancelled() && inner.window_focused() {
                        inner.mark_as_read(phone, thread_id, trigger).await;
                    }
                }
            }
        });
    }

    fn cancel_delayed_mark_as_read(&self) {
        if let Some(t) = self.run.lock_or_panic().ui.delayed.take() {
            t.cancel();
        }
    }

    /// Synced content updated conversations: the selected one is marked read at once when the window
    /// is focused and the messages view is active.
    pub(super) fn on_synced_threads(self: &Arc<Self>, phone: &DcgClient, threads: &BTreeSet<i64>) {
        let selected = {
            let run = self.run.lock_or_panic();
            run.ui.selected.filter(|t| threads.contains(t) && run.ui.messages_view_active && run.ui.window_focused)
        };
        if let Some(thread_id) = selected {
            self.spawn_mark_as_read(phone.clone(), thread_id, "UpdateExistingConversation");
        }
    }
}

impl Messaging {
    /// The user selected a conversation: mark it read 500 ms later, if no newer trigger cancelled it
    /// and the app window is focused then. `None` (no selection) is recorded only: it is not a
    /// trigger, so it cancels nothing.
    pub fn on_conversation_selected(&self, thread_id: Option<i64>) {
        self.inner.run.lock_or_panic().ui.selected = thread_id;
        let phone = self.inner.app.upgrade().and_then(|a| a.selected_phone());
        if let (Some(t), Some(phone)) = (thread_id, phone) {
            self.inner.delayed_mark_as_read(phone, t, "SetConversationSelected");
        }
    }

    /// The app window gained (`true`) or lost focus. Gaining it with a conversation selected in the
    /// active messages view marks it read 500 ms later; losing it cancels a pending trigger. The
    /// focus is kept: the triggers that need a focused window read it (the app being active, which
    /// the App layer's foreground state tracks, is not enough — another of its windows may be key).
    pub fn on_window_focused(&self, focused: bool) {
        let (selected, active) = {
            let mut run = self.inner.run.lock_or_panic();
            run.ui.window_focused = focused;
            (run.ui.selected, run.ui.messages_view_active)
        };

        let phone = self.inner.app.upgrade().and_then(|a| a.selected_phone());
        match (selected, phone) {
            (Some(t), Some(phone)) if focused && active => {
                self.inner.delayed_mark_as_read(phone, t, "ConversationFocused")
            }
            _ => self.inner.cancel_delayed_mark_as_read(),
        }
    }

    /// The user navigated to (`true`) or away from the messages view: arriving with a conversation
    /// selected marks it read at once.
    pub fn on_messages_view_changed(&self, active: bool) {
        let selected = {
            let mut run = self.inner.run.lock_or_panic();
            run.ui.messages_view_active = active;
            run.ui.selected
        };

        let phone = self.inner.app.upgrade().and_then(|a| a.selected_phone());
        if let (true, Some(t), Some(phone)) = (active, selected, phone) {
            self.inner.spawn_mark_as_read(phone, t, "NavigatedToMessages");
        }
    }

    /// A conversation composed on the PC was replaced by its synced conversation `thread_id`, which
    /// is now the selected one: marked read at once when the window is focused and the messages view
    /// is active.
    pub fn on_composed_conversation_replaced(&self, thread_id: i64) {
        let shown = {
            let mut run = self.inner.run.lock_or_panic();
            run.ui.selected = Some(thread_id);
            run.ui.messages_view_active && run.ui.window_focused
        };

        let Some(app) = self.inner.app.upgrade() else { return };
        if let (true, Some(phone)) = (shown, app.selected_phone()) {
            self.inner.spawn_mark_as_read(phone, thread_id, "UpdateTemporaryConversation");
        }
    }

    /// The user acted on the "allow messages on your phone" prompt: `/permission` type 2, its
    /// outcome applied to the permission state, then — when the permission is granted and no such
    /// sync is running — the sync request and the contacts sync request, not waited for.
    pub async fn request_messages_permission(&self) -> Result<AppOutcome, MessagingError> {
        let (app, phone, _) = self.inner.selected()?;
        let request = app.permission_request(
            permission::MESSAGES,
            false,
            RequestOptions::user_initiated().named("ErrorActionInvoked", "MessagesDevicePermission"),
        );

        let response = app.request(request).await?;
        self.inner.apply_permission_outcome(&app, &phone, &response.outcome);

        let granted = app.phone_record().is_some_and(|r| Inner::phone_permission(&r));
        if granted {
            let inner = Arc::clone(&self.inner);
            self.inner.runtime.spawn(async move {
                inner.permission_flow_sync(&app, &phone).await;
            });
        }
        Ok(response.outcome)
    }

    /// Asks the phone for one part's bytes; only a part shown as
    /// downloadable may be requested, and only once at a time. Returns the request's outcome, which
    /// the protocol does not act on: the bytes arrive later as an item (a `ContentChanged` event).
    pub async fn download_part(&self, part: PartRef) -> Result<AppOutcome, MessagingError> {
        let (app, phone, record) = self.inner.selected()?;
        let (media, id) = match part {
            PartRef::MmsPart(id) => (Media::MmsPart(id), id),
            PartRef::RcsFile(id) => (Media::RcsFile(id), id),
            PartRef::SentPart { .. } => return Err(MessagingError::NotDownloadable),
        };

        let db = self.inner.db(&phone)?;
        match db.read(|c| views::part_state_of(c, &self.inner, part))? {
            None => return Err(MessagingError::UnknownPart(id)),
            Some(PartState::Downloadable) => {}
            Some(_) => return Err(MessagingError::NotDownloadable),
        }

        let key = (phone.clone(), media);
        if !self.inner.run.lock_or_panic().downloading.insert(key.clone()) {
            return Err(MessagingError::NotAllowed("a download of this part is under way"));
        }

        let result = match self.inner.predicates(&phone, &record) {
            Ok(p) => app.request(media_request(&p, media)).await.map(|r| r.outcome).map_err(MessagingError::from),
            Err(e) => Err(e.into()),
        };

        self.inner.run.lock_or_panic().downloading.remove(&key);
        result
    }
}

#[cfg(test)]
mod tests;

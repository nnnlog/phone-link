//! Reading the engine's messaging data again.
//!
//! Its state, the contacts' state, the conversations and the shown transcript are read on the engine's
//! messaging events and when a new engine core starts.

use std::pin::Pin;

use cxx_qt_lib::QString;
use phonelink_engine::api::FeatureApiError;
use phonelink_engine::api::messaging::{MessagePartRef, MessagingChange};
use tracing::{info, warn};

use super::refresh::{ALL, DETAIL, LIST, TRANSCRIPT};
use crate::engine_bridge::EngineBridge;
use crate::engine_bridge::features::Revision;
use crate::views::messages::{Detail, unsent_groups};

pub(super) fn not_started(error: &FeatureApiError) -> bool {
    matches!(error, FeatureApiError::NotStarted)
}

/// A read that failed: the last data stays shown.
///
/// Before the engine starts every read fails, which only means there is nothing to show yet.
pub(super) fn record(error: &FeatureApiError, subject: &str) {
    if !not_started(error) {
        warn!(what = subject, %error, "a read for the messages views failed; the last data stays shown");
    }
}

impl EngineBridge {
    pub(in crate::engine_bridge) fn reload_conversations(mut self: Pin<&mut Self>) {
        self.as_mut().reload_state();
        self.as_mut().reload_contacts_state();
        self.reload_snapshot();
    }

    pub(in crate::engine_bridge) fn reload_messages(self: Pin<&mut Self>) {
        self.reload_transcript();
    }

    pub(super) fn reload_state(mut self: Pin<&mut Self>) {
        let Some(engine) = self.engine.clone() else { return };
        match engine.messaging().state() {
            Ok(state) => {
                self.as_mut().msg().state = Some(state);
                // The gate may have closed or lifted.
                self.as_mut().report_selection(false);
                self.mark(LIST | DETAIL);
            }
            Err(e) => record(&e, "the messaging state was not read"),
        }
    }

    pub(super) fn reload_contacts_state(mut self: Pin<&mut Self>) {
        let Some(engine) = self.engine.clone() else { return };
        match engine.contacts().state() {
            Ok(loaded) => {
                let state = self.as_mut().msg();
                let changed = state.contacts.as_ref().map(|c| c.access) != Some(loaded.access);
                state.contacts = Some(loaded);
                if changed {
                    self.as_mut().schedule_suggestions();
                }
                self.mark(DETAIL);
            }
            Err(e) => record(&e, "the contacts state was not read"),
        }
    }

    /// The conversations, the SIMs, what was sent to new recipients and the new message's limits.
    pub(super) fn reload_snapshot(mut self: Pin<&mut Self>) {
        let Some(engine) = self.engine.clone() else { return };
        let messaging = engine.messaging();
        let read = (|| {
            Ok::<_, FeatureApiError>((
                messaging.conversations()?,
                messaging.subscriptions()?,
                messaging.new_conversation_messages()?,
                messaging.new_message_limits()?,
            ))
        })();
        let (conversations, subscriptions, unsent, limits) = match read {
            Ok(snapshot) => snapshot,
            Err(e) => return record(&e, "the conversations were not read"),
        };

        let state = self.as_mut().msg();
        state.conversations = conversations;
        state.subscriptions = subscriptions;
        state.unsent = unsent_groups(&unsent);
        state.limits = Some(limits);

        if let Some(thread_id) = state.selection()
            && state.conversation(thread_id).is_none()
        {
            self.as_mut().set_detail(None);
        }
        // What was sent to new recipients shows in their composer or their conversation.
        if !self.as_mut().adopt_composed_conversation() {
            self.as_mut().arrivals_seen();
            self.as_mut().mark(TRANSCRIPT);
        }
        if self.areas.messages.detail == Some(Detail::NewMessage) && !self.areas.messages.can_add_recipient() {
            self.as_mut().cancel_suggestions();
        }
        self.mark(LIST | DETAIL);
    }

    /// Reads the shown conversation's messages again (also the detail's Try Again).
    pub(super) fn reload_transcript(mut self: Pin<&mut Self>) {
        let owner = self.areas.messages.detail.clone();
        let state = self.as_mut().msg();
        if state.transcript_owner != owner {
            state.transcript_owner = owner.clone();
            state.transcript = Vec::new();
            state.transcript_loaded = false;
            state.transcript_failed = false;
        }
        // The conversation's content may bring the bytes of pictures that had none.
        state.pictures_missing.clear();

        if let (Some(Detail::Conversation(thread_id)), Some(engine)) = (owner, self.engine.clone()) {
            match engine.messaging().messages(thread_id) {
                Ok(messages) => {
                    let state = self.as_mut().msg();
                    state.transcript = messages;
                    state.transcript_loaded = true;
                    state.transcript_failed = false;
                }
                Err(e) => {
                    record(&e, "the messages were not read");
                    let state = self.as_mut().msg();
                    // Before the engine starts there is nothing to show yet, which is no failure.
                    state.transcript_failed = !state.transcript_loaded && !not_started(&e);
                }
            }
        }
        self.as_mut().arrivals_seen();
        self.mark(TRANSCRIPT);
    }

    /// Downloads whose bytes the transcript now shows are no longer awaited.
    fn arrivals_seen(mut self: Pin<&mut Self>) {
        let state = &self.areas.messages;
        let arrived: Vec<MessagePartRef> =
            state.awaited_downloads.keys().copied().filter(|part| state.arrived(*part)).collect();
        let state = self.as_mut().msg();
        for part in arrived {
            state.awaited_downloads.remove(&part);
        }
    }

    /// Names in conversations and messages come from contacts, as do the suggestions and photos.
    pub(super) fn contacts_changed_here(mut self: Pin<&mut Self>) {
        self.as_mut().msg().without_photo.clear();
        self.as_mut().reload_contacts_state();
        self.as_mut().reload_snapshot();
        self.as_mut().reload_transcript();
        self.schedule_suggestions();
    }

    /// A new engine core knows nothing of the window: tell it again what the user is looking at.
    pub(in crate::engine_bridge) fn reapply_messaging_view(mut self: Pin<&mut Self>) {
        let Some(engine) = self.engine.clone() else { return };
        self.as_mut().msg().reported_selection = Some(None);
        let messaging = engine.messaging();
        let result = messaging
            .messages_view_changed(self.areas.messages.view_active)
            .and_then(|()| messaging.window_focused(self.window_focused));
        if let Err(e) = result {
            info!(error = %e, "the messages view state was not passed on");
        }

        self.as_mut().report_selection(false);
        self.as_mut().mark(ALL);
        // The views read the section as soon as the main window shows.
        self.flush_messages();
    }

    pub(in crate::engine_bridge) fn messaging_changed(mut self: Pin<&mut Self>, change: MessagingChange) {
        match change {
            MessagingChange::ContentChanged { threads } => {
                self.as_mut().reload_snapshot();
                // An empty list means every conversation changed.
                if threads.is_empty() || self.areas.messages.selection().is_some_and(|t| threads.contains(&t)) {
                    self.reload_transcript();
                }
            }
            MessagingChange::SendChanged { .. } => {
                self.as_mut().reload_snapshot();
                if self.areas.messages.selection().is_some() {
                    self.reload_transcript();
                }
            }
            MessagingChange::StateChanged | MessagingChange::PermissionChanged { .. } => {
                // The settings decide which conversations take attachments.
                self.as_mut().reload_state();
                self.as_mut().reload_snapshot();
                self.bump(Revision::Messaging);
            }
            MessagingChange::OpenConversation { thread_id } => {
                self.as_mut().set_detail(Some(Detail::Conversation(thread_id)));
                self.show_section(QString::from("messages"));
            }
        }
    }
}

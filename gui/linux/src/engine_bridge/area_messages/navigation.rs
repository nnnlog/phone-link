//! Which detail the window shows.
//!
//! The selection the engine is told, the new message opening and closing, what was sent to new
//! recipients and the conversation that takes its place, and another part's request to write to people.

use std::pin::Pin;
use std::sync::Arc;

use cxx_qt_lib::QString;
use phonelink_engine::api::contacts::ContactsAccess;
use phonelink_engine::api::engine::Engine;
use tracing::warn;

use super::arguments::Compose;
use super::draft::Draft;
use super::recipients::contact_name_for;
use super::refresh::ALL;
use super::reload::record;
use crate::engine_bridge::EngineBridge;
use crate::views::messages::{ComposerTarget, Detail, Recipient, address, clamp_utf16};

impl EngineBridge {
    /// Tells the engine which conversation the window shows.
    ///
    /// None while the list and detail stand aside for the gate, so a conversation the gate hides is
    /// never marked read; the selection again once the gate lifts. `replaced` reports a new message's
    /// conversation taking the composer's place.
    pub(super) fn report_selection(mut self: Pin<&mut Self>, replaced: bool) {
        let Some(engine) = self.engine.clone() else { return };
        let state = &self.areas.messages;
        let shown = if state.gate().is_some() { None } else { state.selection() };
        let messaging = engine.messaging();
        if replaced {
            if let Some(thread_id) = shown {
                match messaging.composed_conversation_replaced(thread_id) {
                    Ok(()) => self.as_mut().msg().reported_selection = Some(Some(thread_id)),
                    Err(e) => record(&e, "the composed conversation's replacement was not passed on"),
                }
            }
            return;
        }

        if state.reported_selection == Some(shown) {
            return;
        }
        match messaging.conversation_selected(shown) {
            Ok(()) => self.as_mut().msg().reported_selection = Some(shown),
            Err(e) => record(&e, "the selected conversation was not passed on"),
        }
    }

    pub(super) fn set_detail(mut self: Pin<&mut Self>, detail: Option<Detail>) {
        if self.areas.messages.detail == detail {
            return;
        }

        let new_message = detail == Some(Detail::NewMessage);
        self.as_mut().msg().detail = detail;
        if !new_message {
            self.as_mut().cancel_suggestions();
        }
        self.as_mut().report_selection(false);
        self.as_mut().reload_transcript();
        self.mark(ALL);
    }

    /// Shows the new message, as far as it was written.
    pub(super) fn start_new_message(mut self: Pin<&mut Self>) {
        if self.areas.messages.detail == Some(Detail::NewMessage) {
            return;
        }

        let before = self.areas.messages.selection();
        self.as_mut().msg().before_new_message = before;
        self.as_mut().set_detail(Some(Detail::NewMessage));
        self.as_mut().reload_contacts_state();
        self.schedule_suggestions();
    }

    /// Escape in an empty new message: it closes, and the conversation shown before it comes back.
    pub(super) fn cancel_new_message(mut self: Pin<&mut Self>) {
        let state = &self.areas.messages;
        if state.detail != Some(Detail::NewMessage) || state.new_message_has_content() {
            return;
        }

        let before = state.shown_before_new_message();
        self.as_mut().set_detail(before);
    }

    /// Deletes the new message being written: its recipients, what is typed in "To:" and its draft.
    pub(super) fn discard_new_message(mut self: Pin<&mut Self>) {
        let state = self.as_mut().msg();
        state.new_recipients.clear();
        state.recipient_query.clear();
        state.drafts.remove(&ComposerTarget::NewMessage);
        self.as_mut().cancel_suggestions();

        if self.areas.messages.detail == Some(Detail::NewMessage) {
            let before = self.areas.messages.shown_before_new_message();
            self.as_mut().set_detail(before);
        }
        self.mark(ALL);
    }

    /// Opens what was sent to new recipients: their messages, and a composer to the same recipients.
    pub(super) fn open_unsent(mut self: Pin<&mut Self>, addresses: &[String]) {
        let state = &self.areas.messages;
        if let Some(Detail::Unsent(people)) = &state.detail {
            let shown: Vec<String> = people.iter().map(|p| p.address.clone()).collect();
            if address::same_recipients(&shown, addresses) {
                return;
            }
        }
        let Some(group) = state.unsent_group(addresses) else {
            warn!(recipient_count = addresses.len(), "no messages were sent to the recipients the list named");
            return;
        };

        let people = group.recipients.iter().map(|r| Recipient { address: r.address.clone(), name: r.name.clone() });
        let detail = Detail::Unsent(people.collect());
        self.as_mut().set_detail(Some(detail));
    }

    /// Moves the messages sent to new recipients to their conversation; `true` when they moved.
    ///
    /// The conversation is the one the phone created for them, or the one those recipients already
    /// had; what is typed meanwhile moves with them, and writing to a hidden conversation shows it
    /// again.
    pub(super) fn adopt_composed_conversation(mut self: Pin<&mut Self>) -> bool {
        let state = &self.areas.messages;
        let Some(Detail::Unsent(people)) = &state.detail else { return false };
        let addresses: Vec<String> = people.iter().map(|p| p.address.clone()).collect();
        let Some(found) = state.conversation_matching(&addresses) else { return false };
        let (thread_id, muted, hidden) = (found.thread_id, found.muted, found.hidden);

        let state = self.as_mut().msg();
        let destination = ComposerTarget::Conversation(thread_id);
        if let Some(leftover) = state.drafts.remove(&ComposerTarget::Recipients(addresses)) {
            // What is being typed stays as it is; a draft the conversation had follows it.
            let kept = state.drafts.remove(&destination);
            state.drafts.insert(destination, leftover.joined(kept));
        }
        state.detail = Some(Detail::Conversation(thread_id));
        // The engine takes this as the selection and marks the conversation read when it is seen.
        self.as_mut().report_selection(true);
        if hidden && let Some(engine) = self.engine.clone() {
            self.as_mut().set_flags(&engine, thread_id, muted, false);
        }
        self.as_mut().reload_transcript();
        self.mark(ALL);
        true
    }

    /// Writes to people (another part's Message button).
    ///
    /// Turns messages on when they are off, then shows the conversation of exactly those recipients,
    /// else the new message addressed to them, keeping what its draft holds; `text` joins the draft.
    pub(super) fn compose(mut self: Pin<&mut Self>, engine: Arc<Engine>, compose: Compose) {
        let addresses: Vec<String> =
            compose.addresses.iter().map(|a| a.trim().to_owned()).filter(|a| !a.is_empty()).collect();
        if addresses.is_empty() {
            warn!("messages.compose names no address");
            return;
        }
        if self.areas.messages.state.as_ref().is_some_and(|s| !s.settings.pc_consent) {
            self.as_mut().turn_on_messages(engine.clone());
        }
        self.as_mut().show_section(QString::from("messages"));

        let found = self.areas.messages.conversation_matching(&addresses).map(|c| c.thread_id);
        let target = match found {
            Some(thread_id) => {
                self.as_mut().set_detail(Some(Detail::Conversation(thread_id)));
                ComposerTarget::Conversation(thread_id)
            }
            None => {
                let granted =
                    self.areas.messages.contacts.as_ref().is_some_and(|c| c.access == ContactsAccess::Granted);
                let recipients = addresses
                    .iter()
                    .map(|a| Recipient {
                        address: a.clone(),
                        name: granted.then(|| contact_name_for(&engine, a)).flatten(),
                    })
                    .collect();
                let state = self.as_mut().msg();
                state.new_recipients = recipients;
                state.recipient_query.clear();
                self.as_mut().cancel_suggestions();
                self.as_mut().start_new_message();
                ComposerTarget::NewMessage
            }
        };

        if !compose.text.is_empty() {
            let max = self.areas.messages.composer().max_text_length;
            let state = self.as_mut().msg();
            let added = Draft { text: clamp_utf16(&compose.text, max), images: Vec::new() };
            let kept = state.drafts.remove(&target);
            let joined = kept.unwrap_or_default().joined(Some(added));
            state.drafts.insert(target, joined);
        }
        self.mark(ALL);
    }
}

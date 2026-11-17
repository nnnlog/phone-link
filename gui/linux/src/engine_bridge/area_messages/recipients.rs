//! The new message's recipients.
//!
//! What is typed in "To:", the phone's contacts suggested for it, and the contacts permission and
//! switch the suggestions need.

use std::pin::Pin;
use std::sync::Arc;
use std::time::Duration;

use cxx_qt::Threading;
use phonelink_engine::api::contacts::ContactsAccess;
use phonelink_engine::api::engine::Engine;
use tracing::warn;

use super::refresh::{DETAIL, LIST};
use super::reload::record;
use crate::engine_bridge::EngineBridge;
use crate::engine_bridge::features::{Failure, Revision, fail_later};
use crate::l10n::translate;
use crate::user_text;
use crate::views::messages::{Detail, Recipient, address};

/// The most contacts the contacts search suggests.
const SUGGESTION_LIMIT: u32 = 8;
/// After the last keystroke in "To:", the contact search waits this long.
const SEARCH_DELAY: Duration = Duration::from_millis(200);

impl EngineBridge {
    pub(super) fn set_recipient_query(mut self: Pin<&mut Self>, query: &str) {
        if self.areas.messages.detail != Some(Detail::NewMessage) {
            return;
        }

        let had_content = self.areas.messages.new_message_has_content();
        self.as_mut().msg().recipient_query = query.to_owned();
        if had_content != self.areas.messages.new_message_has_content() {
            self.as_mut().mark(LIST | DETAIL);
        }
        self.schedule_suggestions();
    }

    /// Return in the "To:" field: the first suggestion becomes a recipient.
    ///
    /// Without suggestions, what was typed does when it is a number or an email address.
    pub(super) fn submit_recipient_query(mut self: Pin<&mut Self>) {
        let state = &self.areas.messages;
        if let Some(first) = state.suggestions.first() {
            let recipient = Recipient { address: first.number.clone(), name: Some(first.name.clone()) };
            return self.append_recipient(recipient);
        }

        let typed = state.recipient_query.trim().to_owned();
        if address::is_plausible(&typed) {
            self.as_mut().append_recipient(Recipient { address: typed, name: None });
        }
    }

    pub(super) fn add_suggestion(self: Pin<&mut Self>, index: usize) {
        let Some(suggestion) = self.areas.messages.suggestions.get(index) else {
            warn!(index, "no suggestion at the index");
            return;
        };
        let recipient = Recipient { address: suggestion.number.clone(), name: Some(suggestion.name.clone()) };
        self.append_recipient(recipient);
    }

    pub(super) fn remove_recipient(mut self: Pin<&mut Self>, address: &str) {
        if self.areas.messages.detail != Some(Detail::NewMessage) {
            return;
        }

        self.as_mut().msg().new_recipients.retain(|r| r.address != address);
        self.as_mut().schedule_suggestions();
        self.mark(LIST | DETAIL);
    }

    fn append_recipient(mut self: Pin<&mut Self>, recipient: Recipient) {
        if !self.areas.messages.can_add_recipient() {
            return;
        }

        let state = self.as_mut().msg();
        if !state.new_recipients.iter().any(|r| address::same(&r.address, &recipient.address)) {
            state.new_recipients.push(recipient);
        }
        state.recipient_query.clear();
        self.as_mut().cancel_suggestions();
        self.mark(LIST | DETAIL);
    }

    pub(super) fn cancel_suggestions(mut self: Pin<&mut Self>) {
        let state = self.as_mut().msg();
        state.suggestion_generation += 1;
        if !state.suggestions.is_empty() {
            state.suggestions.clear();
            self.mark(DETAIL);
        }
    }

    /// Searches the contacts for what is typed in "To:" once typing pauses.
    ///
    /// Only while the contacts may be used and another recipient fits.
    pub(super) fn schedule_suggestions(mut self: Pin<&mut Self>) {
        let state = &self.areas.messages;
        let query = state.recipient_query.trim().to_owned();
        let granted = state.contacts.as_ref().is_some_and(|c| c.access == ContactsAccess::Granted);
        if state.detail != Some(Detail::NewMessage) || query.is_empty() || !granted || !state.can_add_recipient() {
            return self.cancel_suggestions();
        }

        let state = self.as_mut().msg();
        state.suggestion_generation += 1;
        let generation = state.suggestion_generation;
        let thread = self.qt_thread();
        self.runtime.spawn(async move {
            tokio::time::sleep(SEARCH_DELAY).await;
            let _ = thread.queue(move |bridge| bridge.read_suggestions(&query, generation));
        });
    }

    fn read_suggestions(mut self: Pin<&mut Self>, query: &str, generation: u64) {
        let Some(engine) = self.engine.clone() else { return };
        if generation != self.areas.messages.suggestion_generation {
            return;
        }

        match engine.contacts().suggestions(query.to_owned(), SUGGESTION_LIMIT) {
            Ok(found) => {
                let state = self.as_mut().msg();
                state.suggestions = found
                    .into_iter()
                    .filter(|s| !state.new_recipients.iter().any(|r| address::same(&r.address, &s.number)))
                    .collect();
                self.mark(DETAIL);
            }
            Err(e) => record(&e, "the contact suggestions were not read"),
        }
    }

    /// Asks the phone to allow contacts, which the suggestions come from.
    pub(super) fn request_contacts_permission(mut self: Pin<&mut Self>, engine: Arc<Engine>) {
        if self.areas.messages.requesting_contacts {
            return;
        }
        self.as_mut().msg().requesting_contacts = true;
        self.as_mut().mark(DETAIL);

        let thread = self.qt_thread();
        self.runtime.spawn(async move {
            let failure = match engine.contacts().request_permission().await {
                Ok(result) if result.timed_out => Some(translate("Your phone didn't respond.").to_owned()),
                Ok(result) => user_text::outcome_failure(&result.outcome),
                Err(e) => Some(user_text::feature_error(&e)),
            };
            if let Some(text) = failure {
                fail_later(&thread, Failure::new("Contacts Not Allowed", text));
            }
            let _ = thread.queue(|mut bridge| {
                bridge.as_mut().msg().requesting_contacts = false;
                bridge.as_mut().reload_contacts_state();
                bridge.mark(DETAIL);
            });
        });
    }

    /// Turns on "Use my phone's contacts".
    pub(super) fn turn_on_contacts(mut self: Pin<&mut Self>, engine: Arc<Engine>) {
        if let Err(e) = engine.contacts().set_pc_consent(true) {
            self.as_mut().show_failure(Failure::feature("Contacts Not Turned On", &e));
        }
        self.as_mut().reload_contacts_state();
        self.bump(Revision::Settings);
    }
}

/// The contact name of an address the window was asked to write to, from the contacts' suggestions.
pub(super) fn contact_name_for(engine: &Arc<Engine>, address: &str) -> Option<String> {
    let query = address::contact_search(address)?;
    let found = engine.clone().contacts().suggestions(query, SUGGESTION_LIMIT);
    found
        .inspect_err(|e| record(e, "the contacts were not searched for a recipient"))
        .ok()?
        .into_iter()
        .find(|s| address::same(&s.number, address))
        .map(|s| s.name)
        .filter(|name| !name.trim().is_empty())
}

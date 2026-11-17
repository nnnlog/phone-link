//! Sending the shown draft.
//!
//! A draft the engine refused is put back, and a failed message can be sent again.

use std::pin::Pin;
use std::sync::Arc;

use cxx_qt::Threading;
use phonelink_engine::api::engine::Engine;
use phonelink_engine::api::messaging::{MessageAttachment, MessageTarget};

use super::draft::Draft;
use super::refresh::{ALL, DRAFT, LIST, TRANSCRIPT};
use crate::engine_bridge::EngineBridge;
use crate::engine_bridge::features::Failure;
use crate::l10n::{fill, translate};
use crate::views::messages::{ComposerTarget, Detail, Recipient, address, cap_sentence};

impl EngineBridge {
    /// Sends the shown draft.
    ///
    /// The draft clears at once — the message shows in the transcript as it is sent — and comes back,
    /// with the reason, when the engine refuses it. A new message takes a number or an email address
    /// still in the "To:" field, and its recipients lock: the composer then shows what was sent to them
    /// until the phone creates their conversation, which takes its place.
    pub(super) fn send(mut self: Pin<&mut Self>, engine: Arc<Engine>) {
        let state = &self.areas.messages;
        let Some(target) = state.composer_target() else { return };
        if !state.can_submit() {
            return;
        }
        let (destination, sent_to) = match &state.detail {
            Some(Detail::Conversation(thread_id)) => (MessageTarget::Conversation { thread_id: *thread_id }, None),
            Some(Detail::Unsent(people)) => {
                let recipients = people.iter().map(|p| p.address.clone()).collect();
                (MessageTarget::NewConversation { recipients }, None)
            }
            Some(Detail::NewMessage) => {
                let mut people = state.new_recipients.clone();
                let typed = state.recipient_query.trim().to_owned();
                if !typed.is_empty() {
                    if !address::is_plausible(&typed) {
                        let text = fill(
                            translate("\u{201c}%@\u{201d} isn't a phone number or an email address."),
                            std::slice::from_ref(&typed),
                        );
                        return self.show_failure(Failure::new("Message Not Sent", text));
                    }
                    if !people.iter().any(|p| address::same(&p.address, &typed)) {
                        people.push(Recipient { address: typed, name: None });
                    }
                }
                if let Some(cap) = state.recipient_cap().filter(|cap| people.len() > *cap) {
                    return self.show_failure(Failure::new("Message Not Sent", cap_sentence(cap)));
                }
                let recipients = people.iter().map(|p| p.address.clone()).collect();
                (MessageTarget::NewConversation { recipients }, Some(people))
            }
            None => return,
        };

        let state = self.as_mut().msg();
        let draft = state.drafts.remove(&target).unwrap_or_default();
        if let Some(people) = &sent_to {
            state.new_recipients.clear();
            state.recipient_query.clear();
            self.as_mut().cancel_suggestions();
            self.as_mut().set_detail(Some(Detail::Unsent(people.clone())));
        }
        if let ComposerTarget::Conversation(thread_id) = target
            && let Some(c) = self.areas.messages.conversation(thread_id).filter(|c| c.hidden)
        {
            let muted = c.muted;
            self.as_mut().set_flags(&engine, thread_id, muted, false);
        }
        self.as_mut().mark(LIST | DRAFT);

        let attachments: Vec<MessageAttachment> = draft
            .images
            .iter()
            .map(|i| MessageAttachment {
                name: i.name.clone(),
                content_type: i.content_type.clone(),
                bytes: i.bytes.clone(),
            })
            .collect();
        let text = draft.text.clone();
        let thread = self.qt_thread();
        self.runtime.spawn(async move {
            let result = engine.messaging().send(destination, text, attachments).await;
            let _ = thread.queue(move |mut bridge| match result {
                Ok(_) => {
                    bridge.as_mut().reload_snapshot();
                    bridge.reload_transcript();
                }
                Err(e) => {
                    bridge.as_mut().restore(draft, target, sent_to);
                    bridge.show_failure(Failure::feature("Message Not Sent", &e));
                }
            });
        });
    }

    /// A refused send's draft goes back where it was typed, before what was typed there meanwhile.
    ///
    /// A refused new message goes back to the composer of earlier messages to the same recipients, else
    /// to the new message with its recipients, editable again — unless another new message was started.
    fn restore(mut self: Pin<&mut Self>, draft: Draft, target: ComposerTarget, sent_to: Option<Vec<Recipient>>) {
        let state = self.as_mut().msg();
        let Some(sent_to) = sent_to else {
            let kept = state.drafts.remove(&target);
            state.drafts.insert(target, draft.joined(kept));
            return self.mark(LIST | DRAFT);
        };

        let addresses: Vec<String> = sent_to.iter().map(|p| p.address.clone()).collect();
        if state.unsent_group(&addresses).is_some() {
            let composed = ComposerTarget::Recipients(addresses);
            let kept = state.drafts.remove(&composed);
            state.drafts.insert(composed, draft.joined(kept));
            return self.mark(LIST | DRAFT);
        }

        let reopen = state.detail == Some(Detail::Unsent(sent_to.clone()));
        let fresh = state.new_recipients.is_empty()
            && state.recipient_query.trim().is_empty()
            && state.drafts.get(&ComposerTarget::NewMessage).is_none_or(Draft::is_empty);
        if fresh {
            state.new_recipients = sent_to;
            state.drafts.insert(ComposerTarget::NewMessage, draft);
        }
        if reopen {
            self.as_mut().set_detail(Some(Detail::NewMessage));
        }
        self.mark(ALL);
    }

    /// Sends again a message the phone could not deliver.
    pub(super) fn retry(mut self: Pin<&mut Self>, engine: Arc<Engine>, send_id: i64) {
        if !self.as_mut().msg().retrying.insert(send_id) {
            return;
        }
        self.as_mut().mark(TRANSCRIPT);

        let thread = self.qt_thread();
        self.runtime.spawn(async move {
            let result = engine.messaging().retry(send_id).await;
            let _ = thread.queue(move |mut bridge| {
                bridge.as_mut().msg().retrying.remove(&send_id);
                match result {
                    Ok(_) => {
                        bridge.as_mut().reload_snapshot();
                        bridge.as_mut().reload_transcript();
                    }
                    Err(e) => bridge.as_mut().show_failure(Failure::feature("Message Not Sent", &e)),
                }
                bridge.mark(TRANSCRIPT);
            });
        });
    }
}

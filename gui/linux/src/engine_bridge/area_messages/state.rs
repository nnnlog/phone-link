//! What the messages part keeps between calls.
//!
//! The lookups on it are here too: a conversation, what was sent to new recipients, the gate, the
//! selection and the composer's target.

use std::collections::{HashMap, HashSet};
use std::pin::Pin;
use std::sync::Arc;

use cxx_qt::CxxQtType;
use phonelink_clipboard::pasteboard::ClipboardPasteboard;
use phonelink_engine::api::contacts::{ContactSuggestion, ContactsState};
use phonelink_engine::api::messaging::{
    Conversation, Message, MessagePartRef, MessagingState, NewMessageLimits, SimSubscription,
};
use serde_json::Value;

use super::draft::Draft;
use super::refresh::Dirty;
use crate::engine_bridge::EngineBridge;
use crate::views::messages::{ComposerTarget, Detail, Recipient, UnsentGroup, address};

/// What the messages part keeps between calls.
#[derive(Default)]
pub(in crate::engine_bridge) struct State {
    /// The main window shows the Messages section.
    pub(super) view_active: bool,

    // The engine's data.
    /// Every conversation, newest first, hidden ones included.
    pub(super) conversations: Vec<Conversation>,
    /// What this computer sent to new recipients that the phone has not confirmed yet.
    pub(super) unsent: Vec<UnsentGroup>,
    /// The engine's messaging state; `None` until read.
    pub(super) state: Option<MessagingState>,
    pub(super) subscriptions: Vec<SimSubscription>,
    pub(super) contacts: Option<ContactsState>,
    /// What a new message may carry (the default SIM's limits); `None` until read.
    pub(super) limits: Option<NewMessageLimits>,

    // The window's state.
    pub(super) detail: Option<Detail>,
    pub(super) search: String,
    /// The engine's messages of the conversation `transcript_owner` names.
    pub(super) transcript: Vec<Message>,
    pub(super) transcript_owner: Option<Detail>,
    /// A read of the owner's messages succeeded.
    pub(super) transcript_loaded: bool,
    /// The shown conversation's messages could not be read.
    pub(super) transcript_failed: bool,
    /// What the user types in the new message's "To:" field.
    pub(super) recipient_query: String,
    pub(super) suggestions: Vec<ContactSuggestion>,
    pub(super) suggestion_generation: u64,
    /// Failed sends being sent again.
    pub(super) retrying: HashSet<i64>,
    pub(super) requesting_permission: bool,
    pub(super) requesting_contacts: bool,
    /// The Settings window's contacts request is under way.
    pub(super) settings_requesting_contacts: bool,
    /// The new message's recipients (it keeps them while another detail is shown).
    pub(super) new_recipients: Vec<Recipient>,
    pub(super) drafts: HashMap<ComposerTarget, Draft>,
    pub(super) next_image: u64,
    /// Parts whose download request is under way.
    pub(super) requested_downloads: HashSet<MessagePartRef>,
    /// Parts the phone agreed to send, until their bytes arrive: the number of the wait that runs.
    pub(super) awaited_downloads: HashMap<MessagePartRef, u64>,
    pub(super) download_waits: u64,
    /// The conversation shown when the new message opened, shown again when Escape closes it.
    pub(super) before_new_message: Option<i64>,
    /// The selection the engine was last told (`None`: not known).
    pub(super) reported_selection: Option<Option<i64>>,

    // Pictures read off the GUI thread.
    pub(super) photos_reading: HashSet<i64>,
    pub(super) without_photo: HashSet<i64>,
    pub(super) pictures_reading: HashSet<String>,
    /// Pictures whose bytes were not there when read; read again with the conversation's next content.
    pub(super) pictures_missing: HashSet<String>,
    /// The clipboard, read for pasted images (connected the first time).
    pub(super) pasteboard: Option<Arc<dyn ClipboardPasteboard>>,

    // The views' JSON, rebuilt when marked dirty.
    pub(super) dirty: Dirty,
    pub(super) flush_queued: bool,
    pub(super) list_json: Value,
    pub(super) detail_json: Value,
    pub(super) draft_json: Value,
}

impl State {
    pub(super) fn conversation(&self, thread_id: i64) -> Option<&Conversation> {
        self.conversations.iter().find(|c| c.thread_id == thread_id)
    }

    /// The conversation of exactly these recipients.
    pub(super) fn conversation_matching(&self, addresses: &[String]) -> Option<&Conversation> {
        self.conversations.iter().find(|c| {
            let recipients: Vec<String> = c.recipients.iter().map(|r| r.address.clone()).collect();
            address::same_recipients(&recipients, addresses)
        })
    }

    pub(super) fn unsent_group(&self, addresses: &[String]) -> Option<&UnsentGroup> {
        self.unsent.iter().find(|group| address::same_recipients(&group.addresses(), addresses))
    }

    /// Messages are off on this computer or not allowed on the phone: the list and detail stand aside.
    pub(super) fn gate(&self) -> Option<&'static str> {
        let state = self.state.as_ref()?;
        if !state.settings.pc_consent {
            Some("turnedOff")
        } else if !state.phone_permission {
            Some("permissionNeeded")
        } else {
            None
        }
    }

    pub(super) fn selection(&self) -> Option<i64> {
        match self.detail {
            Some(Detail::Conversation(thread_id)) => Some(thread_id),
            _ => None,
        }
    }

    pub(super) fn selected_conversation(&self) -> Option<&Conversation> {
        self.selection().and_then(|thread_id| self.conversation(thread_id))
    }

    pub(super) fn composer_target(&self) -> Option<ComposerTarget> {
        match &self.detail {
            None => None,
            Some(Detail::Conversation(thread_id)) => Some(ComposerTarget::Conversation(*thread_id)),
            Some(Detail::NewMessage) => Some(ComposerTarget::NewMessage),
            Some(Detail::Unsent(people)) => {
                Some(ComposerTarget::Recipients(people.iter().map(|p| p.address.clone()).collect()))
            }
        }
    }

    pub(super) fn draft(&self) -> Option<&Draft> {
        self.composer_target().and_then(|target| self.drafts.get(&target))
    }

    /// The "To:" line's recipients.
    pub(super) fn recipients(&self) -> &[Recipient] {
        match &self.detail {
            Some(Detail::NewMessage) => &self.new_recipients,
            Some(Detail::Unsent(people)) => people,
            Some(Detail::Conversation(_)) | None => &[],
        }
    }

    pub(super) fn list_status(&self) -> &'static str {
        if let Some(gate) = self.gate() {
            return gate;
        }
        if !self.conversations.is_empty() || !self.unsent.is_empty() || self.shows_new_message_row() {
            return "conversations";
        }
        if self.state.as_ref().is_some_and(|s| s.syncing) { "syncing" } else { "empty" }
    }

    /// The conversation shown before the new message opened, when it is still there.
    pub(super) fn shown_before_new_message(&self) -> Option<Detail> {
        let thread_id = self.before_new_message?;
        self.conversation(thread_id).map(|_| Detail::Conversation(thread_id))
    }
}

impl EngineBridge {
    /// The messages part's state, to change.
    pub(super) fn msg(self: Pin<&mut Self>) -> &mut State {
        &mut self.rust_mut().get_mut().areas.messages
    }
}

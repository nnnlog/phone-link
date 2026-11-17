//! The views' JSON.
//!
//! The conversation list, the shown detail, its draft and its transcript, with the pictures they show
//! from the image cache; and the Settings window's Messages pane.

use std::sync::Arc;

use phonelink_engine::api::contacts::ContactsAccess;
use phonelink_engine::api::engine::Engine;
use phonelink_engine::api::messaging::{Conversation, MessagePart, MessagePartRef, MessagePartState, MessageType};
use serde_json::{Value, json};

use super::state::State;
use crate::images;
use crate::l10n::translate;
use crate::messaging_edges::file_types;
use crate::views::messages::{
    ComposerTarget, Detail, TranscriptRow, attachment_kind, bare_content_type, bluetooth_notice, contact_name,
    conversation_matches, conversation_preview, conversation_subtitle, conversation_title, new_message_preview,
    number_line, part_file_name, part_key, sim_name, transcript_row_json, transcript_rows, unsent_matches,
    unsent_preview, unsent_title,
};

/// The image cache kind of contact photos, shared with the contacts views.
pub(super) const PHOTOS: &str = "contacts";
/// The image cache kind of message pictures.
pub(super) const PICTURES: &str = "message-parts";

/// Pictures a view showed without their file, to read off the GUI thread.
#[derive(Default)]
pub(super) struct Wanted {
    pub(super) photos: Vec<i64>,
    pub(super) pictures: Vec<(String, MessagePartRef)>,
}

fn part_state_name(state: MessagePartState) -> &'static str {
    match state {
        MessagePartState::Image => "image",
        MessagePartState::Playable => "playable",
        MessagePartState::Openable => "openable",
        MessagePartState::Downloadable => "downloadable",
        MessagePartState::Unsupported => "unsupported",
    }
}

fn picture_key(part: &MessagePart) -> String {
    format!("{}-{}", part_key(part.part), part.size)
}

impl State {
    /// A contact's photo when it is on disk; otherwise it is wanted, unless the contact has none.
    fn photo(&self, contact_id: Option<i64>, wanted: &mut Wanted) -> Option<String> {
        let id = contact_id?;
        if let Some(url) = images::cached(PHOTOS, &id.to_string(), || None) {
            return Some(url);
        }
        if !self.without_photo.contains(&id) {
            wanted.photos.push(id);
        }
        None
    }

    /// An image part's picture when it is on disk; otherwise it is wanted.
    fn picture(&self, part: &MessagePart, wanted: &mut Wanted) -> Option<String> {
        if part.state != MessagePartState::Image || !part.has_bytes {
            return None;
        }
        let key = picture_key(part);
        let url = images::cached(PICTURES, &key, || None);
        if url.is_none() && !self.pictures_missing.contains(&key) {
            wanted.pictures.push((key, part.part));
        }
        url
    }

    fn conversation_row(&self, c: &Conversation, wanted: &mut Wanted) -> Value {
        let single = (c.recipients.len() == 1).then(|| &c.recipients[0]);
        json!({
            "key": ComposerTarget::Conversation(c.thread_id).key(),
            "threadId": c.thread_id,
            "title": conversation_title(c),
            "preview": conversation_preview(c),
            "latestMs": c.latest_ms,
            "unread": c.unread > 0,
            "muted": c.muted,
            "hidden": c.hidden,
            "group": c.recipients.len() > 1,
            "name": single.map(contact_name),
            "photo": single.and_then(|r| self.photo(r.photo_contact_id, wanted)),
        })
    }

    pub(super) fn list_json(&self, wanted: &mut Wanted) -> Value {
        let query = self.search.trim();
        let new_message = self.shows_new_message_row().then(|| {
            let text = self.drafts.get(&ComposerTarget::NewMessage).map_or("", |d| d.text.as_str());
            json!({
                "key": ComposerTarget::NewMessage.key(),
                "preview": new_message_preview(&self.new_recipients, text),
                "hasContent": self.new_message_has_content(),
            })
        });
        let unsent: Vec<Value> = self
            .unsent
            .iter()
            .filter(|g| self.conversation_matching(&g.addresses()).is_none() && unsent_matches(g, query))
            .map(|g| {
                let single = (g.recipients.len() == 1).then(|| &g.recipients[0]);
                json!({
                    "key": ComposerTarget::Recipients(g.addresses()).key(),
                    "addresses": g.addresses(),
                    "title": unsent_title(g),
                    "preview": unsent_preview(g),
                    "latestMs": g.latest().map(|m| m.timestamp_ms),
                    "group": g.recipients.len() > 1,
                    "name": single.map(contact_name),
                    "photo": single.and_then(|r| self.photo(r.photo_contact_id, wanted)),
                })
            })
            .collect();

        let mut listed = Vec::new();
        let mut hidden = Vec::new();
        for c in self.conversations.iter().filter(|c| conversation_matches(c, query)) {
            let row = self.conversation_row(c, wanted);
            if c.hidden { hidden.push(row) } else { listed.push(row) }
        }

        json!({
            "status": self.list_status(),
            "gated": self.gate().is_some(),
            "requestingPermission": self.requesting_permission,
            "searching": !query.is_empty(),
            "newMessage": new_message,
            "unsent": unsent,
            "conversations": listed,
            "hidden": hidden,
            "selection": self.composer_target().map(|t| t.key()),
        })
    }

    pub(super) fn detail_json(&self, wanted: &mut Wanted) -> Value {
        let kind = match self.detail {
            None => Value::Null,
            Some(Detail::Conversation(_)) => "conversation".into(),
            Some(Detail::NewMessage) => "newMessage".into(),
            Some(Detail::Unsent(_)) => "unsent".into(),
        };
        let header = self.selected_conversation().map(|c| {
            let single = (c.recipients.len() == 1).then(|| &c.recipients[0]);
            let number = single.filter(|_| !c.is_group).map(|r| r.address.as_str()).filter(|a| !a.contains('@'));
            json!({
                "title": conversation_title(c),
                "subtitle": conversation_subtitle(c),
                "group": c.recipients.len() > 1,
                "name": single.map(contact_name),
                "photo": single.and_then(|r| self.photo(r.photo_contact_id, wanted)),
                "callNumber": number,
            })
        });
        let recipients: Vec<Value> =
            self.recipients().iter().map(|r| json!({ "address": r.address, "title": r.title() })).collect();
        let suggestions: Vec<Value> = self
            .suggestions
            .iter()
            .map(|s| {
                let photo = if s.has_thumbnail { self.photo(Some(s.contact_id), wanted) } else { None };
                json!({ "name": s.name, "line": number_line(s), "photo": photo })
            })
            .collect();
        let composer = self.composer();
        // Why a phone that sends only over Bluetooth cannot send now, above any composer.
        let proximity = self.state.as_ref().and_then(|s| s.proximity);
        let bluetooth = self.composer_target().and(bluetooth_notice(proximity));
        let conversation = self.selected_conversation();
        let shows_sims = conversation.is_some() && self.subscriptions.len() > 1;
        let active = conversation.and_then(|c| self.subscriptions.iter().find(|s| s.id == c.active_subscription));
        let sims: Vec<Value> = if shows_sims {
            self.subscriptions
                .iter()
                .map(|s| json!({ "id": s.id, "name": sim_name(s), "active": active.is_some_and(|a| a.id == s.id) }))
                .collect()
        } else {
            Vec::new()
        };

        json!({
            "kind": kind,
            "key": self.composer_target().map(|t| t.key()),
            "gated": self.gate().is_some(),
            "header": header,
            "recipients": recipients,
            "recipientsLocked": matches!(self.detail, Some(Detail::Unsent(_))),
            "canAddRecipient": self.can_add_recipient(),
            "recipientQuery": self.recipient_query,
            "contactsNotice": (self.detail == Some(Detail::NewMessage)).then(|| self.contacts_notice()).flatten(),
            "requestingContacts": self.requesting_contacts,
            "suggestions": suggestions,
            "transcriptFailed": self.transcript_failed,
            "composer": {
                "enabled": composer.enabled,
                "placeholder": composer.placeholder,
                "canAttach": composer.can_attach,
                "canAddImages": self.can_add_images(),
                "maxTextLength": composer.max_text_length,
            },
            "bluetoothNotice": bluetooth,
            "sims": sims,
            "simName": active.map_or_else(|| translate("SIM").to_owned(), sim_name),
            "newMessageHasContent": self.new_message_has_content(),
        })
    }

    pub(super) fn draft_json(&self) -> Value {
        let draft = self.draft();
        let images: Vec<Value> = draft
            .map(|d| d.images.iter().map(|i| json!({ "id": i.id, "name": i.name, "url": i.url })).collect())
            .unwrap_or_default();
        json!({
            "key": self.composer_target().map(|t| t.key()),
            "text": draft.map_or("", |d| d.text.as_str()),
            "images": images,
        })
    }

    fn part_json(&self, part: &MessagePart, wanted: &mut Wanted) -> Value {
        let content_type = bare_content_type(&part.content_type);
        let database = file_types().database();
        json!({
            "key": part_key(part.part),
            "state": part_state_name(part.state),
            "kind": attachment_kind(database.family_of(&content_type)),
            "size": part.size,
            "url": self.picture(part, wanted),
            "downloading": self.downloading(part.part),
            "hasBytes": part.has_bytes,
            "fileName": part_file_name(&part.name, database.extension_of(&content_type).as_deref()),
        })
    }

    pub(super) fn transcript_json(&self, wanted: &mut Wanted) -> Value {
        let (messages, is_group) = self.shown_messages();
        let rows: Vec<Value> = transcript_rows(&messages, is_group)
            .iter()
            .map(|row: &TranscriptRow<'_>| {
                let parts = row.message.parts.iter().map(|p| self.part_json(p, wanted)).collect();
                let retrying = row.message.kind == MessageType::PendingSend && self.retrying.contains(&row.message.id);
                transcript_row_json(row, parts, retrying)
            })
            .collect();
        Value::Array(rows)
    }
}

/// The Settings window's Messages pane: this computer's messaging settings and the phone's contacts.
pub(super) fn settings_json(engine: &Arc<Engine>, state: &State) -> Value {
    let messaging = engine.clone().messaging().state().ok();
    let contacts = engine.clone().contacts().state().ok();
    let access = contacts.as_ref().map(|c| match c.access {
        ContactsAccess::Granted => "granted",
        ContactsAccess::PcConsentRequired => "pcConsentRequired",
        ContactsAccess::DeviceConsentRequired => "deviceConsentRequired",
        ContactsAccess::DeviceConsentDenied => "deviceConsentDenied",
    });
    json!({
        "available": messaging.is_some(),
        "showMessages": messaging.as_ref().is_some_and(|s| s.settings.pc_consent),
        "notify": messaging.as_ref().is_some_and(|s| s.settings.message_notifications),
        "autoDownload": messaging.as_ref().is_some_and(|s| s.settings.auto_download_mms),
        "contactsCapable": contacts.as_ref().map(|c| c.capable),
        "useContacts": contacts.as_ref().is_some_and(|c| c.pc_consent),
        "contactsAccess": access,
        "requestingContacts": state.settings_requesting_contacts,
    })
}

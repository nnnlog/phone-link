//! What the composer allows for its target.
//!
//! Whether it is on, its placeholder, attachments, the text limit and the new message's recipients.

use phonelink_engine::api::contacts::ContactsAccess;

use super::draft::{Draft, MAX_IMAGES};
use super::state::State;
use crate::l10n::translate;
use crate::views::messages::{ComposerTarget, Detail, cap_notice};

/// What the composer allows for its target.
pub(super) struct Composer {
    pub(super) enabled: bool,
    pub(super) placeholder: String,
    pub(super) can_attach: bool,
    /// The carrier's text limit in UTF-16 code units; `None` is unlimited.
    pub(super) max_text_length: Option<usize>,
}

impl State {
    /// How many recipients a new message may have; `None` is unlimited.
    ///
    /// One when the default SIM sends no group messages, else its limit.
    pub(super) fn recipient_cap(&self) -> Option<usize> {
        let limits = self.limits.as_ref()?;
        if !limits.group_allowed {
            return Some(1);
        }
        limits.max_recipients.and_then(|max| usize::try_from(max).ok())
    }

    pub(super) fn can_add_recipient(&self) -> bool {
        self.detail == Some(Detail::NewMessage)
            && self.recipient_cap().is_none_or(|cap| self.new_recipients.len() < cap)
    }

    /// The new message holds recipients, typed text or a draft; Escape does not close it then.
    pub(super) fn new_message_has_content(&self) -> bool {
        !self.new_recipients.is_empty()
            || !self.recipient_query.trim().is_empty()
            || self.drafts.get(&ComposerTarget::NewMessage).is_some_and(|draft| !draft.is_empty())
    }

    pub(super) fn shows_new_message_row(&self) -> bool {
        self.detail == Some(Detail::NewMessage) || self.new_message_has_content()
    }

    pub(super) fn composer(&self) -> Composer {
        let text_message = || translate("Text Message").to_owned();
        let off =
            |placeholder: String| Composer { enabled: false, placeholder, can_attach: false, max_text_length: None };
        let limit = |max: Option<i32>| max.and_then(|max| usize::try_from(max).ok());
        match &self.detail {
            None => off(text_message()),
            Some(Detail::NewMessage | Detail::Unsent(_)) => {
                if let Some(cap) = self.recipient_cap().filter(|cap| self.recipients().len() > *cap) {
                    return off(cap_notice(cap));
                }
                // A new conversation sends from the default SIM.
                let can_attach = self
                    .limits
                    .as_ref()
                    .map(|l| l.attachments_allowed)
                    .or_else(|| self.state.as_ref().map(|s| s.settings.send_mms))
                    .unwrap_or(true);
                Composer {
                    enabled: true,
                    placeholder: text_message(),
                    can_attach,
                    max_text_length: limit(self.limits.as_ref().and_then(|l| l.max_text_length)),
                }
            }
            Some(Detail::Conversation(thread_id)) => match self.conversation(*thread_id) {
                None => off(text_message()),
                Some(c) if !c.can_reply => off(translate("You can't reply to this conversation").to_owned()),
                Some(c) => Composer {
                    enabled: true,
                    placeholder: text_message(),
                    can_attach: c.attachments_allowed,
                    max_text_length: limit(c.max_text_length),
                },
            },
        }
    }

    /// "+": images are offered here and the draft has room for one more.
    pub(super) fn can_add_images(&self) -> bool {
        let composer = self.composer();
        composer.enabled && composer.can_attach && self.draft().map_or(0, |d| d.images.len()) < MAX_IMAGES
    }

    /// Return sends: the draft has something to send and somewhere to send it.
    pub(super) fn can_submit(&self) -> bool {
        if !self.composer().enabled || !self.draft().is_some_and(Draft::has_content) {
            return false;
        }
        match self.detail {
            Some(Detail::Conversation(_) | Detail::Unsent(_)) => true,
            Some(Detail::NewMessage) => !self.new_recipients.is_empty() || !self.recipient_query.trim().is_empty(),
            None => false,
        }
    }

    /// How the new message offers contacts it cannot search yet.
    pub(super) fn contacts_notice(&self) -> Option<&'static str> {
        let contacts = self.contacts.as_ref().filter(|c| c.capable)?;
        match contacts.access {
            ContactsAccess::Granted => None,
            ContactsAccess::PcConsentRequired => Some("turnOn"),
            ContactsAccess::DeviceConsentRequired | ContactsAccess::DeviceConsentDenied => Some("allowOnPhone"),
        }
    }
}

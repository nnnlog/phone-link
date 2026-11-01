//! `MessagingCapabilities` and the gating predicates derived from it, including the value used
//! before the phone has reported any.

use super::consts::{MIN_CONTRACT_VERSION, caps};

/// The `MessagingCapabilities` a phone that has reported none uses for the rest of the app run.
///
/// The connect response and every inbound `settings` object store a value. **Before the phone has
/// reported any** (nothing stored) the value is `None` (`0`) from pairing until the end of that app
/// run, and from the next app start `RcsReceive` (`0x2`) iff the phone's contract version is
/// `≥ 3.9`. The engine derives it when it first opens the phone's messaging store in an app run:
/// `created_now` (the store did not exist: the pairing run) gives `0`; otherwise `contract_version`
/// is the phone's contract version at that moment.
/// The value is fixed for the run: a contract version the phone reports later does not change it.
pub fn default_at_load(created_now: bool, contract_version: f64) -> u32 {
    if !created_now && contract_version >= MIN_CONTRACT_VERSION { caps::RCS_RECEIVE } else { 0 }
}

/// The `MessagingCapabilities` in force: the stored value, else the run's default.
pub fn effective(stored: Option<u32>, run_default: u32) -> u32 {
    stored.unwrap_or(run_default)
}

/// The gating predicates for one capability value and contract version.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Predicates {
    pub capabilities: u32,
    pub contract_version: f64,
}

impl Predicates {
    pub fn new(capabilities: u32, contract_version: f64) -> Self {
        Self { capabilities, contract_version }
    }

    fn has(&self, bit: u32) -> bool {
        self.capabilities & bit != 0
    }

    fn contract_ok(&self) -> bool {
        self.contract_version >= MIN_CONTRACT_VERSION
    }

    /// **RCS receive** = `RcsReceive` ∧ contract ≥ 3.9.
    pub fn rcs_receive(&self) -> bool {
        self.has(caps::RCS_RECEIVE) && self.contract_ok()
    }

    /// **RCS send** = RCS receive ∧ `RcsSend` ∧ contract ≥ 3.9.
    pub fn rcs_send(&self) -> bool {
        self.rcs_receive() && self.has(caps::RCS_SEND) && self.contract_ok()
    }

    /// **Mark conversation as read** = `MarkConversationAsRead` ∧ contract ≥ 3.9.
    pub fn mark_conversation_as_read(&self) -> bool {
        self.has(caps::MARK_CONVERSATION_AS_READ) && self.contract_ok()
    }

    /// `Participants`: participant tracking, forced resync, Google-RCS recipients.
    pub fn participants(&self) -> bool {
        self.has(caps::PARTICIPANTS)
    }

    /// `Send`: echoed as `configuration.MessagingSend` only.
    pub fn send(&self) -> bool {
        self.has(caps::SEND)
    }

    /// `SendRequiresProximity`: the Bluetooth Classic gate.
    pub fn requires_proximity(&self) -> bool {
        self.has(caps::SEND_REQUIRES_PROXIMITY)
    }

    /// `MmsAutoDownloadOnDevice`: the phone has an "auto-download MMS" setting (settings UI only).
    pub fn mms_auto_download_on_device(&self) -> bool {
        self.has(caps::MMS_AUTO_DOWNLOAD_ON_DEVICE)
    }
}

/// An inbound `settings` object of an item message, already type-checked.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct PhoneSettings {
    /// `capabilities` (absent ⇒ 0).
    pub capabilities: i32,
    pub mms_auto_download_available: bool,
    pub rcs_send_available: bool,
    pub mark_as_read_available: bool,
    /// `mmsAutoDownloadEnabled` (absent ⇒ `false`): "MMS auto-download on phone".
    pub mms_auto_download_enabled: bool,
}

impl PhoneSettings {
    /// The `MessagingCapabilities` the object stores: `capabilities | 0x2 | (0x1 if
    /// mmsAutoDownloadAvailable) | (0x4 if rcsSendAvailable) | (0x8 if markAsReadAvailable)`.
    pub fn stored_capabilities(&self) -> u32 {
        let mut v = self.capabilities as u32 | caps::RCS_RECEIVE;

        if self.mms_auto_download_available {
            v |= caps::MMS_AUTO_DOWNLOAD_ON_DEVICE;
        }
        if self.rcs_send_available {
            v |= caps::RCS_SEND;
        }
        if self.mark_as_read_available {
            v |= caps::MARK_CONVERSATION_AS_READ;
        }

        v
    }
}

/// `capabilities.messaging` of the connect response: stored as `value | 0x2`.
pub fn from_connect(value: i32) -> u32 {
    value as u32 | caps::RCS_RECEIVE
}

#[cfg(test)]
mod tests;

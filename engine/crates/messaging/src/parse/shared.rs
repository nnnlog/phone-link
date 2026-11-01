//! The parts every exchange shares: settings, anchors, acks, correlation.

use super::{MISTYPED, Parsed, object, opt, req};
use crate::caps::PhoneSettings;
use crate::consts::{anchor, key};
use phonelink_app::Malformed;
use phonelink_core::codec::valueset::{Value, ValueSet};

/// The message's `correlationVector` (the dispatcher copied the header in when the payload lacked
/// it); none ⇒ malformed.
pub fn correlation_vector(payload: &ValueSet) -> Parsed<String> {
    match payload.get(key::CORRELATION_VECTOR) {
        Some(Value::String(s)) => Ok(s.clone()),
        Some(_) => Err(Malformed::new(MISTYPED, key::CORRELATION_VECTOR)),
        None => Err(Malformed::new("messaging message without a correlation id", key::CORRELATION_VECTOR)),
    }
}

/// An item message's `settings`, when the payload has it as an Object.
pub fn settings(payload: &ValueSet) -> Parsed<Option<PhoneSettings>> {
    let Some(s) = object(payload, key::SETTINGS) else { return Ok(None) };
    Ok(Some(PhoneSettings {
        capabilities: opt::<i32>(s, key::SETTINGS_CAPABILITIES)?.unwrap_or(0),
        mms_auto_download_available: opt::<bool>(s, key::MMS_AUTO_DOWNLOAD_AVAILABLE)?.unwrap_or(false),
        rcs_send_available: opt::<bool>(s, key::RCS_SEND_AVAILABLE)?.unwrap_or(false),
        mark_as_read_available: opt::<bool>(s, key::MARK_AS_READ_AVAILABLE)?.unwrap_or(false),
        mms_auto_download_enabled: opt::<bool>(s, key::MMS_AUTO_DOWNLOAD_ENABLED)?.unwrap_or(false),
    }))
}

/// One received `sequenceNumbers` object.
///
/// It replaces the exchange's anchor set — a type absent from it becomes "none" (`-1`); the three
/// RCS types are left untouched (`rcs = None`) when RCS receive is off.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AnchorUpdate {
    /// `sms`, `mms`, `conversation`, `subscription`.
    pub base: [i64; 4],
    /// `rcs_chat`, `rcs_filetransfer`, `rcs_conversation`.
    pub rcs: Option<[i64; 3]>,
}

pub fn sequence_numbers(payload: &ValueSet, rcs_receive: bool) -> Parsed<Option<AnchorUpdate>> {
    let Some(s) = object(payload, key::SEQUENCE_NUMBERS) else { return Ok(None) };
    let get = |k: &'static str| -> Parsed<i64> { Ok(opt::<i64>(s, k)?.unwrap_or(-1)) };
    let base = [get(anchor::SMS)?, get(anchor::MMS)?, get(anchor::CONVERSATION)?, get(anchor::SUBSCRIPTION)?];
    let rcs = if rcs_receive {
        Some([get(anchor::RCS_CHAT)?, get(anchor::RCS_FILE_TRANSFER)?, get(anchor::RCS_CONVERSATION)?])
    } else {
        None
    };
    Ok(Some(AnchorUpdate { base, rcs }))
}

/// `acks.ids`: the pending sends the phone has finished.
pub fn acks(payload: &ValueSet) -> Parsed<Option<Vec<i64>>> {
    let Some(a) = object(payload, key::ACKS) else { return Ok(None) };
    Ok(Some(req::<&[i64]>(a, key::IDS)?.to_vec()))
}

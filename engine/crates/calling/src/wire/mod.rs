//! The call-history messages: the phone's `callLogs` content on `/legacy/phonecontent` (a metadata
//! message, then item messages), the PC's answer to the metadata, and the PC's `/calling` request.
//!
//! A parse validates the whole message before anything is applied; a message it refuses is answered
//! `{ result: Int32 1 }` and changes nothing.

use phonelink_app::consts::key as app_key;
use phonelink_app::{AppRequest, Malformed, RequestOptions};
use phonelink_core::codec::valueset::{Value, ValueSet};
use phonelink_core::types::TraceContext;

/// The content type of every call-history message, inbound and outbound.
pub const CONTENT_TYPE: &str = "callLogs";

/// The route of the PC's call-history request.
pub const HISTORY_ROUTE: &str = "/calling";

/// A `SyncAction` of `actions`: the entry is deleted.
pub const ACTION_DELETE: i32 = 2;

pub mod key {
    pub const IDS: &str = "callLogsIds";
    pub const LAST_UPDATED_TIMES: &str = "callLogsLastUpdatedTimes";
    pub const PHONE_NUMBERS: &str = "callLogsPhoneNumbers";
    pub const DURATION: &str = "callLogsDuration";
    pub const TYPE: &str = "callLogsType";
    pub const DATE: &str = "callLogsDate";
    pub const READ: &str = "callLogsRead";
    pub const PHONE_ACCOUNT_IDS: &str = "callLogsPhoneAccountIds";
    pub const ACTIONS: &str = "actions";
    pub const ITEM_COUNT: &str = "itemCount";
    pub const SEQUENCE_NUMBER: &str = "sequenceNumber";
    pub const RELATED_CORRELATION_IDS: &str = "relatedCorrelationIds";
    pub const SUBCONTENT_TYPES: &str = "subcontentTypes";

    // The connect message.
    /// `configuration` entry: call-log sync is on for this PC.
    pub const CONFIG_CALL_LOGS: &str = "CallLogs";
    /// `sequenceNumbers` entry and reply key of the call log.
    pub const SEQ_CALL_LOGS: &str = "call_logs";
    pub const PERMISSIONS: &str = "permissions";
    pub const PERMISSIONS_CALLS: &str = "calls";
    pub const PERMISSIONS_CALL_LOGS: &str = "call_logs";
}

/// A call's type: Android's call-log type, which the phone sends unmapped.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CallType {
    Unknown,
    Incoming,
    Outgoing,
    Missed,
    Voicemail,
    Rejected,
    Blocked,
    AnsweredExternally,
    /// A value outside `0..=7`, kept as received.
    Other(i32),
}

impl CallType {
    pub fn from_i32(value: i32) -> Self {
        match value {
            0 => CallType::Unknown,
            1 => CallType::Incoming,
            2 => CallType::Outgoing,
            3 => CallType::Missed,
            4 => CallType::Voicemail,
            5 => CallType::Rejected,
            6 => CallType::Blocked,
            7 => CallType::AnsweredExternally,
            other => CallType::Other(other),
        }
    }
}

/// The value of a missed call, which the calls badge counts.
pub const TYPE_MISSED: i32 = 3;

/// A metadata message (it has a `metadata` key).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Metadata {
    /// Its `correlationVector` when that is a `String`; echoed in the answer.
    pub correlation_vector: Option<String>,
    /// Every call-log id the phone has.
    pub ids: Vec<i64>,
    /// `last_updated[i]` belongs to `ids[i]`, Unix ms (a longer array is cut to the ids).
    pub last_updated: Vec<i64>,
    pub sequence: Option<i64>,
}

/// Parses a metadata message.
pub fn parse_metadata(payload: &ValueSet) -> Result<Metadata, Malformed> {
    let ids = match payload.get(key::IDS) {
        Some(Value::Int64Array(ids)) => ids,
        _ => return Err(Malformed::new("callLogsIds missing or not an Int64Array", key::IDS)),
    };

    let times = match payload.get(key::LAST_UPDATED_TIMES) {
        Some(Value::Int64Array(times)) if times.len() >= ids.len() => times,
        _ => {
            return Err(Malformed::new(
                "callLogsLastUpdatedTimes missing, not an Int64Array or shorter than callLogsIds",
                key::LAST_UPDATED_TIMES,
            ));
        }
    };

    Ok(Metadata {
        correlation_vector: payload.str(app_key::CORRELATION_VECTOR).map(str::to_string),
        ids: ids.clone(),
        last_updated: times[..ids.len()].to_vec(),
        // A sequence number of another type is not kept.
        sequence: payload.i64(key::SEQUENCE_NUMBER),
    })
}

/// One call of an item message.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CallEntry {
    pub id: i64,
    /// The other party's number as the phone stores it; empty for a delete, or when the phone has
    /// no number for the call.
    pub number: String,
    /// Seconds.
    pub duration: i64,
    /// [`CallType`]'s raw value.
    pub call_type: i32,
    /// Start time, Unix ms.
    pub date: i64,
    pub read: bool,
    /// The SIM subscription the call used; empty when it has none.
    pub phone_account_id: String,
    /// Unix ms.
    pub last_updated: i64,
}

/// An item message: the entries to store and the ids to delete.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Items {
    pub upserts: Vec<CallEntry>,
    pub deletes: Vec<i64>,
    pub sequence: Option<i64>,
}

impl Items {
    /// Every id the message carries, stored or deleted.
    pub fn ids(&self) -> impl Iterator<Item = i64> + '_ {
        self.upserts.iter().map(|e| e.id).chain(self.deletes.iter().copied())
    }
}

/// A required array of an item message, or the refusal naming it.
fn required<'v, T>(
    payload: &'v ValueSet,
    name: &'static str,
    read: impl Fn(&'v ValueSet, &str) -> Option<&'v [T]>,
    reason: &'static str,
) -> Result<&'v [T], Malformed> {
    read(payload, name).ok_or(Malformed::new(reason, name))
}

/// Parses an item message (no `metadata` key).
///
/// The entries processed are as many as the shortest array, and as `actions` when it is present, else
/// as `itemCount`; fewer than `itemCount` refuses the message.
pub fn parse_items(payload: &ValueSet) -> Result<Items, Malformed> {
    let Some(item_count) = payload.i32(key::ITEM_COUNT) else {
        return Err(Malformed::new("itemCount missing or not an Int32", key::ITEM_COUNT));
    };

    let ids = required(payload, key::IDS, ValueSet::i64_array, "callLogsIds missing or not an Int64Array")?;
    let numbers = required(
        payload,
        key::PHONE_NUMBERS,
        ValueSet::string_array,
        "callLogsPhoneNumbers missing or not a StringArray",
    )?;
    let durations =
        required(payload, key::DURATION, ValueSet::i64_array, "callLogsDuration missing or not an Int64Array")?;
    let types = required(payload, key::TYPE, ValueSet::i32_array, "callLogsType missing or not an Int32Array")?;
    let dates = required(payload, key::DATE, ValueSet::i64_array, "callLogsDate missing or not an Int64Array")?;
    let reads = required(payload, key::READ, ValueSet::bool_array, "callLogsRead missing or not a BoolArray")?;
    let accounts = required(
        payload,
        key::PHONE_ACCOUNT_IDS,
        ValueSet::string_array,
        "callLogsPhoneAccountIds missing or not a StringArray",
    )?;
    let times = required(
        payload,
        key::LAST_UPDATED_TIMES,
        ValueSet::i64_array,
        "callLogsLastUpdatedTimes missing or not an Int64Array",
    )?;

    // An `actions` of another type counts as absent.
    let actions = payload.i32_array(key::ACTIONS);
    let bound = actions.map_or(usize::try_from(item_count).unwrap_or(0), <[i32]>::len);
    let count = [ids.len(), numbers.len(), durations.len(), types.len(), dates.len(), reads.len(), accounts.len()]
        .into_iter()
        .chain([times.len(), bound])
        .min()
        .unwrap_or(0);
    if i64::try_from(count).unwrap_or(i64::MAX) < i64::from(item_count) {
        return Err(Malformed::new("fewer call-log entries than itemCount", key::ITEM_COUNT));
    }

    let mut items = Items { upserts: Vec::new(), deletes: Vec::new(), sequence: payload.i64(key::SEQUENCE_NUMBER) };
    for i in 0..count {
        if actions.is_some_and(|a| a[i] == ACTION_DELETE) {
            items.deletes.push(ids[i]);
            continue;
        }

        items.upserts.push(CallEntry {
            id: ids[i],
            number: numbers[i].clone(),
            duration: durations[i],
            call_type: types[i],
            date: dates[i],
            read: reads[i],
            phone_account_id: accounts[i].clone(),
            last_updated: times[i],
        });
    }
    Ok(items)
}

/// The answer to a metadata message: the ids the PC wants.
pub fn metadata_reply(correlation_vector: Option<&str>, contract_version: f64, wanted: &[i64]) -> ValueSet {
    // A `correlationVector` that was not a `String` is answered as `Empty`.
    let correlation_vector = correlation_vector.map_or(Value::Empty, |cv| Value::String(cv.to_string()));
    ValueSet::new()
        .with(app_key::CORRELATION_VECTOR, correlation_vector)
        .with(app_key::CONTENT_TYPE, CONTENT_TYPE)
        .with(app_key::CONTRACT_VERSION, contract_version)
        .with(key::ITEM_COUNT, i32::try_from(wanted.len()).unwrap_or(i32::MAX))
        .with(key::IDS, Value::Int64Array(wanted.to_vec()))
        .with(app_key::RESULT, 0i32)
}

/// The `/calling` legacy request: `correlationVector` = the request's trace correlation id, with
/// `relatedCorrelationIds` and `subcontentTypes`; no headers.
///
/// The shared layer adds the legacy keys (`isuserinitiated` from `options`) and the standard keys.
pub fn history_request(trace: TraceContext, options: RequestOptions) -> AppRequest {
    let correlation_vector = trace.correlation_id.clone();
    let payload = ValueSet::new()
        .with(app_key::CORRELATION_VECTOR, correlation_vector.clone())
        .with(app_key::CONTENT_TYPE, CONTENT_TYPE)
        .with(key::RELATED_CORRELATION_IDS, Value::StringArray(vec![correlation_vector]))
        .with(key::SUBCONTENT_TYPES, Value::StringArray(vec![CONTENT_TYPE.to_string()]));
    AppRequest::legacy(HISTORY_ROUTE, payload).with_options(options).with_trace(trace)
}

#[cfg(test)]
mod tests;

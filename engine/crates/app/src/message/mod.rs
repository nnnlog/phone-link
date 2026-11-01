//! Reading an App message, inbound request or response alike.
//!
//! It covers gzip inflation, `pbvalueset` parse, and the lift of the six App-layer keys from the
//! payload when the envelope lacks them (App `ValueSet`s may carry those keys inside the payload
//! rather than as envelope headers).

use phonelink_core::codec::valueset::{self, Value, ValueSet, ValueSetError};
use phonelink_core::types::{HeaderMap, TransportMessage, header};

use super::consts::{key, result_code};

/// An App message after [`read_app_message`]: `headers` are the envelope headers with the lifted
/// keys added (a header, when present, wins over the payload key); `values` is the parsed payload.
#[derive(Debug, Clone, PartialEq)]
pub struct AppMessage {
    pub headers: HeaderMap,
    pub values: ValueSet,
}

/// The contract version an App message carries, after the lift.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum ContractVersion {
    /// Neither a header nor a `Double` payload key.
    Missing,
    Number(f64),
    /// A header value that does not parse as a number (treated as missing in the inbound
    /// contract-version check).
    NotANumber,
}

impl ContractVersion {
    /// The value the dispatch rules compare: a missing or non-numeric version reads `0`.
    pub fn value_or_zero(self) -> f64 {
        match self {
            ContractVersion::Number(v) => v,
            ContractVersion::Missing | ContractVersion::NotANumber => 0.0,
        }
    }
}

/// `_isGzippedCompressed` equals `true`, case-insensitively.
pub fn is_gzipped(headers: &HeaderMap) -> bool {
    headers.get(header::IS_GZIPPED_COMPRESSED).is_some_and(|v| v.eq_ignore_ascii_case("true"))
}

/// Inflates (when flagged), parses and lifts. A failure means the payload is not a `ValueSet`.
pub fn read_app_message(message: &TransportMessage) -> Result<AppMessage, ValueSetError> {
    let values = if is_gzipped(&message.headers) {
        valueset::decode(&valueset::gunzip(&message.payload)?)?
    } else {
        valueset::decode(&message.payload)?
    };

    let mut headers = message.headers.clone();
    lift(&mut headers, &values);
    Ok(AppMessage { headers, values })
}

/// Adds `result` (from an `Int32`), `contractVersion` (from a `Double`) and `_requestId`, `_route`,
/// `correlationVector`, `contentType` (from `String`s) to `headers` when absent there.
pub fn lift(headers: &mut HeaderMap, values: &ValueSet) {
    for name in [key::REQUEST_ID, key::ROUTE, key::CORRELATION_VECTOR, key::CONTENT_TYPE] {
        if headers.contains(name) {
            continue;
        }
        if let Some(Value::String(s)) = values.get(name) {
            headers.insert(name, s.clone());
        }
    }

    if !headers.contains(key::RESULT) {
        if let Some(Value::Int32(r)) = values.get(key::RESULT) {
            headers.insert(key::RESULT, r.to_string());
        }
    }

    if !headers.contains(key::CONTRACT_VERSION) {
        if let Some(Value::Double(v)) = values.get(key::CONTRACT_VERSION) {
            // `f64`'s `Display` is the shortest text that parses back to the same value.
            headers.insert(key::CONTRACT_VERSION, v.to_string());
        }
    }
}

impl AppMessage {
    pub fn route(&self) -> Option<&str> {
        self.headers.get(key::ROUTE)
    }
    pub fn request_id(&self) -> Option<&str> {
        self.headers.get(key::REQUEST_ID)
    }
    pub fn content_type(&self) -> Option<&str> {
        self.headers.get(key::CONTENT_TYPE)
    }
    pub fn correlation_vector(&self) -> Option<&str> {
        self.headers.get(key::CORRELATION_VECTOR)
    }

    /// `result` as an integer; missing or non-integer reads `-1`.
    pub fn result(&self) -> i32 {
        self.headers.get(key::RESULT).and_then(|s| s.trim().parse::<i32>().ok()).unwrap_or(result_code::MISSING)
    }

    pub fn contract_version(&self) -> ContractVersion {
        match self.headers.get(key::CONTRACT_VERSION) {
            None => ContractVersion::Missing,
            Some(s) => match parse_number(s) {
                Some(v) => ContractVersion::Number(v),
                None => ContractVersion::NotANumber,
            },
        }
    }
}

/// A finite decimal number, surrounding whitespace allowed.
fn parse_number(s: &str) -> Option<f64> {
    s.trim().parse::<f64>().ok().filter(|v| v.is_finite())
}

#[cfg(test)]
mod tests;

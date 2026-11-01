//! The device list's `customData`.

use tracing::warn;

/// The `customData` JSON object; absent, empty or not an object → empty (logged when unparsable).
pub(super) fn parse_custom_data(raw: Option<&str>) -> serde_json::Map<String, serde_json::Value> {
    let Some(raw) = raw.filter(|s| !s.trim().is_empty()) else {
        return serde_json::Map::new();
    };
    match serde_json::from_str::<serde_json::Value>(raw) {
        Ok(serde_json::Value::Object(m)) => m,
        _ => {
            warn!(len = raw.len(), "device-list customData is not a JSON object; ignored");
            serde_json::Map::new()
        }
    }
}

/// An unsigned decimal integer: a JSON number, or a string of one.
pub(super) fn json_u32(v: &serde_json::Value) -> Option<u32> {
    match v {
        serde_json::Value::Number(n) => n.as_u64().and_then(|x| u32::try_from(x).ok()),
        serde_json::Value::String(s) => s.trim().parse::<u32>().ok(),
        _ => None,
    }
}

pub(super) fn json_i32(v: &serde_json::Value) -> Option<i32> {
    match v {
        serde_json::Value::Number(n) => n.as_i64().and_then(|x| i32::try_from(x).ok()),
        serde_json::Value::String(s) => s.trim().parse::<i32>().ok(),
        _ => None,
    }
}

pub(super) fn json_string(v: &serde_json::Value) -> Option<String> {
    match v {
        serde_json::Value::String(s) => Some(s.clone()),
        _ => None,
    }
}

/// A flags enum given as a number (JSON or text) or as comma-separated flag names
/// (case-insensitive).
pub(super) fn json_flags(v: &serde_json::Value, names: &[(&str, u32)]) -> Option<u32> {
    match v {
        serde_json::Value::Number(n) => n
            .as_u64()
            .and_then(|x| u32::try_from(x).ok())
            .or_else(|| n.as_i64().and_then(|x| i32::try_from(x).ok()).map(|x| x as u32)),
        serde_json::Value::String(s) => {
            let s = s.trim();
            if let Ok(n) = s.parse::<u32>() {
                return Some(n);
            }
            if let Ok(n) = s.parse::<i32>() {
                return Some(n as u32);
            }

            let mut bits = 0;
            for part in s.split(',') {
                let part = part.trim();
                let (_, bit) = names.iter().find(|(n, _)| n.eq_ignore_ascii_case(part))?;
                bits |= bit;
            }
            Some(bits)
        }
        _ => None,
    }
}

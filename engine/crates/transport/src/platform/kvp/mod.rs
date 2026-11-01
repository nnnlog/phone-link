//! KVP (`text/json`) bodies of platform messages: a flat UTF-8 JSON object whose values are all JSON
//! strings; the empty map is `{}` (two bytes).

use serde::de::IgnoredAny;

use phonelink_core::types::{HeaderMap, MessageType, TransportMessage, header};

/// An ordered `string → string` map with unique keys: the decoded form of a `text/json` body.
/// Inserting an existing key replaces its value in place; the writer emits keys in insertion order.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Kvp(Vec<(String, String)>);

/// Why a `text/json` body could not be read as a KVP map.
///
/// The KVP reader produces an empty map for a non-`text/json` content type and for an empty body;
/// these are the remaining cases, which each consumer handles on its own.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum KvpError {
    #[error("the body is not JSON: {0}")]
    NotJson(String),
    #[error("the body is JSON but not an object")]
    NotAnObject,
    #[error("the key {0:?} appears more than once")]
    DuplicateKey(String),
}

/// The nesting depth the JSON reader accepts; the root object is level 1.
const MAX_DEPTH: usize = 64;

impl Kvp {
    pub fn new() -> Self {
        Self(Vec::new())
    }

    /// Builder form of [`Kvp::insert`].
    pub fn with(mut self, key: impl Into<String>, value: impl Into<String>) -> Self {
        self.insert(key, value);
        self
    }

    /// Sets `key` to `value`, keeping the key's position when it already exists.
    pub fn insert(&mut self, key: impl Into<String>, value: impl Into<String>) {
        let key = key.into();
        let value = value.into();
        match self.0.iter_mut().find(|(k, _)| *k == key) {
            Some(slot) => slot.1 = value,
            None => self.0.push((key, value)),
        }
    }

    pub fn get(&self, key: &str) -> Option<&str> {
        self.0.iter().find(|(k, _)| k == key).map(|(_, v)| v.as_str())
    }

    #[cfg(feature = "test-support")]
    pub fn len(&self) -> usize {
        self.0.len()
    }

    #[cfg(feature = "test-support")]
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    pub fn iter(&self) -> impl Iterator<Item = (&str, &str)> {
        self.0.iter().map(|(k, v)| (k.as_str(), v.as_str()))
    }

    /// The writer: `{"k":"v",…}` in insertion order, no white space; the empty map is `{}`.
    pub fn to_body(&self) -> Vec<u8> {
        let mut out = String::from("{");

        for (i, (k, v)) in self.0.iter().enumerate() {
            if i > 0 {
                out.push(',');
            }
            out.push_str(&serde_json::to_string(k).expect("a string always serializes"));
            out.push(':');
            out.push_str(&serde_json::to_string(v).expect("a string always serializes"));
        }

        out.push('}');
        out.into_bytes()
    }

    /// The KVP reader rules: `content_type` present and not exactly `text/json` ⇒ empty map; empty
    /// body ⇒ empty map; otherwise parse a JSON object (nesting at most 64 levels) and turn every
    /// value into text: a string is its unescaped text, `true` / `false` are `True` / `False`,
    /// `null` is the empty string, and a number, array or object is its JSON text exactly as it
    /// appears in the body. A key that appears twice is an error.
    pub fn read(content_type: Option<&str>, body: &[u8]) -> Result<Kvp, KvpError> {
        if content_type.is_some_and(|ct| ct != header::CONTENT_TYPE_JSON) {
            return Ok(Kvp::new());
        }
        if body.is_empty() {
            return Ok(Kvp::new());
        }

        let text = std::str::from_utf8(body).map_err(|e| KvpError::NotJson(e.to_string()))?;
        // Syntax only: the values are taken from the text below, so no number is range-checked.
        serde_json::from_str::<IgnoredAny>(text).map_err(|e| KvpError::NotJson(e.to_string()))?;

        let mut kvp = Kvp::new();
        for (key, raw) in top_level_members(text)? {
            if kvp.get(&key).is_some() {
                return Err(KvpError::DuplicateKey(key));
            }
            kvp.0.push((key, element_text(raw)?));
        }

        Ok(kvp)
    }

    /// [`Kvp::read`] applied to a message's `ms-content-type` header and payload.
    pub fn from_message(message: &TransportMessage) -> Result<Kvp, KvpError> {
        Kvp::read(message.headers.get(header::MS_CONTENT_TYPE), &message.payload)
    }

    /// A Platform message with this map as its `text/json` body: headers `_route` (when given) and
    /// `ms-content-type: text/json`. Responses pass `None`: the router sets `_route = /internal/response`.
    pub fn platform_message(&self, route: Option<&str>) -> TransportMessage {
        let mut headers = HeaderMap::new();
        if let Some(route) = route {
            headers.insert(header::ROUTE, route);
        }
        headers.insert(header::MS_CONTENT_TYPE, header::CONTENT_TYPE_JSON);
        TransportMessage::new(MessageType::Platform, headers, self.to_body())
    }
}

impl<K: Into<String>, V: Into<String>> FromIterator<(K, V)> for Kvp {
    fn from_iter<I: IntoIterator<Item = (K, V)>>(iter: I) -> Self {
        let mut kvp = Kvp::new();
        for (k, v) in iter {
            kvp.insert(k, v);
        }
        kvp
    }
}

/// The text of one member value, given its JSON text.
fn element_text(raw: &str) -> Result<String, KvpError> {
    match raw.as_bytes()[0] {
        b'"' => serde_json::from_str::<String>(raw).map_err(|e| KvpError::NotJson(e.to_string())),
        b't' => Ok("True".to_owned()),
        b'f' => Ok("False".to_owned()),
        b'n' => Ok(String::new()),
        _ => Ok(raw.to_owned()),
    }
}

fn skip_ws(bytes: &[u8], mut i: usize) -> usize {
    while i < bytes.len() && matches!(bytes[i], b' ' | b'\t' | b'\n' | b'\r') {
        i += 1;
    }
    i
}

/// The end (exclusive) of the JSON string starting at `start` (its opening quote).
fn string_end(bytes: &[u8], start: usize) -> usize {
    let mut i = start + 1;
    loop {
        match bytes[i] {
            b'\\' => i += 2,
            b'"' => return i + 1,
            _ => i += 1,
        }
    }
}

/// The end (exclusive) of the JSON value starting at `start`, which is nested at `depth` (the root
/// object's members are at depth 1).
fn value_end(bytes: &[u8], start: usize, depth: usize) -> Result<usize, KvpError> {
    match bytes[start] {
        b'"' => Ok(string_end(bytes, start)),
        b'{' | b'[' => {
            let mut level = depth;
            let mut i = start;
            loop {
                match bytes[i] {
                    b'"' => {
                        i = string_end(bytes, i);
                        continue;
                    }
                    b'{' | b'[' => {
                        level += 1;
                        if level > MAX_DEPTH {
                            return Err(KvpError::NotJson(format!("nesting deeper than {MAX_DEPTH} levels")));
                        }
                    }
                    b'}' | b']' => {
                        level -= 1;
                        if level == depth {
                            return Ok(i + 1);
                        }
                    }
                    _ => {}
                }
                i += 1;
            }
        }
        _ => {
            let mut i = start;
            while i < bytes.len() && !matches!(bytes[i], b',' | b'}' | b']' | b' ' | b'\t' | b'\n' | b'\r') {
                i += 1;
            }
            Ok(i)
        }
    }
}

/// The members of the top-level object of `text`, which is valid JSON: each key decoded, each value
/// as its JSON text, in body order.
fn top_level_members(text: &str) -> Result<Vec<(String, &str)>, KvpError> {
    let bytes = text.as_bytes();
    let mut i = skip_ws(bytes, 0);
    if bytes[i] != b'{' {
        return Err(KvpError::NotAnObject);
    }

    let mut members = Vec::new();
    i = skip_ws(bytes, i + 1);
    if bytes[i] == b'}' {
        return Ok(members);
    }

    loop {
        let key_end = string_end(bytes, i);
        let key = serde_json::from_str::<String>(&text[i..key_end]).map_err(|e| KvpError::NotJson(e.to_string()))?;
        i = skip_ws(bytes, key_end) + 1; // past ':'

        let start = skip_ws(bytes, i);
        let end = value_end(bytes, start, 1)?;
        members.push((key, &text[start..end]));

        i = skip_ws(bytes, end);
        match bytes[i] {
            b',' => i = skip_ws(bytes, i + 1),
            _ => return Ok(members), // '}'
        }
    }
}

#[cfg(test)]
mod tests;

//! JSON shapes the wire needs that `serde_json`'s map does not keep: an object written in insertion order.

use serde::Serialize;
use serde::ser::SerializeMap;

/// A `string → string` JSON object written in insertion order (unique keys).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct OrderedStringMap(pub Vec<(String, String)>);

impl OrderedStringMap {
    /// Appends `key` with `value`; panics when `key` is already present.
    pub fn push(&mut self, key: impl Into<String>, value: impl Into<String>) {
        let key = key.into();
        assert!(self.get(&key).is_none(), "duplicate key {key:?} in an ordered map");
        self.0.push((key, value.into()));
    }

    /// The value of `key`, or `None` when absent.
    pub fn get(&self, key: &str) -> Option<&str> {
        self.0.iter().find(|(k, _)| k == key).map(|(_, v)| v.as_str())
    }

    /// Compact JSON object text in insertion order.
    pub fn to_json(&self) -> String {
        serde_json::to_string(self).expect("a string map always serializes")
    }
}

impl Serialize for OrderedStringMap {
    fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        let mut m = s.serialize_map(Some(self.0.len()))?;
        for (k, v) in &self.0 {
            m.serialize_entry(k, v)?;
        }
        m.end()
    }
}

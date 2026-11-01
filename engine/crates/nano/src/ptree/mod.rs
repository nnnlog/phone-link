//! The JSON dialect of the connection contract: objects whose every leaf is a string (integers and
//! enums as decimal text, booleans as `"1"`/`"0"`), read back with any JSON scalar accepted as its text.

use serde_json::{Map, Value};

/// A failure reading a contract document.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum PtreeError {
    #[error("`{0}` is missing")]
    Missing(String),
    #[error("`{0}` is not a scalar")]
    NotScalar(String),
    #[error("`{0}` is not a valid {1}: {2:?}")]
    Invalid(String, &'static str, String),
}

/// A string leaf.
pub fn leaf(value: impl ToString) -> Value {
    Value::String(value.to_string())
}

/// A boolean leaf: `"1"` or `"0"`.
pub fn bool_leaf(value: bool) -> Value {
    leaf(if value { "1" } else { "0" })
}

/// Puts `value` at the dotted `path` of `root`, creating the objects on the way.
pub fn put(root: &mut Map<String, Value>, path: &str, value: Value) {
    let mut parts = path.split('.').peekable();
    let mut node = root;
    while let Some(part) = parts.next() {
        if parts.peek().is_none() {
            node.insert(part.to_string(), value);
            return;
        }

        let child = node.entry(part.to_string()).or_insert_with(|| Value::Object(Map::new()));
        if !child.is_object() {
            *child = Value::Object(Map::new());
        }
        node = child.as_object_mut().expect("just made an object");
    }
}

/// The node at the dotted `path` of `root`.
pub fn get<'a>(root: &'a Value, path: &str) -> Option<&'a Value> {
    path.split('.').try_fold(root, |node, part| node.get(part))
}

/// The text of the scalar at `path`: a string as it is, a number or a boolean as its JSON text.
pub fn text(root: &Value, path: &str) -> Result<Option<String>, PtreeError> {
    match get(root, path) {
        None | Some(Value::Null) => Ok(None),
        Some(Value::String(s)) => Ok(Some(s.clone())),
        Some(Value::Number(n)) => Ok(Some(n.to_string())),
        Some(Value::Bool(b)) => Ok(Some(b.to_string())),
        Some(_) => Err(PtreeError::NotScalar(path.to_string())),
    }
}

/// The text at `path`, which must be present.
pub fn required_text(root: &Value, path: &str) -> Result<String, PtreeError> {
    text(root, path)?.ok_or_else(|| PtreeError::Missing(path.to_string()))
}

/// The unsigned integer at `path` (decimal text, surrounding spaces allowed).
pub fn uint(root: &Value, path: &str) -> Result<Option<u64>, PtreeError> {
    match text(root, path)? {
        None => Ok(None),
        Some(t) => {
            t.trim().parse::<u64>().map(Some).map_err(|_| PtreeError::Invalid(path.to_string(), "unsigned integer", t))
        }
    }
}

/// The unsigned integer at `path`, which must be present.
pub fn required_uint(root: &Value, path: &str) -> Result<u64, PtreeError> {
    uint(root, path)?.ok_or_else(|| PtreeError::Missing(path.to_string()))
}

/// The boolean at `path`: `"1"`/`"0"`, or a JSON boolean.
pub fn boolean(root: &Value, path: &str) -> Result<Option<bool>, PtreeError> {
    match get(root, path) {
        None | Some(Value::Null) => Ok(None),
        Some(Value::Bool(b)) => Ok(Some(*b)),
        _ => match text(root, path)?.as_deref().map(str::trim) {
            Some("1") => Ok(Some(true)),
            Some("0") => Ok(Some(false)),
            Some(other) => Err(PtreeError::Invalid(path.to_string(), "boolean", other.to_string())),
            None => Ok(None),
        },
    }
}

#[cfg(test)]
mod tests;

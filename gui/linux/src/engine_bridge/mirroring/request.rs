//! A `mirror.*` action's argument: a JSON request, or for some actions a plain value.

use serde_json::Value;

/// A `mirror.*` action's argument, and what most actions read of it.
pub(super) struct MirrorRequest<'a> {
    /// The argument as QML passed it, which some actions take whole.
    pub(super) arg: &'a str,
    /// The argument read as JSON; null when it is not JSON.
    pub(super) json: Value,
    /// The window the action is for: the request's `id`, else the whole argument.
    pub(super) id: String,
    /// The pointer's position for the input actions; 0 when the request has none.
    pub(super) x: f64,
    pub(super) y: f64,
}

impl<'a> MirrorRequest<'a> {
    pub(super) fn parse(arg: &'a str) -> Self {
        let json: Value = serde_json::from_str(arg).unwrap_or(Value::Null);
        let id = json["id"].as_str().unwrap_or(arg).to_owned();
        let (x, y) = (number(&json, "x"), number(&json, "y"));
        Self { arg, json, id, x, y }
    }

    /// The request's number `key`; 0 when it has none.
    pub(super) fn number(&self, key: &str) -> f64 {
        number(&self.json, key)
    }

    /// The request's flag `key`; false when it has none.
    pub(super) fn flag(&self, key: &str) -> bool {
        self.json[key].as_bool().unwrap_or_default()
    }
}

fn number(request: &Value, key: &str) -> f64 {
    request[key].as_f64().unwrap_or_default()
}

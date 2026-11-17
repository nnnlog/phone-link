//! The window's JSON arguments to the messages actions.
//!
//! Each is read field by field; `None` when one is missing or mistyped.

use serde_json::Value;
use tracing::warn;

/// An item of the conversation list, as the window names it.
#[derive(Debug, PartialEq, Eq)]
pub(super) enum ListItem {
    Conversation(i64),
    NewMessage,
    Unsent(Vec<String>),
}

/// People another part of the app asks to write to, and text for the draft.
#[derive(Debug)]
pub(super) struct Compose {
    pub(super) addresses: Vec<String>,
    pub(super) text: String,
}

/// A conversation's alerts or hiding turned on or off.
#[derive(Debug)]
pub(super) struct Flag {
    pub(super) thread_id: i64,
    pub(super) on: bool,
}

/// An attachment to save, by its part key, and the file URL the user chose for it.
#[derive(Debug)]
pub(super) struct SavePart {
    pub(super) key: String,
    pub(super) url: String,
}

/// Image data dropped on a conversation: base64, and its MIME type when the drop named one.
#[derive(Debug)]
pub(super) struct DroppedImage {
    pub(super) data: String,
    pub(super) content_type: Option<String>,
}

/// A switch of the Settings window's Messages pane, by its name, turned on or off.
#[derive(Debug)]
pub(super) struct Setting {
    pub(super) name: String,
    pub(super) on: bool,
}

pub(super) fn strings(value: &Value) -> Option<Vec<String>> {
    value.as_array()?.iter().map(|item| item.as_str().map(str::to_owned)).collect()
}

fn text(value: &Value) -> Option<String> {
    value.as_str().map(str::to_owned)
}

impl ListItem {
    /// `Some(None)` for `null`, the window's "nothing selected".
    pub(super) fn read(value: &Value) -> Option<Option<Self>> {
        if value.is_null() {
            return Some(None);
        }
        let item = match value["kind"].as_str()? {
            "conversation" => Self::Conversation(value["threadId"].as_i64()?),
            "newMessage" => Self::NewMessage,
            "unsent" => Self::Unsent(strings(&value["addresses"])?),
            _ => return None,
        };
        Some(Some(item))
    }
}

impl Compose {
    pub(super) fn read(value: &Value) -> Option<Self> {
        let text = if value["text"].is_null() { String::new() } else { text(&value["text"])? };
        Some(Self { addresses: strings(&value["addresses"])?, text })
    }
}

impl Flag {
    pub(super) fn read(value: &Value) -> Option<Self> {
        Some(Self { thread_id: value["threadId"].as_i64()?, on: value["on"].as_bool()? })
    }
}

impl SavePart {
    pub(super) fn read(value: &Value) -> Option<Self> {
        Some(Self { key: text(&value["key"])?, url: text(&value["url"])? })
    }
}

impl DroppedImage {
    pub(super) fn read(value: &Value) -> Option<Self> {
        let content_type = if value["type"].is_null() { None } else { Some(text(&value["type"])?) };
        Some(Self { data: text(&value["data"])?, content_type })
    }
}

impl Setting {
    pub(super) fn read(value: &Value) -> Option<Self> {
        Some(Self { name: text(&value["name"])?, on: value["on"].as_bool()? })
    }
}

/// Reads an action's JSON argument; a malformed one is logged and the action dropped.
pub(super) fn parse<T>(name: &str, arg: &str, read: impl FnOnce(&Value) -> Option<T>) -> Option<T> {
    let value: Value = match serde_json::from_str(arg) {
        Ok(value) => value,
        Err(e) => {
            warn!(action = name, %arg, error = %e, "the action takes JSON");
            return None;
        }
    };

    let read = read(&value);
    if read.is_none() {
        warn!(action = name, %arg, "the action got JSON it does not take");
    }
    read
}

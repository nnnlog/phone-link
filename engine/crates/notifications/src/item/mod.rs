//! The notification item JSON: the identity rules that make an item malformed, the reading of every
//! other key, and what the list, the banner decision and the banner content derive from it.
//!
//! An item is applied unless a key read while storing it fails: the identity keys (`key`,
//! `packageName`, `postTime`, and `platform` with `version`), a JSON `null` as `text`, `title`,
//! `template` or — under a `$BigTextStyle` template — `bigText`, and a `publisherWebsite` that is not
//! a string. Every other key is read only after the item is stored (the alert decision, the list); a
//! present one of another JSON type reads as absent (differs from Phone Link on Windows).

use serde_json::{Map, Value as Json};

/// 0001-01-01T00:00:00Z as Unix milliseconds.
pub const MIN_UNIX_MS: i64 = -62_135_596_800_000;
/// 9999-12-31T23:59:59.999Z as Unix milliseconds.
pub const MAX_UNIX_MS: i64 = 253_402_300_799_999;

/// Lines of a `MessagingStyle` conversation are cut to the last this many entries.
const MESSAGING_STYLE_MAX_LINES: usize = 7;
/// The list offers up to this many non-reply actions.
const LIST_MAX_ACTIONS: usize = 4;
/// Banners offer only actions whose `actionIndex` is below this.
const BANNER_MAX_ACTION_INDEX: i32 = 3;

/// Why an item JSON is malformed: the engine skips the item, applies the rest of its batch, and logs
/// it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ItemError {
    NotJson,
    NotAnObject,
    /// A required key is missing or has another JSON type.
    MissingOrMistyped(&'static str),
    /// `postTime` is not an integral number of milliseconds within 0001-01-01 … 9999-12-31.
    PostTimeOutOfRange,
    /// `platform` and `version` are both present and `platform` is not `0` (Android).
    NotAndroid,
    /// A JSON `null` as `text`, `title`, `template` or — under a `$BigTextStyle` template — `bigText`.
    NullWhileStored(&'static str),
    /// `publisherWebsite` present but not a string.
    PublisherWebsiteNotAString,
}

impl ItemError {
    /// A fixed English phrase for the log (never content).
    pub fn reason(self) -> &'static str {
        match self {
            ItemError::NotJson => "item is not JSON",
            ItemError::NotAnObject => "item is not a JSON object",
            ItemError::MissingOrMistyped("key") => "item key missing or not a string",
            ItemError::MissingOrMistyped("packageName") => "item packageName missing or not a string",
            ItemError::MissingOrMistyped("postTime") => "item postTime missing or not an integral number",
            ItemError::MissingOrMistyped(_) => "item required key missing or mistyped",
            ItemError::PostTimeOutOfRange => "item postTime out of range",
            ItemError::NotAndroid => "item platform selects no Android parser",
            ItemError::NullWhileStored("text") => "item text is null",
            ItemError::NullWhileStored("title") => "item title is null",
            ItemError::NullWhileStored("template") => "item template is null",
            ItemError::NullWhileStored("bigText") => "item bigText is null under a BigTextStyle template",
            ItemError::NullWhileStored(_) => "item key read while storing is null",
            ItemError::PublisherWebsiteNotAString => "item publisherWebsite is not a string",
        }
    }
}

/// One `notificationActions` entry with all three fields of their stated types.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ItemAction {
    pub name: String,
    pub inline_reply: bool,
    /// The phone's index, sent back verbatim as `actionIndex`.
    pub index: i32,
    /// The entry's position in `notificationActions` (identifies it when two entries share an index).
    pub position: usize,
}

/// One line of a `MessagingStyle` conversation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ConversationLine {
    /// A sender heading: shown whenever the sender changes.
    Sender(String),
    Message(String),
}

/// How the list shows an item.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ItemDisplay {
    pub title: Option<String>,
    pub body: String,
    /// The `MessagingStyle` conversation; empty when the item has none.
    pub conversation: Vec<ConversationLine>,
}

/// A parsed Android item. `json` is the text as received (what the store keeps).
#[derive(Debug, Clone, PartialEq)]
pub struct NotificationItem {
    pub key: String,
    pub package_name: String,
    /// Unix epoch milliseconds.
    pub post_time: i64,
    obj: Map<String, Json>,
}

fn object_of(text: &str) -> Result<Map<String, Json>, ItemError> {
    match serde_json::from_str::<Json>(text) {
        Ok(Json::Object(obj)) => Ok(obj),
        Ok(_) => Err(ItemError::NotAnObject),
        Err(_) => Err(ItemError::NotJson),
    }
}

/// The other keys read while storing an item: `text`, `title` and `template` when present, `bigText`
/// when the template's JSON text names `$BigTextStyle` and neither `$InboxStyle` nor
/// `$MessagingStyle`, and `publisherWebsite` read as a string.
fn check_read_while_stored(obj: &Map<String, Json>) -> Result<(), ItemError> {
    for k in ["text", "title", "template"] {
        if obj.get(k).is_some_and(Json::is_null) {
            return Err(ItemError::NullWhileStored(k));
        }
    }

    if let Some(template) = obj.get("template").map(Json::to_string) {
        let big_text_only = template.contains("$BigTextStyle")
            && !template.contains("$InboxStyle")
            && !template.contains("$MessagingStyle");
        if big_text_only && obj.get("bigText").is_some_and(Json::is_null) {
            return Err(ItemError::NullWhileStored("bigText"));
        }
    }

    if obj.get("publisherWebsite").is_some_and(|v| !v.is_string()) {
        return Err(ItemError::PublisherWebsiteNotAString);
    }
    Ok(())
}

/// A JSON number truncated toward zero to an `i64`.
fn number_trunc(v: &Json) -> Option<i64> {
    if let Some(i) = v.as_i64() {
        return Some(i);
    }
    let f = v.as_f64()?;
    let t = f.trunc();
    if t.is_finite() && t >= i64::MIN as f64 && t <= i64::MAX as f64 { Some(t as i64) } else { None }
}

/// A JSON integer that fits an `i32`.
fn int32_of(v: &Json) -> Option<i32> {
    v.as_i64().and_then(|i| i32::try_from(i).ok())
}

impl NotificationItem {
    /// Parses `text` with the Android rules, in this order: when `platform` and `version` are both
    /// present, `platform` must be `0`; `key` (string), `packageName` (string) and `postTime` (an
    /// integral number of milliseconds within 0001-01-01 … 9999-12-31) are required; then the other
    /// keys read while storing (else the item is malformed).
    pub fn parse(text: &str) -> Result<Self, ItemError> {
        let obj = object_of(text)?;
        if obj.contains_key("platform") && obj.contains_key("version") {
            let android = obj.get("platform").and_then(Json::as_f64).is_some_and(|p| p.trunc() == 0.0);
            if !android {
                return Err(ItemError::NotAndroid);
            }
        }

        let key = obj.get("key").and_then(Json::as_str).ok_or(ItemError::MissingOrMistyped("key"))?.to_string();
        let package_name = obj
            .get("packageName")
            .and_then(Json::as_str)
            .ok_or(ItemError::MissingOrMistyped("packageName"))?
            .to_string();
        let post_time = obj.get("postTime").and_then(Json::as_i64).ok_or(ItemError::MissingOrMistyped("postTime"))?;
        if !(MIN_UNIX_MS..=MAX_UNIX_MS).contains(&post_time) {
            return Err(ItemError::PostTimeOutOfRange);
        }

        check_read_while_stored(&obj)?;
        Ok(Self { key, package_name, post_time, obj })
    }

    fn string(&self, k: &str) -> Option<&str> {
        self.obj.get(k).and_then(Json::as_str)
    }

    fn string_array(&self, k: &str) -> Option<Vec<String>> {
        let arr = self.obj.get(k)?.as_array()?;
        arr.iter().map(|v| v.as_str().map(str::to_string)).collect()
    }

    /// The length of `k` when it is a JSON array, whatever its elements.
    fn array_len(&self, k: &str) -> Option<usize> {
        self.obj.get(k).and_then(Json::as_array).map(Vec::len)
    }

    fn template(&self) -> &str {
        self.string("template").unwrap_or_default()
    }

    pub fn app_name(&self) -> Option<&str> {
        self.string("appName")
    }

    pub fn title(&self) -> Option<&str> {
        self.string("title")
    }

    pub fn text(&self) -> Option<&str> {
        self.string("text")
    }

    pub fn subtitle(&self) -> Option<&str> {
        self.string("subtitle")
    }

    /// `timestamp` when non-zero and representable (Unix ms).
    pub fn timestamp(&self) -> Option<i64> {
        let t = number_trunc(self.obj.get("timestamp")?)?;
        (t != 0 && (MIN_UNIX_MS..MAX_UNIX_MS).contains(&t)).then_some(t)
    }

    /// The displayed time and the banner-recency time: `timestamp` when set, else `postTime`.
    pub fn event_time_ms(&self) -> i64 {
        self.timestamp().unwrap_or(self.post_time)
    }

    /// `notificationClass`: `1` messaging, `2` media-style, `3` phone call; else ordinary.
    pub fn notification_class(&self) -> Option<i32> {
        self.obj.get("notificationClass").and_then(int32_of)
    }

    pub fn is_messaging(&self) -> bool {
        self.notification_class() == Some(1)
    }

    pub fn is_media_style(&self) -> bool {
        self.notification_class() == Some(2)
    }

    pub fn is_phone_call(&self) -> bool {
        self.notification_class() == Some(3)
    }

    /// `largeIcon`: base64 image bytes (a media-style item's album art).
    pub fn large_icon(&self) -> Option<&str> {
        self.string("largeIcon")
    }

    /// `smallIcon`: base64 image bytes (a media-style item's app icon).
    pub fn small_icon(&self) -> Option<&str> {
        self.string("smallIcon")
    }

    /// `playbackState` (Android's `PlaybackState.STATE_*`) when it is an integer.
    pub fn playback_state(&self) -> Option<i32> {
        self.obj.get("playbackState").and_then(int32_of)
    }

    /// `importance` as sent, when it is an integer.
    pub fn importance(&self) -> Option<i32> {
        self.obj.get("importance").and_then(int32_of)
    }

    /// The importance the banner decision uses: absent ⇒ `3` (Default).
    pub fn banner_importance(&self) -> i32 {
        self.importance().unwrap_or(3)
    }

    /// `flags` bit `8` (only-alert-once).
    pub fn only_alert_once(&self) -> bool {
        self.obj.get("flags").and_then(int32_of).is_some_and(|f| f & 8 != 0)
    }

    /// `featureFlags` bit `2`: the notification supports launch.
    pub fn launch_supported(&self) -> bool {
        self.obj.get("featureFlags").and_then(number_trunc).is_some_and(|f| f & 2 != 0)
    }

    pub fn is_read_only_group_message(&self) -> bool {
        self.obj.get("isReadOnlyGroupMessage").and_then(Json::as_bool) == Some(true)
    }

    /// Multi-line content: a non-empty `textLines` with an `$InboxStyle` template, else a non-empty
    /// `messages` with a `$MessagingStyle` template.
    pub fn has_multi_line_content(&self) -> bool {
        let template = self.template();
        let len = if template.contains("$InboxStyle") {
            self.array_len("textLines")
        } else if template.contains("$MessagingStyle") {
            self.array_len("messages")
        } else {
            None
        };
        len.is_some_and(|n| n > 0)
    }

    /// `D` of the content dedupe hash: `bigText` for a `$BigTextStyle` template (empty when absent),
    /// else `text` (empty when absent). An `$InboxStyle` or `$MessagingStyle` template keeps `text`.
    pub fn dedupe_description(&self) -> &str {
        let template = self.template();
        if !template.contains("$InboxStyle")
            && !template.contains("$MessagingStyle")
            && template.contains("$BigTextStyle")
        {
            return self.string("bigText").unwrap_or_default();
        }
        self.text().unwrap_or_default()
    }

    /// The `notificationActions` entries that carry all three fields with their types, in order;
    /// an entry whose `actionIndex` is negative or beyond `Int32` is skipped too.
    pub fn actions(&self) -> Vec<ItemAction> {
        let Some(entries) = self.obj.get("notificationActions").and_then(Json::as_array) else {
            return Vec::new();
        };
        entries
            .iter()
            .enumerate()
            .filter_map(|(position, e)| {
                let e = e.as_object()?;
                let name = e.get("actionName")?.as_str()?.to_string();
                let inline_reply = e.get("isActionInlineReply")?.as_bool()?;
                let index = number_trunc(e.get("actionIndex")?).and_then(|i| i32::try_from(i).ok())?;
                (index >= 0).then_some(ItemAction { name, inline_reply, index, position })
            })
            .collect()
    }

    /// The list's reply box: the first reply action.
    pub fn list_reply(&self) -> Option<ItemAction> {
        self.actions().into_iter().find(|a| a.inline_reply)
    }

    /// The list's buttons: the first four non-reply actions.
    pub fn list_actions(&self) -> Vec<ItemAction> {
        self.actions().into_iter().filter(|a| !a.inline_reply).take(LIST_MAX_ACTIONS).collect()
    }

    /// The banner's reply actions: one per reply entry with `actionIndex < 3`, in entry order.
    pub fn banner_reply_actions(&self) -> Vec<ItemAction> {
        self.actions().into_iter().filter(|a| a.inline_reply && a.index < BANNER_MAX_ACTION_INDEX).collect()
    }

    /// How the list shows the item: `template` substring tests in this order —
    /// `$BigTextStyle`, then `$InboxStyle`, then `$MessagingStyle`.
    pub fn display(&self) -> ItemDisplay {
        let mut title = self.title().map(str::to_string);
        let mut body = self.text().unwrap_or_default().to_string();
        let mut conversation = Vec::new();
        let template = self.template();

        if template.contains("$BigTextStyle") {
            if let Some(b) = self.string("bigText") {
                body = b.to_string();
            }
            if let Some(t) = self.string("bigTitle") {
                title = Some(t.to_string());
            }
        } else if template.contains("$InboxStyle") {
            if let Some(lines) = self.string_array("textLines") {
                body = lines.join("\n");
            }
        } else if template.contains("$MessagingStyle") {
            conversation = self.messaging_style_lines();
            if conversation.is_empty() {
                if let Some(lines) = self.string_array("textLines") {
                    body = lines.join("\n");
                }
            }

            let title_is_a_sender = conversation
                .iter()
                .any(|l| matches!(l, ConversationLine::Sender(s) if Some(s.as_str()) == title.as_deref()));
            if title_is_a_sender {
                if let Some(ConversationLine::Sender(first)) = conversation.first() {
                    title = Some(first.clone());
                }
                conversation.remove(0);
            }
        }

        ItemDisplay { title, body, conversation }
    }

    /// `messages` / `senderNames`, each cut to its last 7 entries: sender/message runs when both have
    /// the same non-zero length; the messages alone when a sender is blank; else nothing.
    fn messaging_style_lines(&self) -> Vec<ConversationLine> {
        let (Some(senders), Some(messages)) = (self.string_array("senderNames"), self.string_array("messages")) else {
            return Vec::new();
        };

        let last = |v: Vec<String>| -> Vec<String> {
            let skip = v.len().saturating_sub(MESSAGING_STYLE_MAX_LINES);
            v.into_iter().skip(skip).collect()
        };

        let (senders, messages) = (last(senders), last(messages));
        if messages.is_empty() || senders.len() != messages.len() {
            return Vec::new();
        }
        if senders.iter().any(|s| s.trim().is_empty()) {
            return messages.into_iter().map(ConversationLine::Message).collect();
        }

        let mut lines = Vec::new();
        let mut current: Option<&str> = None;
        for (sender, message) in senders.iter().zip(messages) {
            if current != Some(sender.as_str()) {
                current = Some(sender.as_str());
                lines.push(ConversationLine::Sender(sender.clone()));
            }
            lines.push(ConversationLine::Message(message));
        }
        lines
    }
}

#[cfg(feature = "test-support")]
/// The `postTime` of a stored item JSON (it was validated when stored).
pub fn stored_post_time(json: &str) -> Option<i64> {
    object_of(json).ok()?.get("postTime").and_then(Json::as_i64)
}

#[cfg(test)]
mod tests;

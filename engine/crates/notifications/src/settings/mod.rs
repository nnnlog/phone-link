//! Per-app notification settings.
//!
//! It covers the `NotificationsAppSettingsValue` flags, how a stored row reads, how an app's
//! effective flags combine the phone's sync row with the PC's local row, the `/settings` request
//! bodies (triggers 25, 26, 27) and the parse of the phone's `savedSetting` / `syncSetting` answer.

use serde::Serialize;
use serde_json::Value as Json;

use phonelink_core::codec::valueset::{Value, ValueSet};

use super::value_text;
use super::wire::{SETTINGS_CONTENT_TYPE, key, setting, trigger_type};

/// `NotificationsAppSettingsValue`: `1` `ShowNotificationsInFeed` (the phone's "sync this app"), `2`
/// `ShowNotificationsInBanner` (PC-only banners). Parsed values may carry other bits.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct AppFlags(pub u32);

impl AppFlags {
    pub const NONE: AppFlags = AppFlags(0);
    pub const FEED: AppFlags = AppFlags(1);
    pub const BANNER: AppFlags = AppFlags(2);
    pub const ALL: AppFlags = AppFlags(3);

    pub fn feed(self) -> bool {
        self.0 & Self::FEED.0 != 0
    }

    pub fn banner(self) -> bool {
        self.0 & Self::BANNER.0 != 0
    }

    pub fn with_feed(self, on: bool) -> Self {
        if on { AppFlags(self.0 | Self::FEED.0) } else { AppFlags(self.0 & !Self::FEED.0) }
    }

    pub fn with_banner(self, on: bool) -> Self {
        if on { AppFlags(self.0 | Self::BANNER.0) } else { AppFlags(self.0 & !Self::BANNER.0) }
    }

    /// Only the feed bit.
    pub fn feed_part(self) -> Self {
        AppFlags(self.0 & Self::FEED.0)
    }

    /// Only the banner bit.
    pub fn banner_part(self) -> Self {
        AppFlags(self.0 & Self::BANNER.0)
    }
}

impl std::ops::BitOr for AppFlags {
    type Output = AppFlags;
    fn bitor(self, rhs: AppFlags) -> AppFlags {
        AppFlags(self.0 | rhs.0)
    }
}

/// One settings row (`settings` store; keyed by `(group_id, key)`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SettingRow {
    pub group_id: String,
    pub key: String,
    pub setting_type: i32,
    pub value: String,
    /// UTC ticks (100 ns since 0001-01-01); `0` = none.
    pub mute_expiration_ticks: i64,
}

/// A `NotificationsAppSettingsValue` as text: a number, or member names joined by `,`.
pub fn parse_flags_text(text: &str) -> Option<AppFlags> {
    let t = text.trim();
    if t.is_empty() {
        return None;
    }
    if t.starts_with(|c: char| c.is_ascii_digit() || c == '+' || c == '-') {
        return t.strip_prefix('+').unwrap_or(t).parse::<u32>().ok().map(AppFlags);
    }

    let mut bits = 0u32;
    for name in t.split(',') {
        bits |= match name.trim() {
            "None" => 0,
            "ShowNotificationsInFeed" => 1,
            "ShowNotificationsInBanner" => 2,
            "AllNotifications" => 3,
            _ => return None,
        };
    }
    Some(AppFlags(bits))
}

/// How a stored row reads: type `11` ⇒ `"1"` is sync on (Feed), anything else None;
/// type `5` ⇒ the value parsed as flags, All when it does not parse; no row or another type ⇒ All.
pub fn read_row(row: Option<&SettingRow>) -> AppFlags {
    match row {
        Some(r) if r.setting_type == setting::TYPE_BOOLEAN => {
            if r.value == "1" {
                AppFlags::FEED
            } else {
                AppFlags::NONE
            }
        }
        Some(r) if r.setting_type == setting::TYPE_FLAGS => parse_flags_text(&r.value).unwrap_or(AppFlags::ALL),
        _ => AppFlags::ALL,
    }
}

/// An app's effective flags: the sync row's feed bit, plus the local row's flags (All when there is
/// no local row).
///
/// With quiet by default on, an app without a local row takes the banner
/// bit from the default preference instead, and only while its feed bit is on.
pub fn effective_flags(
    sync_row: Option<&SettingRow>,
    local_row: Option<&SettingRow>,
    quiet_by_default: bool,
    default_preference: AppFlags,
) -> AppFlags {
    let sync = read_row(sync_row).feed_part();
    if local_row.is_none() && quiet_by_default {
        let local = if sync.feed() { default_preference.banner_part() } else { AppFlags::NONE };
        return sync | local;
    }
    sync | read_row(local_row)
}

/// One setting of a `/settings` request: the app's sync flag.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SyncSettingItem {
    pub setting_key: String,
    pub setting_value: String,
    pub setting_type: i32,
    pub setting_group_id: String,
    pub setting_mute_expiration_timestamp: i64,
}

impl SyncSettingItem {
    /// The sync item of `package` for `flags`: `"1"` when the feed bit is on, else `"0"`.
    pub fn new(package: &str, flags: AppFlags) -> Self {
        Self {
            setting_key: package.to_string(),
            setting_value: if flags.feed() { "1" } else { "0" }.to_string(),
            setting_type: setting::TYPE_BOOLEAN,
            setting_group_id: setting::SYNC_GROUP.to_string(),
            setting_mute_expiration_timestamp: 0,
        }
    }
}

fn settings_tail(mut vs: ValueSet, trigger: i32, correlation_vector: &str) -> ValueSet {
    vs.insert(key::TRIGGER_TYPE, trigger);
    vs.insert(phonelink_app::consts::key::CONTENT_TYPE, SETTINGS_CONTENT_TYPE);
    vs.insert(phonelink_app::consts::key::CORRELATION_VECTOR, correlation_vector.to_string());
    vs
}

/// Trigger 25: no setting keys.
pub fn sync_all_body(correlation_vector: &str) -> ValueSet {
    settings_tail(ValueSet::new(), trigger_type::SYNC_SETTINGS, correlation_vector)
}

/// Trigger 26: one app's sync flag.
pub fn setting_changed_body(item: &SyncSettingItem, correlation_vector: &str) -> ValueSet {
    let vs = ValueSet::new()
        .with(key::SETTING_KEY, item.setting_key.clone())
        .with(key::SETTING_VALUE, item.setting_value.clone())
        .with(key::SETTING_TYPE, item.setting_type)
        .with(key::SETTING_GROUP_ID, item.setting_group_id.clone())
        .with(key::SETTING_MUTE_EXPIRATION_TIMESTAMP, Value::Int64(item.setting_mute_expiration_timestamp));
    settings_tail(vs, trigger_type::SETTING_CHANGED, correlation_vector)
}

/// Trigger 27: `settings` = JSON `{"settings":[…]}` with the items' keys in their stated order.
pub fn batch_body(items: &[SyncSettingItem], correlation_vector: &str) -> ValueSet {
    #[derive(Serialize)]
    struct Batch<'a> {
        settings: &'a [SyncSettingItem],
    }
    let text = serde_json::to_string(&Batch { settings: items }).expect("settings items serialize");
    settings_tail(ValueSet::new().with(key::SETTINGS, text), trigger_type::BATCH_SETTINGS_CHANGED, correlation_vector)
}

fn json_object(text: &str) -> Option<serde_json::Map<String, Json>> {
    match serde_json::from_str::<Json>(text) {
        Ok(Json::Object(o)) => Some(o),
        _ => None,
    }
}

/// One JSON setting object: `None` when a key is missing (skipped); `Err` when a key has another JSON
/// type, which ends the parse.
fn setting_from_json(o: &serde_json::Map<String, Json>) -> Result<Option<SettingRow>, ()> {
    let names = [key::SETTING_KEY, key::SETTING_VALUE, key::SETTING_TYPE, key::SETTING_GROUP_ID];
    if names.iter().any(|n| !o.contains_key(*n)) {
        return Ok(None);
    }

    let string = |n: &str| o.get(n).and_then(Json::as_str).map(str::to_string).ok_or(());
    let number = |n: &str| o.get(n).and_then(Json::as_f64).ok_or(());

    let key = string(key::SETTING_KEY)?;
    let group_id = string(key::SETTING_GROUP_ID)?;
    let value = string(key::SETTING_VALUE)?;
    let setting_type = number(key::SETTING_TYPE)? as i32;
    let mute_expiration_ticks = match o.get(key::SETTING_MUTE_EXPIRATION_TIMESTAMP) {
        None => 0,
        Some(_) => number(key::SETTING_MUTE_EXPIRATION_TIMESTAMP)? as i64,
    };

    Ok(Some(SettingRow { group_id, key, setting_type, value, mute_expiration_ticks }))
}

/// The settings a `Success` answer carries: `savedSetting` when it holds a JSON
/// object, else `syncSetting`'s `settings` array.
///
/// An object missing a key is skipped; an object (or array element) of another JSON type ends the
/// parse, keeping the objects before it; text that is not a JSON object yields nothing.
pub fn parse_response(values: &ValueSet) -> Vec<SettingRow> {
    if let Some(o) = values.get(key::SAVED_SETTING).and_then(value_text).and_then(|t| json_object(&t)) {
        return setting_from_json(&o).ok().flatten().into_iter().collect();
    }

    let Some(o) = values.get(key::SYNC_SETTING).and_then(value_text).and_then(|t| json_object(&t)) else {
        return Vec::new();
    };
    let Some(items) = o.get(key::SETTINGS).and_then(Json::as_array) else {
        return Vec::new();
    };

    let mut rows = Vec::new();
    for item in items {
        let Some(obj) = item.as_object() else { break };
        match setting_from_json(obj) {
            Ok(Some(row)) => rows.push(row),
            Ok(None) => {}
            Err(()) => break,
        }
    }
    rows
}

#[cfg(test)]
mod tests;

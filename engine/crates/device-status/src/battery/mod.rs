//! The low-battery notification.
//!
//! It covers battery `≤ 9` %, not `-1`, not charging, at most once per hour, while the per-phone
//! setting (default on) allows and the system lets the app notify — applied to the battery the
//! precedence rule selects.
//!
//! The setting and the time of the last notification are
//! persisted per phone.

use std::time::{Duration, SystemTime};

use rusqlite::OptionalExtension;

use phonelink_core::types::DcgClient;
use phonelink_features::shared::db::{FeatureDb, StoreError};
use phonelink_features::shared::notifier::SystemNotification;

/// The feature's per-phone store schema.
pub const MIGRATIONS: &[&str] = &["CREATE TABLE low_battery_notification (
        id INTEGER PRIMARY KEY CHECK (id = 0),
        enabled INTEGER NOT NULL,
        last_shown_ms INTEGER
    );"];

/// The persisted part of the rule for one phone.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LowBatterySettings {
    /// The user setting; on until the user turns it off.
    pub enabled: bool,
    /// When a notification was last posted (`None`: never).
    pub last_shown: Option<SystemTime>,
}

impl Default for LowBatterySettings {
    fn default() -> Self {
        Self { enabled: true, last_shown: None }
    }
}

pub(crate) fn load(db: &FeatureDb) -> Result<LowBatterySettings, StoreError> {
    let row: Option<(bool, Option<i64>)> = db.read(|c| {
        c.query_row("SELECT enabled, last_shown_ms FROM low_battery_notification WHERE id = 0", [], |r| {
            Ok((r.get(0)?, r.get(1)?))
        })
        .optional()
    })?;
    Ok(row.map_or_else(LowBatterySettings::default, |(enabled, last)| LowBatterySettings {
        enabled,
        last_shown: last.map(|ms| phonelink_core::time::system_time(ms.max(0))),
    }))
}

pub(crate) fn save(db: &FeatureDb, settings: LowBatterySettings) -> Result<(), StoreError> {
    db.transaction(|tx| -> Result<(), StoreError> {
        tx.execute(
            "INSERT INTO low_battery_notification (id, enabled, last_shown_ms) VALUES (0, ?1, ?2)
             ON CONFLICT (id) DO UPDATE SET enabled = excluded.enabled, last_shown_ms = excluded.last_shown_ms",
            rusqlite::params![settings.enabled, settings.last_shown.map(|t| phonelink_core::time::unix_ms(t).max(0))],
        )
        .map(|_| ())
        .map_err(StoreError::from)
    })
}

/// The battery state the rule fires on: a level `≤ threshold` that is not `-1`, not charging.
pub fn is_low(level: Option<i32>, charging: bool, threshold: i32) -> bool {
    level.is_some_and(|l| l <= threshold && l != -1) && !charging
}

/// No notification was posted within `interval` before `now`.
pub fn interval_passed(last_shown: Option<SystemTime>, now: SystemTime, interval: Duration) -> bool {
    last_shown.is_none_or(|last| now.duration_since(last).is_ok_and(|elapsed| elapsed >= interval))
}

/// The notification id of `phone`'s low-battery notification (`device_status/…`).
pub fn notification_id(phone: &DcgClient) -> String {
    format!("device_status/low-battery/{}.{}", phone.id.to_ascii_lowercase(), phone.env)
}

/// `user_info` keys naming the notification's phone.
pub const USER_INFO_PHONE_ID: &str = "phoneId";
pub const USER_INFO_ENVIRONMENT: &str = "environment";

/// The notification: the phone's name and its battery level; `user_info` names the phone.
pub fn notification(phone: &DcgClient, phone_name: &str, level: i32) -> SystemNotification {
    SystemNotification {
        id: notification_id(phone),
        thread_id: Some("device_status".into()),
        title: format!("{phone_name}: low battery"),
        subtitle: None,
        body: format!("{level}% battery remaining."),
        alert: true,
        actions: Vec::new(),
        user_info: [
            (USER_INFO_PHONE_ID.to_owned(), phone.id.clone()),
            (USER_INFO_ENVIRONMENT.to_owned(), phone.env.name().to_owned()),
        ]
        .into_iter()
        .collect(),
        image: None,
    }
}

/// The phone a notification's `user_info` names.
pub fn notification_phone(user_info: &std::collections::HashMap<String, String>) -> Option<DcgClient> {
    let id = user_info.get(USER_INFO_PHONE_ID)?;
    let env = phonelink_core::types::Environment::from_name(user_info.get(USER_INFO_ENVIRONMENT)?)?;
    Some(DcgClient::new(id.clone(), env))
}

#[cfg(test)]
mod tests;

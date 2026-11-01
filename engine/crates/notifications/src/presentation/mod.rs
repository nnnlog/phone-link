//! Presentation on macOS (`UNUserNotificationCenter` through
//! [`SystemNotifier`](phonelink_features::shared::notifier::SystemNotifier)).
//!
//! It covers whether an upserted item alerts (Phone Link's decision rules), the 20 s duplicate
//! check, per-package throttling with its "paused" notice, and the content of the system
//! notifications this feature posts.
//!
//! Ids: `notifications/<phone>/<key>` for a phone notification; the two notices use
//! `notifications/<phone>:paused/<packageName>` and `notifications/<phone>:reply-failed/<key>`, which
//! can never equal a notification id (a phone's string form has no `:`).

use std::collections::{HashMap, VecDeque};
use std::time::{Duration, SystemTime};

use sha2::{Digest, Sha256};

use phonelink_core::types::DcgClient;
use phonelink_features::shared::notifier::{NotificationAction, SystemNotification, TextInput};

use super::item::NotificationItem;
use super::settings::AppFlags;

/// `user_info` keys of the posted notifications.
pub mod info {
    pub const KIND: &str = "kind";
    pub const PHONE_ID: &str = "phoneId";
    pub const PHONE_ENV: &str = "phoneEnv";
    pub const KEY: &str = "key";
    pub const PACKAGE_NAME: &str = "packageName";
    pub const IMPORTANCE: &str = "importance";
    /// `kind` values.
    pub const KIND_NOTIFICATION: &str = "notification";
    pub const KIND_PAUSED: &str = "paused";
    pub const KIND_REPLY_FAILED: &str = "replyFailed";
    /// The action id prefix of a reply action: `reply/<actionIndex>/<position>`, the entry's
    /// `actionIndex` and its position in `notificationActions` (unique per entry).
    pub const REPLY_ACTION_PREFIX: &str = "reply/";
    /// The "paused" notice's button: turn throttling off for this phone.
    pub const DISABLE_THROTTLING_ACTION: &str = "disableThrottling";
}

/// The English texts of what the engine posts (the GUI may localize by id later).
pub mod text {
    pub const REPLY_SEND_BUTTON: &str = "Send";
    pub const REPLY_PLACEHOLDER: &str = "Reply";
    pub const PAUSED_BUTTON: &str = "Don't pause";
    pub const REPLY_FAILED_TITLE: &str = "Couldn't send your reply";
    pub const REPLY_FAILED_BODY: &str = "Try again, or reply on your phone.";

    pub fn paused_title(app_name: &str) -> String {
        format!("{app_name} notifications paused")
    }

    pub fn paused_body(minutes: u64) -> String {
        format!("You're getting a lot of notifications from this app. Banners are paused for {minutes} minutes.")
    }
}

/// The id of `key`'s system notification.
pub fn notification_id(phone: &DcgClient, key: &str) -> String {
    format!("notifications/{phone}/{key}")
}

pub fn paused_notice_id(phone: &DcgClient, package: &str) -> String {
    format!("notifications/{phone}:paused/{package}")
}

pub fn reply_failed_notice_id(phone: &DcgClient, key: &str) -> String {
    format!("notifications/{phone}:reply-failed/{key}")
}

fn sha256(text: &str) -> [u8; 32] {
    Sha256::digest(text.as_bytes()).into()
}

/// The 20 s duplicate check: SHA-256 of `title|D|packageName` (content) and of
/// `key|<millisecond component of postTime>` (metadata), each remembered when checked.
#[derive(Debug, Default)]
pub struct Dedupe {
    content: HashMap<[u8; 32], SystemTime>,
    metadata: HashMap<[u8; 32], SystemTime>,
}

impl Dedupe {
    /// Whether each hash was seen within `window` before `now`; records both at `now`.
    pub fn check(&mut self, item: &NotificationItem, now: SystemTime, window: Duration) -> (bool, bool) {
        let content = sha256(&format!(
            "{}|{}|{}",
            item.title().unwrap_or_default(),
            item.dedupe_description(),
            item.package_name
        ));
        let metadata = sha256(&format!("{}|{}", item.key, item.post_time.rem_euclid(1000)));

        let cutoff = now.checked_sub(window).unwrap_or(SystemTime::UNIX_EPOCH);
        self.content.retain(|_, t| *t > cutoff);
        self.metadata.retain(|_, t| *t > cutoff);

        let seen = (self.content.contains_key(&content), self.metadata.contains_key(&metadata));
        self.content.insert(content, now);
        self.metadata.insert(metadata, now);
        seen
    }
}

/// What throttling decides for one alert of a package.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[expect(clippy::enum_variant_names, reason = "each variant names how the alert is shown")]
pub enum ThrottleDecision {
    ShowNormally,
    /// In a cooldown: deliver silently.
    ShowSilently,
    /// This alert reached the threshold: deliver it silently, start the cooldown, post the notice.
    ShowSilentlyWithNotice,
}

/// Throttling parameters: the threshold, the monitoring period and the cooldown.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ThrottleConfig {
    pub threshold: u32,
    pub monitoring_period: Duration,
    pub cooldown: Duration,
}

#[derive(Debug, Default)]
struct AppThrottle {
    window: VecDeque<SystemTime>,
    cooldown_end: Option<SystemTime>,
}

/// Per-package (case-insensitive) throttling of one phone's alerts.
#[derive(Debug, Default)]
pub struct Throttle {
    apps: HashMap<String, AppThrottle>,
}

impl Throttle {
    pub fn process(&mut self, package: &str, now: SystemTime, cfg: &ThrottleConfig) -> ThrottleDecision {
        if package.is_empty() {
            return ThrottleDecision::ShowNormally;
        }

        let app = self.apps.entry(package.to_lowercase()).or_default();
        if app.cooldown_end.is_some_and(|end| end <= now) {
            app.cooldown_end = None;
            app.window.clear();
        }
        if app.cooldown_end.is_some() {
            return ThrottleDecision::ShowSilently;
        }

        let start = now.checked_sub(cfg.monitoring_period).unwrap_or(SystemTime::UNIX_EPOCH);
        while app.window.front().is_some_and(|t| *t < start) {
            app.window.pop_front();
        }

        app.window.push_back(now);
        if app.window.len() >= cfg.threshold as usize {
            app.cooldown_end = Some(now + cfg.cooldown);
            app.window.clear();
            return ThrottleDecision::ShowSilentlyWithNotice;
        }
        ThrottleDecision::ShowNormally
    }
}

/// Why an item does not alert (for the log).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NotShown {
    MediaStyle,
    NotImportant,
    AlertOnce,
    Duplicate,
    PermissionNotGranted,
    GroupMessagesDisabled,
    MutedAll,
    AppMuted,
    AppSettingDisabled,
    KeyWindow,
    Messaging,
    LowImportance,
    NotRecent,
}

/// Everything the decision reads besides the item.
#[derive(Debug, Clone, Copy)]
pub struct DecisionInput {
    /// No row had the key when the batch's upserts began.
    pub is_new: bool,
    /// Both hashes of [`Dedupe::check`].
    pub duplicate: (bool, bool),
    pub pc_permission: bool,
    pub device_permission: bool,
    /// The Messages feature allows group-message alerts (read only for group messages).
    pub group_alerts_allowed: bool,
    pub muted_all: bool,
    pub app_muted: bool,
    /// The app's effective flags (All when the per-app list is not supported).
    pub app_flags: AppFlags,
    /// A window of the app is the key window.
    pub key_window: bool,
    pub now_ms: i64,
    pub recency_ms: i64,
}

/// The alert decision, in order.
///
/// The toastable checks (media-style, importance, only-alert-once, duplicate), then the system
/// checks (permissions, group-message consent, mutes, app flags, key window, messaging class, low
/// importance, recency).
pub fn decide(item: &NotificationItem, d: &DecisionInput) -> Result<(), NotShown> {
    let importance = item.banner_importance();
    if item.is_media_style() {
        return Err(NotShown::MediaStyle);
    }
    if importance < 4 {
        return Err(NotShown::NotImportant);
    }
    if !d.is_new && item.only_alert_once() && !item.has_multi_line_content() {
        return Err(NotShown::AlertOnce);
    }
    if d.duplicate.0 && d.duplicate.1 {
        return Err(NotShown::Duplicate);
    }

    let group = item.is_read_only_group_message();
    if !d.device_permission || !d.pc_permission || (group && !d.group_alerts_allowed) {
        return Err(NotShown::PermissionNotGranted);
    }
    if d.muted_all {
        return Err(NotShown::MutedAll);
    }
    if d.app_muted {
        return Err(NotShown::AppMuted);
    }

    let flags_allow = if group { d.app_flags.feed() } else { d.app_flags.feed() || d.app_flags.banner() };
    if !flags_allow {
        return Err(NotShown::AppSettingDisabled);
    }
    if d.key_window {
        return Err(NotShown::KeyWindow);
    }
    if item.is_messaging() {
        return Err(NotShown::Messaging);
    }
    if importance <= 2 {
        return Err(NotShown::LowImportance);
    }
    if d.now_ms.saturating_sub(item.event_time_ms()) > d.recency_ms {
        return Err(NotShown::NotRecent);
    }
    Ok(())
}

/// The popup is allowed: the app's banner flag is on, or the item is a group message.
pub fn popup_allowed(item: &NotificationItem, flags: AppFlags) -> bool {
    item.is_read_only_group_message() || flags.banner()
}

fn base_info(phone: &DcgClient, kind: &str) -> HashMap<String, String> {
    HashMap::from([
        (info::KIND.to_string(), kind.to_string()),
        (info::PHONE_ID.to_string(), phone.id.clone()),
        (info::PHONE_ENV.to_string(), phone.env.name().to_string()),
    ])
}

/// A phone notification: title = `appName`, subtitle = `title`, body = `text`; one text-input
/// reply action per reply entry with `actionIndex < 3`; `alert: false` delivers it silently.
pub fn banner(phone: &DcgClient, item: &NotificationItem, alert: bool) -> SystemNotification {
    let replies = item.banner_reply_actions();
    let single = replies.len() == 1;
    let actions = replies
        .into_iter()
        .map(|a| NotificationAction {
            id: format!("{}{}/{}", info::REPLY_ACTION_PREFIX, a.index, a.position),
            title: a.name.clone(),
            text_input: Some(TextInput {
                button_title: text::REPLY_SEND_BUTTON.to_string(),
                placeholder: if single { a.name } else { text::REPLY_PLACEHOLDER.to_string() },
            }),
        })
        .collect();

    let mut user_info = base_info(phone, info::KIND_NOTIFICATION);
    user_info.insert(info::KEY.to_string(), item.key.clone());
    user_info.insert(info::PACKAGE_NAME.to_string(), item.package_name.clone());
    if let Some(i) = item.importance() {
        user_info.insert(info::IMPORTANCE.to_string(), i.to_string());
    }

    SystemNotification {
        id: notification_id(phone, &item.key),
        thread_id: Some(item.package_name.clone()),
        title: item.app_name().unwrap_or_default().to_string(),
        subtitle: item.title().filter(|t| !t.is_empty()).map(str::to_string),
        body: item.text().unwrap_or_default().to_string(),
        alert,
        actions,
        user_info,
        image: None,
    }
}

/// The "paused for N minutes" notice of a package entering its cooldown; N is the cooldown's
/// minutes component.
pub fn paused_notice(
    phone: &DcgClient,
    app_name: &str,
    package: &str,
    cooldown: Duration,
    alert: bool,
) -> SystemNotification {
    let mut user_info = base_info(phone, info::KIND_PAUSED);
    user_info.insert(info::PACKAGE_NAME.to_string(), package.to_string());

    SystemNotification {
        id: paused_notice_id(phone, package),
        thread_id: Some(package.to_string()),
        title: text::paused_title(app_name),
        subtitle: None,
        body: text::paused_body((cooldown.as_secs() / 60) % 60),
        alert,
        actions: vec![NotificationAction {
            id: info::DISABLE_THROTTLING_ACTION.to_string(),
            title: text::PAUSED_BUTTON.to_string(),
            text_input: None,
        }],
        user_info,
        image: None,
    }
}

/// The failure notice of a banner reply whose final status is not *Success*.
pub fn reply_failed_notice(phone: &DcgClient, key: &str, package: Option<&str>) -> SystemNotification {
    let mut user_info = base_info(phone, info::KIND_REPLY_FAILED);
    user_info.insert(info::KEY.to_string(), key.to_string());

    SystemNotification {
        id: reply_failed_notice_id(phone, key),
        thread_id: package.map(str::to_string),
        title: text::REPLY_FAILED_TITLE.to_string(),
        subtitle: None,
        body: text::REPLY_FAILED_BODY.to_string(),
        alert: true,
        actions: Vec::new(),
        user_info,
        image: None,
    }
}

#[cfg(test)]
mod tests;

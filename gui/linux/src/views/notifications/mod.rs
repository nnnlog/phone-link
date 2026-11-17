//! The notifications part as the window reads it: the list, what the list column shows instead of it,
//! the apps the settings pane lists, and the link of the post-notification help.

use std::collections::{BTreeMap, HashMap, HashSet};

use phonelink_engine::api::notifications::{
    AppNotificationFlags, AppNotificationSetting, NotificationLine, NotificationOpenTarget, NotificationsState,
    PhoneNotification,
};
use serde_json::{Value, json};

/// What the list column shows. The setup states come first, in priority order: this computer's switch
/// is asked about only once the phone has given access (or cannot), so a new user is first sent to the
/// phone.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Content {
    /// Nothing read yet.
    Loading,
    /// The phone has no notification listener for Link to Windows.
    Unavailable,
    /// Link to Windows on the phone (Android 13 or later) may not post notifications; the user allows
    /// it on the phone, through the link the post-notification help shows.
    NeedsPostNotificationPermission,
    /// "Show notifications from my phone" is off on this computer.
    TurnedOff,
    /// The phone answered the access request "device not supported" (a low-memory phone).
    LowMemoryPhone,
    /// The phone has not given Link to Windows notification access.
    NeedsAccess,
    /// The list is empty while a full sync is under way.
    Syncing,
    Empty,
    List,
    /// The state or the list could not be read; Try Again reads them again.
    Failed,
}

impl Content {
    /// What shows for `state` (`None` until it is read), when a read `failed`, with the list empty or
    /// not.
    pub fn of(state: Option<&NotificationsState>, failed: bool, list_empty: bool) -> Self {
        if failed {
            return Self::Failed;
        }
        let Some(state) = state else { return Self::Loading };
        if !state.capable {
            Self::Unavailable
        } else if state.post_notification_needed {
            Self::NeedsPostNotificationPermission
        } else if !state.pc_permission && (state.device_permission || state.low_ram_device) {
            Self::TurnedOff
        } else if state.low_ram_device {
            Self::LowMemoryPhone
        } else if !state.device_permission {
            Self::NeedsAccess
        } else if list_empty {
            if state.syncing { Self::Syncing } else { Self::Empty }
        } else {
            Self::List
        }
    }

    /// The name the QML switches on.
    pub fn name(self) -> &'static str {
        match self {
            Self::Loading => "loading",
            Self::Unavailable => "unavailable",
            Self::NeedsPostNotificationPermission => "needsPostNotificationPermission",
            Self::TurnedOff => "turnedOff",
            Self::LowMemoryPhone => "lowMemoryPhone",
            Self::NeedsAccess => "needsAccess",
            Self::Syncing => "syncing",
            Self::Empty => "empty",
            Self::List => "list",
            Self::Failed => "failed",
        }
    }
}

/// The app's name as the phone reports it, else its package name.
pub fn app_name(notification: &PhoneNotification) -> &str {
    notification.app_name.as_deref().filter(|name| !name.is_empty()).unwrap_or(&notification.package_name)
}

/// A row's text: the body, or a conversation's last message.
fn preview(notification: &PhoneNotification) -> &str {
    if !notification.body.is_empty() {
        return &notification.body;
    }
    notification
        .conversation
        .iter()
        .rev()
        .find_map(|line| match line {
            NotificationLine::Message { text } => Some(text.as_str()),
            NotificationLine::Sender { .. } => None,
        })
        .unwrap_or_default()
}

/// A messaging-style notification as sender runs: each sender beside their messages. A run may have
/// no sender: the engine drops the first heading when it became the title, and a conversation with a
/// blank sender has no headings at all.
fn conversation_runs(lines: &[NotificationLine]) -> Vec<Value> {
    let mut runs: Vec<(Option<&str>, Vec<&str>)> = Vec::new();
    for line in lines {
        match line {
            NotificationLine::Sender { name } => runs.push((Some(name), Vec::new())),
            NotificationLine::Message { text } => match runs.last_mut() {
                Some((_, messages)) => messages.push(text),
                None => runs.push((None, vec![text])),
            },
        }
    }
    runs.into_iter().map(|(sender, messages)| json!({ "sender": sender, "messages": messages })).collect()
}

fn open_target(target: NotificationOpenTarget) -> &'static str {
    match target {
        NotificationOpenTarget::Notifications => "notifications",
        NotificationOpenTarget::Messaging => "messaging",
        NotificationOpenTarget::Calls => "calls",
    }
}

/// The list as the window reads it, newest first; `apps` holds the listed apps' settings by package,
/// for a row's menu (an app whose setting was not read counts as on and not muted).
pub fn notifications_json(
    notifications: &[PhoneNotification],
    apps: &HashMap<String, AppNotificationSetting>,
) -> String {
    let items: Vec<Value> = notifications
        .iter()
        .map(|n| {
            let app = apps.get(&n.package_name);
            let actions: Vec<Value> = n.actions.iter().map(|a| json!({ "name": a.name, "index": a.index })).collect();
            json!({
                "key": n.key,
                "app": app_name(n),
                "package": n.package_name,
                "title": n.title.clone().unwrap_or_default(),
                "subtitle": n.subtitle.clone().unwrap_or_default(),
                "body": n.body,
                "preview": preview(n),
                "conversation": conversation_runs(&n.conversation),
                "timeMs": n.time_ms,
                "pinned": n.pinned,
                "openTarget": open_target(n.open_target),
                "launchSupported": n.launch_supported,
                "replyIndex": n.reply.as_ref().map(|r| r.index),
                "replyName": n.reply.as_ref().map(|r| r.name.as_str()),
                "actions": actions,
                "appOn": app.is_none_or(|setting| setting.flags.feed),
                "appMutedUntilMs": app.and_then(|setting| setting.muted_until_ms),
            })
        })
        .collect();
    Value::Array(items).to_string()
}

/// Adds the apps of `list` (newest first) to `known` (package → name): an app is named after its
/// newest notification that carries a name, else keeps the name it has, else its package name.
/// Returns whether `known` changed.
pub fn remember_apps(known: &mut BTreeMap<String, String>, list: &[PhoneNotification]) -> bool {
    let mut named = HashSet::new();
    let mut changed = false;

    for notification in list {
        let package = notification.package_name.as_str();
        if named.contains(package) {
            continue;
        }
        match notification.app_name.as_deref().map(str::trim).filter(|name| !name.is_empty()) {
            Some(name) => {
                named.insert(package);
                if known.get(package).map(String::as_str) != Some(name) {
                    known.insert(package.to_owned(), name.to_owned());
                    changed = true;
                }
            }
            None if !known.contains_key(package) => {
                known.insert(package.to_owned(), package.to_owned());
                changed = true;
            }
            None => {}
        }
    }
    changed
}

/// The remembered apps of every phone (phone id → package → name) after `stored` joins `known`: a
/// name `known` has (seen in this run) wins over the stored one.
pub fn merge_known_apps(
    known: &BTreeMap<String, BTreeMap<String, String>>,
    stored: BTreeMap<String, BTreeMap<String, String>>,
) -> BTreeMap<String, BTreeMap<String, String>> {
    let mut merged = stored;
    for (phone, apps) in known {
        let phone_apps = merged.entry(phone.clone()).or_default();
        for (package, name) in apps {
            phone_apps.insert(package.clone(), name.clone());
        }
    }
    merged
}

/// How an app's notifications reach this computer: the settings pane's three choices over the app's
/// feed flag (the phone sends its notifications, so they are listed) and banner flag (this computer
/// shows them as desktop notifications).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AppChoice {
    BannersAndList,
    ListOnly,
    Off,
}

impl AppChoice {
    /// A banner flag without the feed flag is Off: the phone sends nothing for the app then.
    pub fn of(flags: AppNotificationFlags) -> Self {
        match (flags.feed, flags.banner) {
            (true, true) => Self::BannersAndList,
            (true, false) => Self::ListOnly,
            (false, _) => Self::Off,
        }
    }

    pub fn parse(name: &str) -> Option<Self> {
        match name {
            "bannersAndList" => Some(Self::BannersAndList),
            "listOnly" => Some(Self::ListOnly),
            "off" => Some(Self::Off),
            _ => None,
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            Self::BannersAndList => "bannersAndList",
            Self::ListOnly => "listOnly",
            Self::Off => "off",
        }
    }

    /// The flags of this choice; the bits the phone reported beyond the two stay as they are.
    pub fn flags(self, current: AppNotificationFlags) -> AppNotificationFlags {
        let (feed, banner) = match self {
            Self::BannersAndList => (true, true),
            Self::ListOnly => (true, false),
            Self::Off => (false, false),
        };
        AppNotificationFlags { feed, banner, other_bits: current.other_bits }
    }
}

/// One app of the settings pane.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AppRow {
    pub package: String,
    pub name: String,
    /// What the engine keeps, or the user's choice while the phone answers it.
    pub flags: AppNotificationFlags,
    /// Its banners are muted until this time (Unix ms).
    pub muted_until_ms: Option<i64>,
    /// The user's change waits for the phone.
    pub pending: bool,
}

/// The pane's apps, by name (case ignored), then package.
pub fn app_rows_json(mut rows: Vec<AppRow>) -> Value {
    rows.sort_by(|a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase()).then_with(|| a.package.cmp(&b.package)));

    let rows: Vec<Value> = rows
        .into_iter()
        .map(|row| {
            json!({
                "package": row.package,
                "name": row.name,
                "choice": AppChoice::of(row.flags).name(),
                "mutedUntilMs": row.muted_until_ms,
                "pending": row.pending,
            })
        })
        .collect();
    Value::Array(rows)
}

/// The link the post-notification help shows as a code, which opens the setting on the phone; each
/// help carries a fresh trace id, `trace` as a lower-case version-4 GUID.
pub fn post_notification_link(trace: [u8; 16]) -> String {
    let mut bytes = trace;
    bytes[6] = (bytes[6] & 0x0f) | 0x40;
    bytes[8] = (bytes[8] & 0x3f) | 0x80;

    let hex: String = bytes.iter().map(|byte| format!("{byte:02x}")).collect();
    format!(
        "https://aka.ms/notification?src=1&traceId={}-{}-{}-{}-{}",
        &hex[0..8],
        &hex[8..12],
        &hex[12..16],
        &hex[16..20],
        &hex[20..32]
    )
}

#[cfg(test)]
mod tests;

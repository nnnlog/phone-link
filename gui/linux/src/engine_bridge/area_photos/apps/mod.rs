//! The Apps section's lists as the QML shows them: the tiles of the recent apps, the favorites and
//! every app, each with its app's notification count, and how a recent task carries over.

use std::collections::HashMap;

use phonelink_engine::api::notifications::PhoneNotification;
use phonelink_engine::api::phone_apps::{PhoneAppItem, RecentAppItem};
use serde_json::{Value, json};

/// The search is offered once the phone lists at least this many apps.
pub(super) const SEARCH_MINIMUM_APPS: usize = 5;

/// How a recent task carries over to this computer, for the mark on its tile: its document opens in an
/// application here (`desktop`) or in the browser (`browser`), or the task continues where the phone
/// left it (`task`); `None` for a recent app that is the app itself.
fn handoff_stamp(entry: &RecentAppItem) -> Option<&'static str> {
    let present = |uri: &Option<String>| uri.as_deref().is_some_and(|uri| !uri.is_empty());
    if present(&entry.desktop_uri) {
        Some("desktop")
    } else if present(&entry.web_uri) {
        Some("browser")
    } else if entry.main_activity {
        None
    } else {
        Some("task")
    }
}

/// The count on each app's tile: the notifications per package (ignoring case), leaving out the apps
/// whose notifications are off (`on` answers per package). Keyed by the lower-cased package.
pub(super) fn notification_counts(
    notifications: &[PhoneNotification],
    on: impl Fn(&str) -> bool,
) -> HashMap<String, usize> {
    let mut counts: HashMap<String, usize> = HashMap::new();
    let mut known: HashMap<String, bool> = HashMap::new();
    for notification in notifications.iter().filter(|n| !n.package_name.is_empty()) {
        let key = notification.package_name.to_lowercase();
        let app_on = *known.entry(key.clone()).or_insert_with(|| on(&notification.package_name));
        if app_on {
            *counts.entry(key).or_default() += 1;
        }
    }
    counts
}

fn count(counts: &HashMap<String, usize>, package: &str) -> usize {
    counts.get(&package.to_lowercase()).copied().unwrap_or(0)
}

/// The count on each recent tile, by recent id: a recent app that is the app itself shows its app's
/// count, a task none.
fn recent_counts(entries: &[RecentAppItem], counts: &HashMap<String, usize>) -> HashMap<i64, usize> {
    entries
        .iter()
        .map(|entry| (entry.recent_id, if entry.main_activity { count(counts, &entry.package_name) } else { 0 }))
        .collect()
}

pub(super) fn app_tile(app: &PhoneAppItem, counts: &HashMap<String, usize>) -> Value {
    json!({
        "package": app.package_name,
        "name": app.name,
        "hasIcon": app.has_icon,
        "favorite": app.favorite,
        "count": count(counts, &app.package_name),
    })
}

pub(super) fn recent_tile(entry: &RecentAppItem, count: usize) -> Value {
    json!({
        "recentId": entry.recent_id,
        "package": entry.package_name,
        "name": entry.name,
        "hasIcon": entry.has_icon,
        "mainActivity": entry.main_activity,
        "taskId": entry.task_id,
        "favorite": entry.favorite,
        "count": count,
        "stamp": handoff_stamp(entry),
        "desktop": entry.desktop_uri.as_deref().is_some_and(|uri| !uri.is_empty()),
        "browser": entry.web_uri.as_deref().is_some_and(|uri| !uri.is_empty()),
    })
}

/// Where a dropped app goes in the favorites: a favorite moves to `index` (the end when `None`), any
/// other app of the list joins them last; `None` when nothing changes.
#[derive(Debug, PartialEq, Eq)]
pub(super) enum FavoritesDrop {
    Move(Vec<String>),
    Add,
}

pub(super) fn drop_on_favorites(
    favorites: &[String],
    apps: &[String],
    package: &str,
    index: Option<usize>,
) -> Option<FavoritesDrop> {
    if favorites.iter().any(|favorite| favorite == package) {
        return index.and_then(|index| moved(favorites, package, index)).map(FavoritesDrop::Move);
    }
    apps.iter().any(|app| app == package).then_some(FavoritesDrop::Add)
}

/// The favorites with `package` moved to `index` (clamped to the list); `None` when it is not a
/// favorite or already there.
pub(super) fn moved(favorites: &[String], package: &str, index: usize) -> Option<Vec<String>> {
    let from = favorites.iter().position(|favorite| favorite == package)?;
    let target = index.min(favorites.len() - 1);
    if from == target {
        return None;
    }
    let mut order = favorites.to_vec();
    let item = order.remove(from);
    order.insert(target, item);
    Some(order)
}

/// What the section remembers between reads: the phone whose lists have shown (they first show once
/// the phone's lists are not behind, then follow every change), and the recent tiles' counts, taken
/// when the recent apps were last built.
#[derive(Debug, Default)]
pub(super) struct Memory {
    pub(super) lists_shown_for: Option<String>,
    recent_ids: Vec<i64>,
    recent_counts: HashMap<i64, usize>,
    /// The recent apps changed: their counts are taken again at the next read.
    pub(super) recent_stale: bool,
}

impl Memory {
    /// The recent tiles' counts: taken again when the recent apps were rebuilt, a different set shows,
    /// or `rebuild`.
    pub(super) fn recent_counts(
        &mut self,
        entries: &[RecentAppItem],
        counts: &HashMap<String, usize>,
        rebuild: bool,
    ) -> HashMap<i64, usize> {
        let ids: Vec<i64> = entries.iter().map(|entry| entry.recent_id).collect();
        if rebuild || self.recent_stale || ids != self.recent_ids {
            self.recent_counts = recent_counts(entries, counts);
            self.recent_ids = ids;
            self.recent_stale = false;
        }
        self.recent_counts.clone()
    }
}

#[cfg(test)]
mod tests;

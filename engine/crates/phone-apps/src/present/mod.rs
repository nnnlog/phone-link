//! The lists the UI shows, from the stored rows: *All apps* (the excluded packages left out, in the
//! order of the user's language), its search, the favorites and the recent apps.

use super::store::{AppRow, RecentRow};
use super::wire::{EXCLUDED_PACKAGES, MAIN_ACTION};

/// A package the lists never show (case-insensitive).
pub fn is_excluded(package: &str) -> bool {
    EXCLUDED_PACKAGES.iter().any(|p| p.eq_ignore_ascii_case(package))
}

/// A name as the lists show it: a line break becomes a space.
pub fn shown_name(name: &str) -> String {
    name.replace('\n', " ")
}

fn eq_ignore_case(a: &str, b: &str) -> bool {
    a.to_lowercase() == b.to_lowercase()
}

/// Every space-separated term of `query` occurs in `name`, ignoring case.
pub fn matches_search(name: &str, query: &str) -> bool {
    let name = name.to_lowercase();
    query.split(' ').filter(|t| !t.is_empty()).all(|t| name.contains(&t.to_lowercase()))
}

/// The rows of *All apps* before sorting: the excluded packages left out, in package-name order (the
/// order equal names keep).
pub fn listed(mut rows: Vec<AppRow>) -> Vec<AppRow> {
    rows.retain(|r| !is_excluded(&r.package_name));
    rows.sort_by(|a, b| a.package_name.cmp(&b.package_name));
    rows
}

/// `rows` in the order `order` names; `order` must hold every position of `rows` once.
pub fn reorder(rows: Vec<AppRow>, order: &[u32]) -> Result<Vec<AppRow>, &'static str> {
    if order.len() != rows.len() {
        return Err("the name order does not cover every app");
    }

    let mut slots: Vec<Option<AppRow>> = rows.into_iter().map(Some).collect();
    order
        .iter()
        .map(|&i| slots.get_mut(i as usize).and_then(Option::take).ok_or("the name order repeats or exceeds an app"))
        .collect()
}

/// The favorites of `all` (*All apps* in its order): rank `≥ 0`, by rank ascending.
pub fn favorites(all: &[AppRow]) -> Vec<AppRow> {
    let mut f: Vec<AppRow> = all.iter().filter(|a| a.favorite_rank >= 0).cloned().collect();
    f.sort_by_key(|a| a.favorite_rank);
    f
}

/// One recent app as the list shows it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RecentView {
    pub recent_id: i64,
    /// The package of its app.
    pub package_name: String,
    /// A main-activity entry shows its app's name, a task entry its own.
    pub name: String,
    /// No document URI, the app's shown name and the main intent action: launching it opens the app.
    pub main_activity: bool,
    /// A main-activity entry whose app is a favorite; a task entry never is.
    pub favorite: bool,
    pub task_id: i64,
    pub rank: i32,
    pub web_uri: Option<String>,
    pub desktop_uri: Option<String>,
    /// Its app's row (for its icon).
    pub app: AppRow,
}

/// The recent apps.
///
/// `rows` (stored entries with an app, by rank) joined to `all` by package (case-insensitive), the
/// duplicates — same id, or two main-activity entries of one package — shown once with the lower
/// rank, at most `max`.
pub fn recent(all: &[AppRow], rows: &[RecentRow], max: usize) -> Vec<RecentView> {
    let mut list: Vec<RecentView> = Vec::new();
    for row in rows {
        let Some(app) = all.iter().find(|a| eq_ignore_case(&a.package_name, &row.package_name)) else {
            continue;
        };

        let app_name = shown_name(&app.name);
        let main_activity = row.web_uri.as_deref().is_none_or(str::is_empty)
            && row.desktop_uri.as_deref().is_none_or(str::is_empty)
            && eq_ignore_case(&app_name, &row.name)
            && eq_ignore_case(&row.intent_action, MAIN_ACTION);

        let view = RecentView {
            recent_id: row.recent_id,
            package_name: app.package_name.clone(),
            name: if main_activity { app_name } else { row.name.clone() },
            main_activity,
            favorite: main_activity && app.favorite_rank >= 0,
            task_id: row.task_id,
            rank: row.rank,
            web_uri: row.web_uri.clone(),
            desktop_uri: row.desktop_uri.clone(),
            app: app.clone(),
        };
        upsert(&mut list, view);
    }

    list.truncate(max);
    list
}

/// Inserts `view` by rank; a duplicate with a rank no greater keeps its place, else it is replaced.
fn upsert(list: &mut Vec<RecentView>, view: RecentView) {
    let duplicate = list.iter().position(|v| {
        v.recent_id == view.recent_id
            || (view.main_activity
                && v.main_activity
                && !view.package_name.is_empty()
                && eq_ignore_case(&v.package_name, &view.package_name))
    });

    if let Some(i) = duplicate {
        if list[i].rank <= view.rank {
            return;
        }
        list.remove(i);
    }

    let at = list.iter().position(|v| view.rank < v.rank).unwrap_or(list.len());
    list.insert(at, view);
}

#[cfg(test)]
mod tests;

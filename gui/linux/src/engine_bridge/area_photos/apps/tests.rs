//! Unit tests of the Apps section's tiles, counts and favorites drops.

use super::*;
use phonelink_engine::api::notifications::NotificationOpenTarget;

fn notification(package: &str) -> PhoneNotification {
    PhoneNotification {
        key: format!("0|{package}|1"),
        package_name: package.to_owned(),
        app_name: None,
        title: None,
        subtitle: None,
        body: String::new(),
        conversation: Vec::new(),
        time_ms: 0,
        pinned: false,
        open_target: NotificationOpenTarget::Notifications,
        launch_supported: true,
        is_read_only_group_message: false,
        reply: None,
        actions: Vec::new(),
    }
}

fn recent(id: i64, package: &str, main: bool, desktop: Option<&str>, web: Option<&str>) -> RecentAppItem {
    RecentAppItem {
        recent_id: id,
        package_name: package.to_owned(),
        name: package.to_owned(),
        main_activity: main,
        favorite: false,
        task_id: id * 10,
        web_uri: web.map(str::to_owned),
        desktop_uri: desktop.map(str::to_owned),
        has_icon: true,
    }
}

fn strings(list: &[&str]) -> Vec<String> {
    list.iter().map(|s| (*s).to_owned()).collect()
}

#[test]
fn counts_ignore_case_and_apps_turned_off() {
    let list = [notification("com.Slack"), notification("com.slack"), notification("a.muted"), notification("")];
    let counts = notification_counts(&list, |package| package != "a.muted");
    assert_eq!(counts.get("com.slack"), Some(&2));
    assert_eq!(counts.get("a.muted"), None);
    assert_eq!(counts.len(), 1);
}

#[test]
fn stamps_say_how_a_task_carries_over() {
    assert_eq!(handoff_stamp(&recent(1, "a", false, Some("ms-word:x"), Some("https://x"))), Some("desktop"));
    assert_eq!(handoff_stamp(&recent(1, "a", false, Some(""), Some("https://x"))), Some("browser"));
    assert_eq!(handoff_stamp(&recent(1, "a", false, None, None)), Some("task"));
    assert_eq!(handoff_stamp(&recent(1, "a", true, None, None)), None);
}

#[test]
fn only_the_app_itself_shows_a_count_among_recent_apps() {
    let counts: HashMap<String, usize> = [("com.slack".to_owned(), 3)].into_iter().collect();
    let entries = [recent(1, "com.Slack", true, None, None), recent(2, "com.slack", false, None, None)];
    let shown = recent_counts(&entries, &counts);
    assert_eq!(shown.get(&1), Some(&3));
    assert_eq!(shown.get(&2), Some(&0));
}

#[test]
fn recent_counts_are_kept_until_the_recent_apps_change() {
    let mut memory = Memory::default();
    let entries = [recent(1, "a", true, None, None)];
    let one: HashMap<String, usize> = [("a".to_owned(), 1)].into_iter().collect();
    let two: HashMap<String, usize> = [("a".to_owned(), 2)].into_iter().collect();
    assert_eq!(memory.recent_counts(&entries, &one, false).get(&1), Some(&1));
    assert_eq!(memory.recent_counts(&entries, &two, false).get(&1), Some(&1));
    memory.recent_stale = true;
    assert_eq!(memory.recent_counts(&entries, &two, false).get(&1), Some(&2));
    let other = [recent(5, "a", true, None, None)];
    assert_eq!(memory.recent_counts(&other, &one, false).get(&5), Some(&1));
}

#[test]
fn drops_move_favorites_and_add_apps() {
    let favorites = strings(&["a", "b", "c"]);
    let apps = strings(&["a", "b", "c", "d"]);
    assert_eq!(
        drop_on_favorites(&favorites, &apps, "c", Some(0)),
        Some(FavoritesDrop::Move(strings(&["c", "a", "b"])))
    );
    assert_eq!(
        drop_on_favorites(&favorites, &apps, "a", Some(9)),
        Some(FavoritesDrop::Move(strings(&["b", "c", "a"])))
    );
    assert_eq!(drop_on_favorites(&favorites, &apps, "b", Some(1)), None);
    assert_eq!(drop_on_favorites(&favorites, &apps, "b", None), None);
    assert_eq!(drop_on_favorites(&favorites, &apps, "d", Some(0)), Some(FavoritesDrop::Add));
    assert_eq!(drop_on_favorites(&favorites, &apps, "gone", None), None);
}

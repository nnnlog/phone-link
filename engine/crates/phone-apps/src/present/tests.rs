use super::*;

fn app(id: i64, package: &str, name: &str, rank: i64) -> AppRow {
    AppRow {
        app_id: id,
        package_name: package.into(),
        name: name.into(),
        version: "1".into(),
        last_updated: 0,
        checksum: Some(0),
        has_icon: false,
        favorite_rank: rank,
    }
}

fn entry(id: i64, package: &str, name: &str, rank: i32) -> RecentRow {
    RecentRow {
        recent_id: id,
        package_name: package.into(),
        name: name.into(),
        checksum: Some(0),
        rank,
        intent_action: MAIN_ACTION.into(),
        task_id: id * 10,
        web_uri: None,
        desktop_uri: None,
    }
}

#[test]
fn excluded_packages_and_search() {
    let rows = vec![app(1, "com.b", "B", -1), app(2, "COM.MICROSOFT.APPMANAGER", "LTW", -1), app(3, "com.a", "A", -1)];
    let l = listed(rows);
    assert_eq!(l.iter().map(|r| r.package_name.as_str()).collect::<Vec<_>>(), ["com.a", "com.b"]);

    assert!(matches_search("Google Maps", "maps  goo"));
    assert!(!matches_search("Google Maps", "maps x"));
    assert!(matches_search("Anything", "   "));

    assert_eq!(shown_name("Two\nLines"), "Two Lines");
}

#[test]
fn reorder_needs_a_permutation() {
    let rows = vec![app(1, "a", "A", -1), app(2, "b", "B", -1)];
    assert_eq!(reorder(rows.clone(), &[1, 0]).unwrap()[0].app_id, 2);
    assert!(reorder(rows.clone(), &[0]).is_err());
    assert!(reorder(rows.clone(), &[0, 0]).is_err());
    assert!(reorder(rows, &[0, 2]).is_err());
}

#[test]
fn favorites_by_rank() {
    let all = vec![app(1, "a", "A", 30), app(2, "b", "B", -1), app(3, "c", "C", 10)];
    assert_eq!(favorites(&all).iter().map(|a| a.app_id).collect::<Vec<_>>(), [3, 1]);
}

#[test]
fn recent_entries_main_activity_tasks_and_duplicates() {
    let all = vec![app(1, "com.a", "App\nA", 5), app(2, "com.b", "B", 6), app(3, "com.c", "C", -1)];
    let mut task = entry(11, "com.b", "Document.docx", 1);
    task.web_uri = Some("https://x".into());
    let mut other_action = entry(13, "com.c", "C", 3);
    other_action.intent_action = "android.intent.action.VIEW".into();
    let rows = vec![
        entry(10, "COM.A", "app a", 0),
        task,
        entry(12, "com.a", "App A", 2),
        other_action,
        entry(14, "com.gone", "Gone", 4),
    ];

    let r = recent(&all, &rows, 10);
    assert_eq!(r.iter().map(|v| v.recent_id).collect::<Vec<_>>(), [10, 11, 13]);
    assert!(r[0].main_activity);
    assert!(r[0].favorite, "a main-activity entry carries its app's favorite");
    assert!(!r[1].favorite, "a task entry is never a favorite, whatever its app");
    assert!(!r[2].favorite);
    assert_eq!(r[0].name, "App A", "a main-activity entry shows its app's shown name");
    assert_eq!(r[0].package_name, "com.a");
    assert!(!r[1].main_activity);
    assert_eq!(r[1].name, "Document.docx");
    assert!(!r[2].main_activity, "another intent action is a task");

    assert_eq!(recent(&all, &rows, 3).len(), 3);
    assert_eq!(recent(&all, &rows, 2).len(), 2);

    // The same id twice: the lower rank stays.
    let rows = vec![entry(20, "com.b", "B", 5), entry(20, "com.b", "B", 1)];
    let r = recent(&all, &rows, 3);
    assert_eq!(r.len(), 1);
    assert_eq!(r[0].rank, 1);
}

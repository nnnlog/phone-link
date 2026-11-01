use super::*;
use phonelink_core::types::DcgClient;
use phonelink_features::shared::db::StoreLocation;

fn store() -> Store {
    let phone = DcgClient::new("phone-1", phonelink_core::types::Environment::Prod);
    Store::new(std::sync::Arc::new(FeatureDb::open_phone(&StoreLocation::Memory, &phone, FEATURE, MIGRATIONS).unwrap()))
}

fn row(id: i64, location: &str, dir: bool) -> Row {
    Row {
        id,
        uri: format!("content://media/external/file/{id}"),
        name: path::leaf(location).to_owned(),
        location: location.to_owned(),
        file_type: if dir { 7 } else { 1 },
        size: if dir { 0 } else { 10 },
        created: Some(1),
        modified: Some(2),
        checksum: id * 7,
        is_directory: dir,
    }
}

#[test]
fn synthetic_folders_fill_the_gaps() {
    let s = store();
    s.replace_all(&[row(1, "storage\\DCIM", true), row(2, "storage\\DCIM\\Camera\\a.jpg", false)], 10).unwrap();

    let (rows, folders) = s.children("").unwrap();
    assert!(rows.is_empty());
    assert_eq!(folders, vec!["storage".to_owned()]);

    let (rows, folders) = s.children("storage\\DCIM").unwrap();
    assert!(rows.is_empty());
    assert_eq!(folders, vec!["storage\\DCIM\\Camera".to_owned()]);

    let (rows, _) = s.children("storage\\DCIM\\Camera").unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(s.data().unwrap().sequence, 10);
}

#[test]
fn journal_reports_changes_and_removals() {
    let s = store();
    s.replace_all(&[row(1, "storage\\a.txt", false), row(2, "storage\\b.txt", false)], 1).unwrap();
    let (g, _) = s.generations().unwrap();

    assert!(!s.apply(&[row(1, "storage\\a.txt", false)], &[], Some(2)).unwrap());
    assert_eq!(s.generations().unwrap().0, g);

    s.apply(&[row(3, "storage\\c.txt", false)], &[2], Some(3)).unwrap();
    let c = s.changes_since(g).unwrap().unwrap();
    assert_eq!(c.rows.iter().map(|r| r.id).collect::<Vec<_>>(), vec![3]);
    assert_eq!(c.removed, vec!["2".to_owned()]);

    s.clear().unwrap();
    assert!(s.changes_since(g).unwrap().is_none());
}

#[test]
fn folder_move_carries_its_content() {
    let s = store();
    s.replace_all(&[row(1, "storage\\A", true), row(2, "storage\\A\\x.txt", false)], 1).unwrap();

    let mut moved = row(1, "storage\\B", true);
    moved.name = "B".into();
    s.apply(&[moved], &[], None).unwrap();

    assert_eq!(s.row(2).unwrap().unwrap().location, "storage\\B\\x.txt");
    assert!(s.folder_at("storage\\A").unwrap().is_none());
}

#[test]
fn a_folder_row_replaces_its_synthetic_folder() {
    let s = store();
    s.replace_all(&[row(2, "storage\\A\\x.txt", false)], 1).unwrap();
    assert!(s.folder_at("storage\\A").unwrap().is_some());

    let (g, _) = s.generations().unwrap();
    s.apply(&[row(1, "storage\\A", true)], &[], Some(2)).unwrap();
    assert!(s.folder_at("storage\\A").unwrap().is_none());

    let c = s.changes_since(g).unwrap().unwrap();
    assert!(c.removed.contains(&"dir:storage\\A".to_owned()));
    // The child is reported again: its parent changed.
    assert!(c.rows.iter().any(|r| r.id == 2));
}

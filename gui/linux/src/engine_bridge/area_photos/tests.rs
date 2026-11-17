//! Unit tests of what the Photos section and its grid show, and of the file actions' rules.

use phonelink_engine::api::photos::{Photo, PhotoAction, PhotoDeleteState, PhotosState};
use serde_json::{Value, json};

use super::State;
use super::action_file::{failure_title, hands_file_to_app};
use super::delete::delete_failure_title;
use super::gallery::{can_delete, content, photo_json, photo_key};

/// How far a photos state gets through the conditions the section checks in turn: each level also
/// meets those before it.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum Reached {
    Nothing,
    Enabled,
    Consent,
    Permission,
    DeleteAvailable,
}

fn state(reached: Reached) -> PhotosState {
    PhotosState {
        enabled: reached >= Reached::Enabled,
        consent: reached >= Reached::Consent,
        photos_permission: reached >= Reached::Permission,
        full_access_reminder: false,
        deletion_allowed: reached >= Reached::DeleteAvailable,
        delete_available: reached >= Reached::DeleteAvailable,
        new_photo_notifications: false,
        syncing: false,
        up_to_date: true,
        last_sync_ms: None,
    }
}

fn photo(id: i64, thumbnail: bool, full: bool, delete_state: PhotoDeleteState) -> Photo {
    Photo {
        id,
        name: format!("IMG_{id}.jpg"),
        taken_ms: 0,
        modified_ms: 7,
        mime_type: "image/jpeg".to_owned(),
        width: 4000,
        height: 3000,
        size: 1,
        has_thumbnail: thumbnail,
        has_full: full,
        delete_state,
    }
}

#[test]
fn the_first_unmet_condition_names_the_section_state() {
    assert_eq!(content(None, 3), "nothing");
    assert_eq!(content(Some(&state(Reached::Nothing)), 3), "unavailable");
    assert_eq!(content(Some(&state(Reached::Enabled)), 3), "turnedOff");
    assert_eq!(content(Some(&state(Reached::Consent)), 3), "needsPhoneAccess");
    assert_eq!(content(Some(&state(Reached::Permission)), 0), "empty");
    assert_eq!(content(Some(&state(Reached::Permission)), 3), "gallery");
}

#[test]
fn only_shown_photos_with_no_delete_under_way_can_be_deleted() {
    let available = state(Reached::DeleteAvailable);
    assert!(can_delete(Some(&available), &photo(1, true, false, PhotoDeleteState::None)));
    assert!(can_delete(Some(&available), &photo(1, false, true, PhotoDeleteState::None)));
    assert!(!can_delete(Some(&available), &photo(1, false, false, PhotoDeleteState::None)));
    assert!(!can_delete(Some(&available), &photo(1, true, true, PhotoDeleteState::Deleting)));
    assert!(!can_delete(Some(&state(Reached::Permission)), &photo(1, true, true, PhotoDeleteState::None)));
    assert!(!can_delete(None, &photo(1, true, true, PhotoDeleteState::None)));
}

#[test]
fn grid_items_carry_their_version_and_delete_state() {
    let item = photo_json(&photo(3, true, false, PhotoDeleteState::Deleting));
    assert_eq!(item["id"], 3);
    assert_eq!(item["modifiedMs"], 7);
    assert_eq!(item["deleteState"], "deleting");
    assert_eq!(photo_key(&photo(3, true, false, PhotoDeleteState::None)), "3-7");
}

#[test]
fn the_ui_state_lists_selection_and_busy_photos() {
    let mut ui = State::default();
    ui.selection.select(Some(4));
    ui.selection.toggle(2);
    ui.busy.insert(9, 1);
    ui.requests = 1;
    let json = ui.ui_json();
    assert_eq!(json["selected"], json!([2, 4]));
    assert_eq!(json["focus"], 2);
    assert_eq!(json["viewer"], Value::Null);
    assert_eq!(json["busy"], json!([9]));
    assert_eq!(json["requestPending"], true);
}

#[test]
fn handing_a_file_to_an_application_needs_a_photo_or_video() {
    assert!(hands_file_to_app(PhotoAction::Open));
    assert!(hands_file_to_app(PhotoAction::DragOut));
    assert!(!hands_file_to_app(PhotoAction::Copy));
    assert!(!hands_file_to_app(PhotoAction::SaveAs));
    assert_eq!(failure_title(PhotoAction::DragOut), "Couldn't Copy the Photo");
    assert_eq!(delete_failure_title(2), "Couldn't Delete the Photos");
}

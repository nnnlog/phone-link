use super::*;
use serde_json::{Value, json};

const ART: &str = "/9j/AA=="; // FF D8 FF 00
const ICON: &str = "/9j/AQ=="; // FF D8 FF 01

fn row(id: i64, fields: Value) -> MediaRow {
    let mut obj = json!({"key": format!("0|com.example.player|{id}|null|10123"), "packageName": "com.example.player", "postTime": 1});
    obj.as_object_mut().unwrap().extend(fields.as_object().unwrap().clone());
    let json = obj.to_string();
    MediaRow { row_id: id, item: NotificationItem::parse(&json).unwrap(), json }
}

fn media(id: i64, state: i32, title: &str) -> MediaRow {
    row(
        id,
        json!({"notificationClass": 2, "playbackState": state, "title": title, "appName": "Player", "text": "Artist"}),
    )
}

fn selected(s: &Sessions) -> Option<i64> {
    s.selected().map(|x| x.row_id)
}

#[test]
fn sessions_from_media_rows_only_in_arrival_order() {
    let mut s = Sessions::default();
    let ordinary = row(1, json!({"title": "Hi"}));

    assert!(s.apply(vec![media(5, 2, "B"), ordinary, media(3, 2, "A")]));
    let list = s.list();
    assert_eq!(list.iter().map(|x| x.row_id).collect::<Vec<_>>(), [3, 5]);
    assert_eq!(list[0].title.as_deref(), Some("A"));
    assert_eq!(list[0].artist.as_deref(), Some("Artist"));
    assert_eq!(list[0].key, "0|com.example.player|3|null|10123");

    assert!(!s.apply(vec![media(5, 2, "B"), media(3, 2, "A")]), "nothing re-sent: no change");
}

#[test]
fn fields_and_images() {
    let mut s = Sessions::default();
    s.apply(vec![row(1, json!({"notificationClass": 2, "largeIcon": ART, "smallIcon": ICON}))]);

    let x = &s.list()[0];
    assert_eq!(x.album_art.as_deref(), Some(&[0xFF, 0xD8, 0xFF, 0x00][..]));
    assert_eq!(x.app_icon.as_deref(), Some(&[0xFF, 0xD8, 0xFF, 0x01][..]));
    assert_eq!(x.playback_state, STATE_NONE, "absent playbackState → None");
    assert_eq!(x.app_name, None);

    let mut s = Sessions::default();
    s.apply(vec![row(1, json!({"notificationClass": 2, "largeIcon": "not base64!"}))]);
    assert_eq!(s.list()[0].album_art, None, "undecodable → placeholder");
}

#[test]
fn an_update_keeps_or_replaces_images() {
    let with_art = |title: &str, state: i32| {
        row(
            1,
            json!({"notificationClass": 2, "title": title, "playbackState": state, "largeIcon": ART, "smallIcon": ICON}),
        )
    };
    let without_art =
        |title: &str, state: i32| row(1, json!({"notificationClass": 2, "title": title, "playbackState": state}));

    let mut s = Sessions::default();
    s.apply(vec![with_art("Song", 3)]);

    // Same metadata, no images: the images stay; the state follows the item.
    assert!(s.apply(vec![without_art("Song", 2)]));
    assert!(s.list()[0].album_art.is_some() && s.list()[0].app_icon.is_some());
    assert_eq!(s.list()[0].playback_state, 2);

    // The title changed and no images came: both are cleared.
    assert!(s.apply(vec![without_art("Next song", 2)]));
    assert_eq!((s.list()[0].album_art.clone(), s.list()[0].app_icon.clone()), (None, None));

    // Images present: replaced.
    assert!(s.apply(vec![with_art("Next song", 2)]));
    assert!(s.list()[0].album_art.is_some());

    // An update without playbackState is None.
    s.apply(vec![row(1, json!({"notificationClass": 2, "title": "Next song", "x": 1}))]);
    assert_eq!(s.list()[0].playback_state, STATE_NONE);
}

#[test]
fn a_row_that_stops_being_media_style_still_updates_its_session() {
    let mut s = Sessions::default();
    s.apply(vec![media(1, 2, "A")]);

    assert!(s.apply(vec![row(1, json!({"notificationClass": 0, "title": "A2"}))]));
    assert_eq!(s.list()[0].title.as_deref(), Some("A2"));

    assert!(s.apply(vec![]), "its row is gone: removed");
    assert!(s.is_empty());
}

#[test]
fn selection_follows_added_and_playing_sessions() {
    let mut s = Sessions::default();
    assert_eq!(selected(&s), None);
    s.apply(vec![media(1, 2, "A")]);
    assert_eq!(selected(&s), Some(1), "the first session");

    // Added and not playing while the selection is not playing either: taken.
    s.apply(vec![media(1, 2, "A"), media(2, 2, "B")]);
    assert_eq!(selected(&s), Some(2));

    // Updated to playing: taken.
    s.apply(vec![media(1, 3, "A"), media(2, 2, "B")]);
    assert_eq!(selected(&s), Some(1));

    // Added, not playing, while the selection plays: not taken.
    s.apply(vec![media(1, 3, "A"), media(2, 2, "B"), media(3, 1, "C")]);
    assert_eq!(selected(&s), Some(1));

    // Added and playing: taken.
    s.apply(vec![media(1, 3, "A"), media(2, 2, "B"), media(3, 1, "C"), media(4, 3, "D")]);
    assert_eq!(selected(&s), Some(4));

    // The selection is removed: forgotten → the first session.
    s.apply(vec![media(2, 2, "B"), media(3, 1, "C"), media(1, 3, "A")]);
    assert_eq!(selected(&s), Some(1));
    s.apply(vec![media(2, 2, "B"), media(3, 1, "C")]);
    assert_eq!(selected(&s), Some(2));
}

#[test]
fn picking_a_paused_session_while_one_plays_plays_it() {
    let mut s = Sessions::default();
    s.apply(vec![media(1, 3, "A"), media(2, 2, "B"), media(3, 3, "C")]);
    assert_eq!(selected(&s), Some(3));

    let play = s.pick(2).expect("the previous selection played and the picked one does not");
    assert_eq!(play.row_id, 2);
    assert_eq!(selected(&s), Some(2));

    assert_eq!(s.pick(1), None, "the previous selection was not playing");
    assert_eq!(s.pick(3), None, "the picked one plays");
    assert_eq!(s.pick(9), None, "unknown row");
    assert_eq!(selected(&s), Some(3));
}

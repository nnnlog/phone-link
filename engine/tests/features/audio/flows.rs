//! Media controls: sessions from media-style notification batches, the gates, the selection, the
//! `/command/audio` requests and the busy indicator.

use crate::support::app_world;
use crate::support::common::payload;
use crate::support::notifications_world as support;
use phonelink_engine::config::features::AudioConfig;

use std::sync::Arc;
use std::time::Duration;

use serde_json::json;

use app_world::*;
use phonelink_engine::app::consts::capability;
use phonelink_engine::codec::valueset::Value;
use phonelink_engine::features::audio::{Audio, AudioEvent, MediaControls};
use support::*;

const ART: &str = "/9j/AA==";

struct AudioWorld {
    w: World,
    audio: Arc<Audio>,
}

/// A world whose phone reports `AudioControl` and grants notification access.
fn audio_world() -> AudioWorld {
    let w = world();
    w.h.app.update_phone_record(|r| r.capability_bits |= capability::AUDIO_CONTROL);
    w.h.messenger.answer_all(result_response(0));
    let audio = Audio::install(&w.w.env, AudioConfig::default(), w.n.clone()).expect("audio install");
    AudioWorld { w, audio }
}

fn media(key: &str, state: i32, title: &str) -> String {
    item(
        key,
        NOW_MS,
        json!({"notificationClass": 2, "playbackState": state, "appName": "Player", "title": title, "text": "Artist", "largeIcon": ART, "smallIcon": ART}),
    )
}

async fn send_items(aw: &AudioWorld, items: &[String]) {
    let keys: Vec<&str> = items.iter().map(|_| "k").collect();
    let ops: Vec<i32> = items.iter().map(|_| 1).collect();
    deliver(&aw.w.h, batch(&keys, &ops, items)).await;
    settle().await;
}

async fn remove(aw: &AudioWorld, key: &str) {
    deliver(&aw.w.h, batch(&[key], &[2], &[String::new()])).await;
    settle().await;
}

fn controls(aw: &AudioWorld) -> MediaControls {
    aw.audio.controls().unwrap()
}

fn titles(c: &MediaControls) -> Vec<String> {
    c.sessions.iter().map(|s| s.title.clone().unwrap_or_default()).collect()
}

fn selected_title(c: &MediaControls) -> Option<String> {
    let id = c.selected?;
    c.sessions.iter().find(|s| s.row_id == id).and_then(|s| s.title.clone())
}

fn commands(aw: &AudioWorld) -> Vec<(i32, String)> {
    aw.w.h
        .messenger
        .requests_on("/command/audio")
        .iter()
        .map(|r| {
            let vs = payload(r);
            (vs.i32("audioCommandValue").unwrap(), vs.str("key").unwrap().to_string())
        })
        .collect()
}

#[tokio::test(start_paused = true)]
async fn media_items_become_sessions_and_stay_out_of_the_list() {
    let aw = audio_world();
    let before = controls(&aw);
    assert!(before.supported && before.setting && before.active);
    assert!(!before.shown, "no session and no sync yet");

    let mut events = aw.audio.subscribe();
    send_items(&aw, &[media("0|player|1", 3, "Song"), alerting("0|chat|1")]).await;

    let c = controls(&aw);
    assert!(c.shown);
    assert_eq!(titles(&c), ["Song"]);
    let s = &c.sessions[0];
    assert_eq!((s.app_name.as_deref(), s.artist.as_deref(), s.playback_state), (Some("Player"), Some("Artist"), 3));
    assert_eq!(s.key, "0|player|1");
    assert_eq!(s.album_art.as_deref(), Some(&[0xFF, 0xD8, 0xFF, 0x00][..]));
    assert!(matches!(events.try_recv(), Ok(AudioEvent::Changed { .. })));
    assert_eq!(aw.w.n.list().unwrap().iter().map(|v| v.key.as_str()).collect::<Vec<_>>(), ["0|chat|1"]);

    remove(&aw, "0|player|1").await;
    let c = controls(&aw);
    assert!(c.sessions.is_empty() && !c.shown && c.active);
}

#[tokio::test(start_paused = true)]
async fn the_gates_drop_and_restore_the_sessions() {
    let aw = audio_world();
    send_items(&aw, &[media("0|player|1", 2, "Song")]).await;
    assert!(controls(&aw).shown);

    aw.audio.set_setting(false).unwrap();
    let c = controls(&aw);
    assert!(!c.setting && !c.active && !c.shown && c.sessions.is_empty());
    aw.audio.set_setting(true).unwrap();
    assert_eq!(titles(&controls(&aw)), ["Song"], "reloaded from the store");

    // Notification access withdrawn on the phone: off whatever the setting.
    deliver(&aw.w.h, device_permission_push(false)).await;
    settle().await;
    let c = controls(&aw);
    assert!(c.setting && !c.notifications_permission && !c.active && c.sessions.is_empty());
    deliver(&aw.w.h, device_permission_push(true)).await;
    settle().await;
    assert!(controls(&aw).shown);

    // A phone without AudioControl: not supported.
    aw.w.h.app.update_phone_record(|r| r.capability_bits &= !capability::AUDIO_CONTROL);
    settle().await;
    let c = controls(&aw);
    assert!(!c.supported && !c.active && c.sessions.is_empty());
}

#[tokio::test(start_paused = true)]
async fn buttons_send_commands_to_the_selected_session() {
    let aw = audio_world();
    send_items(&aw, &[media("0|player|1", 3, "A")]).await;
    aw.audio.play_pause();
    settle().await;

    send_items(&aw, &[media("0|player|1", 2, "A")]).await;
    aw.audio.play_pause();
    aw.audio.next();
    aw.audio.previous();
    settle().await;

    let key = "0|player|1".to_string();
    assert_eq!(commands(&aw), [(2, key.clone()), (1, key.clone()), (3, key.clone()), (4, key)]);

    let req = &aw.w.h.messenger.requests_on("/command/audio")[0];
    let vs = payload(req);
    assert_eq!(vs.get("isuserinitiated"), Some(&Value::Bool(false)));
    assert!(vs.get("contentType").is_none());
    assert_eq!(trace_names(req), ("PauseButtonPressed".to_string(), "SendAudioCommand".to_string()));
    assert!(trace_state(req).contains(&("isuserinitiated".to_string(), "True".to_string())));
}

#[tokio::test(start_paused = true)]
async fn nothing_is_sent_without_a_session_or_while_inactive() {
    let aw = audio_world();
    aw.audio.play_pause();
    send_items(&aw, &[media("0|player|1", 3, "A")]).await;
    aw.audio.set_setting(false).unwrap();
    aw.audio.next();
    settle().await;
    assert!(commands(&aw).is_empty());
}

#[tokio::test(start_paused = true)]
async fn picking_a_paused_session_while_one_plays_sends_play() {
    let aw = audio_world();
    send_items(&aw, &[media("0|player|1", 3, "A")]).await;
    send_items(&aw, &[media("0|radio|1", 2, "B")]).await;

    let c = controls(&aw);
    assert_eq!(titles(&c), ["A", "B"]);
    assert_eq!(selected_title(&c).as_deref(), Some("A"), "a paused session does not take a playing selection");

    let b = c.sessions[1].row_id;
    aw.audio.select(b).unwrap();
    settle().await;

    assert_eq!(selected_title(&controls(&aw)).as_deref(), Some("B"));
    assert_eq!(commands(&aw), [(1, "0|radio|1".to_string())]);
    let req = &aw.w.h.messenger.requests_on("/command/audio")[0];
    assert_eq!(trace_names(req).0, "SelectedAudioSessionIndexChanged");

    assert!(aw.audio.select(9_999).is_err(), "not a shown session");
}

#[tokio::test(start_paused = true)]
async fn the_busy_indicator_ends_after_20_s_or_on_a_session_change() {
    let aw = audio_world();
    send_items(&aw, &[media("0|player|1", 2, "A")]).await;
    aw.audio.play_pause();
    assert!(controls(&aw).commanding);

    settle().await;
    tokio::time::sleep(Duration::from_secs(19)).await;
    assert!(controls(&aw).commanding);

    tokio::time::sleep(Duration::from_secs(2)).await;
    settle().await;
    assert!(!controls(&aw).commanding, "20 s after the request finished");

    aw.audio.play_pause();
    settle().await;
    assert!(controls(&aw).commanding);

    send_items(&aw, &[media("0|player|1", 3, "A")]).await;
    let c = controls(&aw);
    assert!(!c.commanding, "the selected session changed to a state other than none");
    assert_eq!(c.sessions[0].playback_state, 3);
}

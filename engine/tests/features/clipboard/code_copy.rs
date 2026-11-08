//! The Copy action of a notification over the clipboard world: the code goes on the pasteboard as
//! transient plain text, is recorded as the app's own write, and is never announced to the phone.

use crate::support::clipboard_world as clip;
use crate::support::platform_world as fakes;

use std::collections::HashMap;

use clip::*;
use fakes::settle;
use phonelink_engine::features::NotificationResponse;
use phonelink_engine::features::clipboard::ClipboardPasteboardData;
use phonelink_engine::features::clipboard::pasteboard::uti;
use phonelink_engine::features::shared::notifier::copy;

fn rep(uti: &str, data: &[u8]) -> ClipboardPasteboardData {
    ClipboardPasteboardData { uti: uti.into(), data: data.to_vec() }
}

/// Presses Copy on a notification whose code is `code`, through the router as the app does.
async fn press_copy(w: &ClipWorld, code: &str) {
    let user_info = HashMap::from([(copy::CODE_KEY.to_string(), code.to_string())]);
    let press = NotificationResponse::Action { action_id: copy::ACTION_ID.to_string() };
    w.env.responses.route("messaging/phone/sms/1", &user_info, press);
    settle().await;
}

#[tokio::test(start_paused = true)]
async fn a_pressed_copy_writes_the_code_transient_and_announces_nothing() {
    let w = ClipWorld::new().await;
    w.toggle_on().await;
    w.env.responses.set_code_copier(w.clipboard.code_copier()).unwrap();

    press_copy(&w, "123456").await;

    assert_eq!(w.pasteboard.writes(), vec![vec![rep(uti::TRANSIENT, b""), rep(uti::PLAIN_TEXT, b"123456")]]);

    after_copy().await;
    assert!(w.announcements().is_empty(), "a transient pasteboard is not announced");
}

#[tokio::test(start_paused = true)]
async fn a_refused_marker_leaves_no_text() {
    let w = ClipWorld::new().await;
    w.env.responses.set_code_copier(w.clipboard.code_copier()).unwrap();
    w.pasteboard.refuse_type(Some(uti::TRANSIENT));

    press_copy(&w, "123456").await;

    assert_eq!(w.pasteboard.writes(), vec![Vec::new()]);
    assert!(w.pasteboard.contents().is_empty());
}

#[tokio::test(start_paused = true)]
async fn nothing_is_written_once_the_clipboard_shut_down() {
    let w = ClipWorld::new().await;
    w.env.responses.set_code_copier(w.clipboard.code_copier()).unwrap();
    w.clipboard.shutdown();

    press_copy(&w, "123456").await;

    assert!(w.pasteboard.writes().is_empty());
}

//! Unit tests of how the notifications part reads an app's command, words a failed action, and makes
//! a trace id.

use phonelink_engine::api::FeatureApiError;
use phonelink_engine::api::notifications::NotificationMuteDuration;

use super::app_commands::{app_request, mute_duration};
use super::failures::error_text;
use super::queries::trace_bytes;

#[test]
fn an_app_action_names_its_package_and_name() {
    let app = app_request("test", r#"{"package":"com.chat","name":"Chat","duration":"oneHour"}"#)
        .expect("the request names an app");
    assert_eq!((app.package.as_str(), app.name.as_str()), ("com.chat", "Chat"));
    assert_eq!(app.request["duration"].as_str().and_then(mute_duration), Some(NotificationMuteDuration::OneHour));
    assert!(app_request("test", r#"{"package":"com.chat"}"#).is_none());
    assert!(app_request("test", "com.chat").is_none());
}

#[test]
fn mute_durations_are_the_menus_three() {
    assert_eq!(mute_duration("eightHours"), Some(NotificationMuteDuration::EightHours));
    assert_eq!(mute_duration("oneDay"), Some(NotificationMuteDuration::OneDay));
    assert_eq!(mute_duration("forever"), None);
}

#[test]
fn an_engine_that_has_not_started_drops_the_action_silently() {
    assert_eq!(error_text(&FeatureApiError::NotStarted), None);
    assert!(error_text(&FeatureApiError::NoPhone).is_some());
}

#[test]
fn each_help_gets_its_own_trace_id() {
    assert_ne!(trace_bytes(), trace_bytes());
}

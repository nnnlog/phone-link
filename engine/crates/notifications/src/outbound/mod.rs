//! The PC's `/notifications` requests — their keys, operations and trace context names.
//!
//! It covers legacy requests with the default options — not user-initiated, priority `100`,
//! whatever triggered them — and a fresh correlation id per request, which is also the request's
//! trace correlation id.

use phonelink_app::consts::key as app_key;
use phonelink_app::{AppRequest, RequestOptions};
use phonelink_core::codec::valueset::{Value, ValueSet};
use phonelink_core::types::TraceContext;

use super::wire::{CONTENT_TYPE, key, operation, route};

/// One `/notifications` request's feature keys.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct NotificationsRequest {
    pub operation: i32,
    /// Operations 2 and 5; omitted when empty.
    pub key: Option<String>,
    /// Operation 5 with an action.
    pub action_index: Option<i32>,
    /// Operation 5 reply; omitted when empty.
    pub inline_reply_message: Option<String>,
    /// Operations 3 and 4.
    pub keys: Vec<String>,
    /// Operation 3, per key in the same order.
    pub post_times: Vec<i64>,
}

impl NotificationsRequest {
    /// Operation 3: every stored key, newest first, with its `postTime`.
    pub fn full_sync(keys: Vec<String>, post_times: Vec<i64>) -> Self {
        Self { operation: operation::EXISTING_NOTIFICATION, keys, post_times, ..Self::default() }
    }

    /// Operation 2: dismiss `key`.
    pub fn dismiss(key: &str) -> Self {
        Self { operation: operation::REMOVE_NOTIFICATION, key: Some(key.to_string()), ..Self::default() }
    }

    /// Operation 4: the cleared keys that are still stored.
    pub fn clear_all(keys: Vec<String>) -> Self {
        Self { operation: operation::CLEAR_ALL_NOTIFICATIONS, keys, ..Self::default() }
    }

    /// Operation 5 with `actionIndex`, and `inlineReplyMessage` for a reply.
    pub fn launch_action(key: &str, action_index: i32, reply: Option<&str>) -> Self {
        Self {
            operation: operation::LAUNCH_NOTIFICATION,
            key: Some(key.to_string()),
            action_index: Some(action_index),
            inline_reply_message: reply.map(str::to_string),
            ..Self::default()
        }
    }

    /// Operation 5 "open on phone": `key`, and `actionIndex` only when `>= 0` (the body's own rule).
    pub fn open_on_phone(key: &str, action_index: i32) -> Self {
        Self {
            operation: operation::LAUNCH_NOTIFICATION,
            key: Some(key.to_string()),
            action_index: Some(action_index),
            ..Self::default()
        }
    }

    /// The "open on phone" request, sent while the phone-screen window is connected: default
    /// options, traced as user-initiated, `NotificationsAppLaunch` / `Notifications`.
    pub fn app_request_user_traced(&self) -> AppRequest {
        let trace = TraceContext::new_root(Vec::new());
        let payload = self.body(&trace.correlation_id);
        AppRequest::legacy(route::NOTIFICATIONS, payload)
            .with_options(
                RequestOptions::default().named("NotificationsAppLaunch", "Notifications").traced_as_user_initiated(),
            )
            .with_trace(trace)
    }

    /// The feature keys in wire order: `correlationVector`, `contentType`, `operation`, `key`,
    /// `actionIndex`, `inlineReplyMessage`, `notificationKeysCount`, `notificationKeys`,
    /// `postTimes`.
    pub fn body(&self, correlation_vector: &str) -> ValueSet {
        let mut vs = ValueSet::new()
            .with(app_key::CORRELATION_VECTOR, correlation_vector.to_string())
            .with(app_key::CONTENT_TYPE, CONTENT_TYPE)
            .with(key::OPERATION, self.operation);

        if let Some(k) = self.key.as_deref().filter(|k| !k.is_empty()) {
            vs.insert(key::KEY, k.to_string());
        }
        if self.operation == operation::LAUNCH_NOTIFICATION {
            if let Some(i) = self.action_index.filter(|i| *i >= 0) {
                vs.insert(key::ACTION_INDEX, i);
            }
        }
        if let Some(m) = self.inline_reply_message.as_deref().filter(|m| !m.is_empty()) {
            vs.insert(key::INLINE_REPLY_MESSAGE, m.to_string());
        }
        if self.operation == operation::EXISTING_NOTIFICATION || self.operation == operation::CLEAR_ALL_NOTIFICATIONS {
            let count = i32::try_from(self.keys.len()).expect("fewer than 2^31 stored notifications");
            vs.insert(key::NOTIFICATION_KEYS_COUNT, count);
            vs.insert(key::NOTIFICATION_KEYS, Value::StringArray(self.keys.clone()));
            if self.operation == operation::EXISTING_NOTIFICATION {
                vs.insert(key::POST_TIMES, Value::Int64Array(self.post_times.clone()));
            }
        }

        vs
    }

    /// The App request: a legacy request on `/notifications`, default options, traced with
    /// `trigger` / `scenario`; `correlationVector` = the fresh trace context's correlation id.
    pub fn app_request(&self, trigger: &str, scenario: &str) -> AppRequest {
        let trace = TraceContext::new_root(Vec::new());
        let payload = self.body(&trace.correlation_id);
        AppRequest::legacy(route::NOTIFICATIONS, payload)
            .with_options(RequestOptions::default().named(trigger, scenario))
            .with_trace(trace)
    }
}

/// The settings request on `/settings`: a legacy request, user-initiated per its trigger.
pub fn settings_request(
    body: impl FnOnce(&str) -> ValueSet,
    user_initiated: bool,
    trigger: &str,
    scenario: &str,
) -> AppRequest {
    let trace = TraceContext::new_root(Vec::new());
    let payload = body(&trace.correlation_id);
    let options = if user_initiated { RequestOptions::user_initiated() } else { RequestOptions::default() };
    AppRequest::legacy(route::SETTINGS, payload).with_options(options.named(trigger, scenario)).with_trace(trace)
}

#[cfg(test)]
mod tests;

//! The Microsoft account sign-in in the app's web view: the script every page gets, and what the
//! view does next for the page's console messages, loads and script results — answer the page's
//! `CloudExperienceHost.getContext` call, read the end page's `ServerData`, and complete the sign-in
//! once with its result.

use std::collections::HashMap;

use phonelink_engine::edge::WebSignInKind;
use serde_json::{Value, json};

const MESSAGE_PREFIX: &str = "phonelink:";

/// The capabilities `CloudExperienceHost` reports to the page (matches the engine's `cxh-capabilities`).
const CXH_CAPABILITIES: &str =
    r#"{"PrivatePropertyBag":1,"PasswordlessConnect":1,"PreferAssociate":1,"ChromelessUI":0}"#;

/// `QtWebEngine` has no `WebKit` message handler: the engine's bridge script posts to
/// `window.webkit.messageHandlers.phonelink`, which this shim turns into a console message.
const WEBKIT_SHIM: &str = r#"window.webkit = { messageHandlers: { phonelink: { postMessage: function (m) { console.log("phonelink:" + (typeof m === "string" ? m : JSON.stringify(m))); } } } };"#;

/// The script the view runs in every page: the shim, then the engine's bridge script.
pub fn page_script(bridge_script: &str) -> String {
    format!("{WEBKIT_SHIM}\n{bridge_script}")
}

/// What the view does next.
#[derive(Debug, PartialEq, Eq)]
pub enum Step {
    Nothing,
    RunScript(String),
    /// The sign-in ended with this result, as JSON text.
    Complete(String),
}

/// One sign-in page's progress, from its first load to its result.
pub struct SignInPage {
    pub request_id: String,
    /// The Microsoft sign-in, or a consent page the token exchange asked for.
    pub kind: WebSignInKind,
    /// The page the view loads first, and the headers that first request carries.
    pub url: String,
    pub headers: Vec<(String, String)>,
    end_url_prefix: Option<String>,
    awaiting_end_page: bool,
    done: bool,
}

impl SignInPage {
    pub fn new(
        request_id: String,
        kind: WebSignInKind,
        url: String,
        headers: HashMap<String, String>,
        end_url_prefix: Option<String>,
    ) -> Self {
        let mut headers: Vec<(String, String)> = headers.into_iter().collect();
        headers.sort();
        Self { request_id, kind, url, headers, end_url_prefix, awaiting_end_page: false, done: false }
    }

    /// A console message of the page; the bridge script's messages start with `phonelink:`.
    pub fn console_message(&mut self, message: &str) -> Step {
        let Some(text) = message.strip_prefix(MESSAGE_PREFIX) else {
            return Step::Nothing;
        };
        if self.done {
            return Step::Nothing;
        }
        let Ok(json) = serde_json::from_str::<Value>(text) else {
            return Step::Nothing;
        };
        if let Some(context) = get_context_request(&json) {
            return Step::RunScript(get_context_answer(context));
        }
        if contains_sign_in_result(&json) {
            self.done = true;
            return Step::Complete(text.to_owned());
        }
        Step::Nothing
    }

    /// The view finished loading `url`.
    pub fn page_loaded(&mut self, url: &str) -> Step {
        if self.done {
            return Step::Nothing;
        }
        if self.end_url_prefix.as_deref().is_some_and(|end| url.starts_with(end)) {
            self.awaiting_end_page = true;
            return Step::RunScript(END_PAGE_SCRIPT.to_owned());
        }
        if url.contains("post.srf") {
            return Step::RunScript(
                "window.ServerData ? window.external.notify(JSON.stringify(window.ServerData)) : 0".to_owned(),
            );
        }
        Step::Nothing
    }

    /// The result of a script the page ran. Only the end page's script answers with its tag (every
    /// script's result comes back the same way, so a late result of an earlier script must not end the
    /// sign-in): its `ServerData` when that holds the sign-in result, else `{}`.
    pub fn script_result(&mut self, result: &str) -> Step {
        if self.done || !self.awaiting_end_page {
            return Step::Nothing;
        }
        let Some(server_data) = end_page_result(result) else { return Step::Nothing };
        self.done = true;

        match server_data {
            Some(text) if serde_json::from_str::<Value>(&text).is_ok_and(|json| contains_sign_in_result(&json)) => {
                Step::Complete(text)
            }
            _ => Step::Complete("{}".to_owned()),
        }
    }
}

/// The end page's script: its `ServerData` as JSON text (`null` without it), under [`END_PAGE_TAG`].
const END_PAGE_SCRIPT: &str =
    "JSON.stringify({ phonelinkEndPage: window.ServerData ? JSON.stringify(window.ServerData) : null })";
const END_PAGE_TAG: &str = "phonelinkEndPage";

/// The `ServerData` text an end-page script result carries (`Some(None)` for none); `None` for any other
/// script's result.
fn end_page_result(result: &str) -> Option<Option<String>> {
    let json = serde_json::from_str::<Value>(result).ok()?;

    match json.as_object()?.get(END_PAGE_TAG)? {
        Value::String(text) => Some(Some(text.clone())),
        Value::Null => Some(None),
        _ => None,
    }
}

fn get_context_request(json: &Value) -> Option<&str> {
    let value = json.get("value")?;
    let is_get_context =
        json.get("type")?.as_str()? == "invoke" && value.get("name")?.as_str()? == "CloudExperienceHost.getContext";
    is_get_context.then(|| value.get("context").and_then(Value::as_str).unwrap_or_default())
}

fn get_context_answer(context: &str) -> String {
    let callback = json!({
        "type": "callback",
        "value": {
            "name": "CloudExperienceHost.getContext",
            "args": ["CloudExperienceHost", "TokenBroker", "TokenBroker", CXH_CAPABILITIES],
            "context": context,
        },
    });
    let literal = serde_json::to_string(&callback.to_string()).expect("a string serializes");
    format!(
        r#"if (window["CloudExperienceHost.Bridge.dispatchMessage"]) window["CloudExperienceHost.Bridge.dispatchMessage"]({literal})"#
    )
}

fn contains_sign_in_result(json: &Value) -> bool {
    let Some(object) = json.as_object() else {
        return false;
    };
    (object.contains_key("sDAToken") && object.contains_key("K")) || object.values().any(contains_sign_in_result)
}

#[cfg(test)]
mod tests;

use serde_json::{Value, json};

const MESSAGE_PREFIX: &str = "phonelink:";

/// The capabilities CloudExperienceHost reports to the page (matches the engine's `cxh-capabilities`).
const CXH_CAPABILITIES: &str =
    r#"{"PrivatePropertyBag":1,"PasswordlessConnect":1,"PreferAssociate":1,"ChromelessUI":0}"#;

/// QtWebEngine has no WebKit message handler: the engine's bridge script posts to
/// `window.webkit.messageHandlers.phonelink`, which this shim turns into a console message.
const WEBKIT_SHIM: &str = r#"window.webkit = { messageHandlers: { phonelink: { postMessage: function (m) { console.log("phonelink:" + (typeof m === "string" ? m : JSON.stringify(m))); } } } };"#;

pub fn page_script(bridge_script: &str) -> String {
    format!("{WEBKIT_SHIM}\n{bridge_script}")
}

#[derive(Debug, PartialEq, Eq)]
pub enum Step {
    Nothing,
    RunScript(String),
    Complete(String),
}

pub struct SignInPage {
    pub request_id: String,
    end_url_prefix: Option<String>,
    awaiting_end_page: bool,
    done: bool,
}

impl SignInPage {
    pub fn new(request_id: String, end_url_prefix: Option<String>) -> Self {
        Self { request_id, end_url_prefix, awaiting_end_page: false, done: false }
    }

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

    pub fn page_loaded(&mut self, url: &str) -> Step {
        if self.done {
            return Step::Nothing;
        }
        if self.end_url_prefix.as_deref().is_some_and(|end| url.starts_with(end)) {
            self.awaiting_end_page = true;
            return Step::RunScript("window.ServerData ? JSON.stringify(window.ServerData) : null".to_owned());
        }
        if url.contains("post.srf") {
            return Step::RunScript(
                "window.ServerData ? window.external.notify(JSON.stringify(window.ServerData)) : 0".to_owned(),
            );
        }
        Step::Nothing
    }

    pub fn script_result(&mut self, result: &str) -> Step {
        if self.done || !self.awaiting_end_page {
            return Step::Nothing;
        }
        self.done = true;
        match serde_json::from_str::<Value>(result) {
            Ok(json) if contains_sign_in_result(&json) => Step::Complete(result.to_owned()),
            _ => Step::Complete("{}".to_owned()),
        }
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
mod tests {
    use super::*;

    fn page() -> SignInPage {
        SignInPage::new("r1".to_owned(), Some("https://end/".to_owned()))
    }

    #[test]
    fn ignores_console_messages_without_the_prefix() {
        assert_eq!(page().console_message(r#"{"sDAToken":1,"K":2}"#), Step::Nothing);
    }

    #[test]
    fn answers_get_context_with_its_context() {
        let message = r#"phonelink:{"type":"invoke","value":{"name":"CloudExperienceHost.getContext","context":"c7"}}"#;
        let Step::RunScript(script) = page().console_message(message) else { panic!("no script") };
        assert!(script.contains("dispatchMessage"));
        assert!(script.contains(r#"\"context\":\"c7\""#));
    }

    #[test]
    fn completes_once_with_a_nested_result() {
        let mut page = page();
        let message = r#"phonelink:{"data":{"sDAToken":"t","K":"k"}}"#;
        assert_eq!(page.console_message(message), Step::Complete(r#"{"data":{"sDAToken":"t","K":"k"}}"#.to_owned()));
        assert_eq!(page.console_message(message), Step::Nothing);
    }

    #[test]
    fn end_page_without_a_result_completes_empty() {
        let mut page = page();
        assert!(matches!(page.page_loaded("https://end/done"), Step::RunScript(_)));
        assert_eq!(page.script_result(""), Step::Complete("{}".to_owned()));
    }

    #[test]
    fn post_srf_script_result_is_ignored() {
        let mut page = page();
        assert!(matches!(page.page_loaded("https://login.live.com/ppsecure/post.srf"), Step::RunScript(_)));
        assert_eq!(page.script_result("0"), Step::Nothing);
    }
}

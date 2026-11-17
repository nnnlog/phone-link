use super::*;

fn page() -> SignInPage {
    SignInPage::new(
        "r1".to_owned(),
        WebSignInKind::Consent,
        "https://start/".to_owned(),
        HashMap::new(),
        Some("https://end/".to_owned()),
    )
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
    assert_eq!(page.script_result(r#"{"phonelinkEndPage":null}"#), Step::Complete("{}".to_owned()));
}

#[test]
fn end_page_completes_with_its_server_data() {
    let mut page = page();
    assert!(matches!(page.page_loaded("https://end/done"), Step::RunScript(_)));
    let result = json!({ END_PAGE_TAG: r#"{"sDAToken":"t","K":"k"}"# }).to_string();
    assert_eq!(page.script_result(&result), Step::Complete(r#"{"sDAToken":"t","K":"k"}"#.to_owned()));
    assert_eq!(page.script_result(&result), Step::Nothing);
}

#[test]
fn a_late_result_of_an_earlier_script_does_not_end_the_sign_in() {
    let mut page = page();
    assert!(matches!(page.page_loaded("https://end/done"), Step::RunScript(_)));
    assert_eq!(page.script_result("0"), Step::Nothing);
    assert_eq!(page.script_result(""), Step::Nothing);
    assert_eq!(page.script_result(r#"{"phonelinkEndPage":null}"#), Step::Complete("{}".to_owned()));
}

#[test]
fn post_srf_script_result_is_ignored() {
    let mut page = page();
    assert!(matches!(page.page_loaded("https://login.live.com/ppsecure/post.srf"), Step::RunScript(_)));
    assert_eq!(page.script_result("0"), Step::Nothing);
}

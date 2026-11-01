use super::*;

#[test]
fn urls_follow_the_library_rules() {
    let hub = Url::parse("https://dcg.microsoft.com/relayhub/").unwrap();
    assert_eq!(negotiate_url(&hub).as_str(), "https://dcg.microsoft.com/relayhub/negotiate?negotiateVersion=1");
    assert_eq!(transport_url(&hub, "a/b+c").unwrap().as_str(), "wss://dcg.microsoft.com/relayhub/?id=a%2Fb%2Bc");

    let azure = Url::parse("https://x.service.signalr.net/client/?hub=relayhub").unwrap();
    assert_eq!(
        negotiate_url(&azure).as_str(),
        "https://x.service.signalr.net/client/negotiate?hub=relayhub&negotiateVersion=1"
    );

    let local = Url::parse("http://127.0.0.1:9/relayhub/").unwrap();
    assert_eq!(transport_url(&local, "t").unwrap().as_str(), "ws://127.0.0.1:9/relayhub/?id=t");
}

#[test]
fn redirect_tokens_are_redacted_in_the_tap() {
    let body = br#"{"url":"https://x/","accessToken":"secret"}"#;
    assert_eq!(body_json(body)["accessToken"], "<redacted 6 bytes>");
}

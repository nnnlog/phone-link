use super::*;

#[test]
fn defaults_follow_the_spec() {
    let c = EngineConfig::default();
    assert_eq!(c.identity.profile.client_type, "WEA");
    assert_eq!(c.endpoints.hub_url(Environment::Prod), "https://dcg.microsoft.com/relayhub/");
    assert_eq!(c.dcg.fragment_size, 512 * 1024 * 1024);
    assert_eq!(c.relay.reconnect_delays.len(), 7);
    assert_eq!(display_name_from_host("my-mac.local"), "my-mac");
}

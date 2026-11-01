use super::testing::StaticPeers;
use super::*;
use phonelink_core::types::Environment;

fn peer(id: &str, client_type: &str, os: &str) -> PeerDevice {
    PeerDevice {
        client: DcgClient::new(id.to_string(), Environment::Prod),
        info: DeviceInfoItem {
            id: id.to_string(),
            client_type: Some(client_type.to_string()),
            os_name: Some(os.to_string()),
            ..Default::default()
        },
    }
}

#[tokio::test]
async fn only_the_displayed_phone_and_the_other_pcs_are_shown() {
    let (a, b, pc) = (peer("a", "LTW", "Android"), peer("b", "LTW", "Android"), peer("pc", "PL", "Windows"));
    let inner = StaticPeers::new(vec![a.clone(), b.clone(), pc.clone()]);
    let view = DisplayedPeers::new(inner.clone());
    let ids = |v: &DisplayedPeers| v.peers().into_iter().map(|p| p.client.id).collect::<Vec<_>>();
    assert_eq!(ids(&view), ["pc"], "no phone before one is displayed");

    let mut events = view.subscribe();
    view.set_displayed(Some(a.client.clone()));
    assert_eq!(ids(&view), ["a", "pc"]);
    assert!(matches!(events.recv().await, Ok(PeerEvent::Added(p)) if p.client.id == "a"));

    view.set_displayed(Some(b.client.clone()));
    assert_eq!(ids(&view), ["b", "pc"]);
    assert!(matches!(events.recv().await, Ok(PeerEvent::Removed(c)) if c.id == "a"));
    assert!(matches!(events.recv().await, Ok(PeerEvent::Added(p)) if p.client.id == "b"));

    // A refresh's event for a phone that is not displayed does not reach the view's subscribers.
    inner.upsert(peer("a", "LTW", "Android"));
    inner.upsert(peer("b", "LTW", "Android"));
    assert!(matches!(events.recv().await, Ok(PeerEvent::Updated(p)) if p.client.id == "b"));
}

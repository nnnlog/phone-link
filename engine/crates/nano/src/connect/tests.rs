use super::*;
use crate::cloud_relay::{NoRelay, RelayServers, RelayServersError};

struct Fixed(RelayServers);

#[async_trait]
impl RelaySource for Fixed {
    async fn relay_servers(&self, _only_stun: bool) -> Result<RelayServers, RelayServersError> {
        Ok(self.0.clone())
    }
}

#[tokio::test]
async fn mirroring_defaults_race_ip_and_ice() {
    let phone = PhoneRemoting { flags: 0x1EFFF, contract_version: 3.99 };
    let a = mirroring_attempts(&NanoConfig::default(), phone, &NoRelay).await;
    let t: Vec<i32> = a.iter().map(|a| a.transport).collect();
    assert_eq!(t, vec![0, 1]);
    assert!(a[0].first && !a[1].first && a.iter().all(|a| a.multiple_connections));

    let old = PhoneRemoting { flags: 0, contract_version: 3.99 };
    let a = mirroring_attempts(&NanoConfig::default(), old, &NoRelay).await;
    assert_eq!((a.len(), a[0].multiple_connections), (1, false));

    let config = NanoConfig {
        mirroring_stun_servers: "stun.a:3478, stun.b".into(),
        mirroring_attempts: 4 | 1,
        ..Default::default()
    };
    let a = mirroring_attempts(&config, phone, &NoRelay).await;
    assert_eq!(a.iter().map(|a| a.transport).collect::<Vec<_>>(), vec![0, 2]);
    assert_eq!(a[1].servers.stun_url, "stun.a:3478");
}

#[tokio::test]
async fn device_attempts_need_stun_and_turn() {
    assert_eq!(device_attempts(&NoRelay).await.len(), 2);

    let relay = Fixed(RelayServers {
        stun_urls: vec!["s1".into(), "s2".into()],
        turn_urls: vec!["t1".into(), "t2".into()],
        turn_username: "u".into(),
        turn_credential: "c".into(),
        turn_expire_time: None,
    });

    let a = device_attempts(&relay).await;
    assert_eq!(a.iter().map(|a| a.transport).collect::<Vec<_>>(), vec![0, 1, 2, 3]);
    assert_eq!(a[2].servers.turn_url, "", "IceStun drops TURN");
    assert_eq!((a[3].servers.stun_url.as_str(), a[3].servers.turn_url.as_str()), ("s1 s2", "t1 t2"));
}

#[test]
fn context_offer_wrapper_keys() {
    let attempt = Attempt { first: true, ..Attempt::new(1, ConnectionType::IceSrtp, IceServers::default()) };
    let wrapper: Value = serde_json::from_str(&ContextExchange::wrapper(&attempt, Map::new())).unwrap();

    assert_eq!(wrapper["connectiontype"], 1);
    assert_eq!(wrapper["multipleconnections"], true);
    assert_eq!(wrapper["connectionid"], attempt.connection_id.as_str());
}

use super::*;

#[test]
fn parses_and_caches_by_lifetime() {
    let now = SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_secs() as i64;
    let v = serde_json::json!({"stunInfo":{"urls":["stun.example:3478"]},
            "turnCredential":{"urls":["turn.example:3478"],"username":"u","credential":"c","turnExpireTime": now + 1000}});

    let s = RelayServers::from_json(&v).unwrap();
    assert!(s.has_stun_and_turn());

    let l = CloudRelay::lifetime(false, &s).unwrap();
    assert!(l > Duration::from_secs(800) && l <= Duration::from_secs(850));

    let expired = RelayServers { turn_expire_time: Some(now - 1), ..s };
    assert_eq!(CloudRelay::lifetime(false, &expired), None);
    assert_eq!(CloudRelay::lifetime(true, &expired), Some(STUN_ONLY_LIFETIME));

    assert!(!RelayServers::from_json(&serde_json::json!({"stunInfo":{"urls":[]}})).unwrap().has_stun_and_turn());
}

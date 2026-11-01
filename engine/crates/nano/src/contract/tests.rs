use super::*;
use crate::ice::candidate::{Candidate, CandidateType};
use serde_json::json;

#[test]
fn server_entries() {
    let p = |s| ServerEntry::parse(s, 3478);
    assert_eq!(p("stun.example.com"), ServerEntry { host: "stun.example.com".into(), port: 3478 });
    assert_eq!(p("1.2.3.4:19302"), ServerEntry { host: "1.2.3.4".into(), port: 19302 });
    assert_eq!(p("[2001:db8::1]:5000"), ServerEntry { host: "2001:db8::1".into(), port: 5000 });

    // A URI is a host name as a whole.
    assert_eq!(p("stun:host:3478").host, "stun:host:3478");
    assert_eq!(p("turn:host:3478?transport=udp").host, "turn:host:3478?transport=udp");

    let servers = IceServers { stun_url: " a:1  b ".into(), turn_url: "t1 t2".into(), ..Default::default() };
    assert_eq!(servers.stun_servers().len(), 2);
    assert_eq!(servers.turn_server().unwrap().host, "t1 t2", "the TURN string is not split");
}

#[test]
fn ice_offer_shape() {
    let base: SocketAddr = "192.168.1.20:51000".parse().unwrap();
    let offer = IceSrtpOffer {
        key: MasterKey { key: [0x11; 16], salt: [0xAB; 14] },
        description: SessionDescription::local(
            "u".into(),
            "p".into(),
            vec![Candidate::local(CandidateType::Host, base, base, None, 65_535)],
        ),
        servers: IceServers::default(),
    };

    let v = Value::Object(offer.to_json());

    assert_eq!(v["security"]["key"], "11".repeat(16));
    assert_eq!(v["security"]["salt"], "ab".repeat(14));
    assert_eq!(v["ice"]["turn"], json!({"url":"","username":"","password":""}));
    assert!(v["ice"]["candidates"].is_string());
}

#[test]
fn ip_answer_selects_wlan() {
    let answer = json!({
            "security":{"key":"00112233445566778899AABBCCDDEEFF","salt":"0102030405060708090a0b0c0d0e"},
            "ip":{"count":"3","0":{"iname":"rmnet0","addr":"10.0.0.1"},"1":{"iname":"wlan1","addr":"192.168.0.9"},
                  "2":{"iname":"wlan0","addr":"192.168.1.7"}},
            "port":"40000","keepalive":{"warningtimeout":"5000"},"tbst":{"wifiIp":"x"}});

    let a = IpSrtpAnswer::parse(&answer).unwrap();
    assert_eq!(a.select_address().unwrap(), "192.168.1.7:40000".parse().unwrap());
    assert_eq!(a.warning_timeout, Duration::from_secs(5));
    assert_eq!(a.key.key[15], 0xFF);

    let mut no_wlan0 = answer.clone();
    no_wlan0["ip"]["2"]["iname"] = json!("eth0");
    assert_eq!(IpSrtpAnswer::parse(&no_wlan0).unwrap().select_address().unwrap().ip().to_string(), "192.168.0.9");

    let mut odd = answer;
    odd["security"]["salt"] = json!("010");
    assert!(matches!(IpSrtpAnswer::parse(&odd), Err(ContractError::Hex("security.salt", 14))));
}

#[test]
fn ice_answer_requires_warning_timeout() {
    let base: SocketAddr = "192.168.1.30:6000".parse().unwrap();
    let d = SessionDescription::local(
        "x".into(),
        "y".into(),
        vec![Candidate::local(CandidateType::Host, base, base, None, 1)],
    );

    let ok = json!({"ice":{"candidates": d.to_json_string()},"keepalive":{"warningtimeout":"5000"}});
    assert_eq!(IceSrtpAnswer::parse(&ok).unwrap().description, d);

    let missing = json!({"ice":{"candidates": d.to_json_string()}});
    assert!(IceSrtpAnswer::parse(&missing).is_err());
}

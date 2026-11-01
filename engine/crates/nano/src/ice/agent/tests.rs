use super::*;
use crate::ice::stun::{Class, attr};

fn base(addr: &str) -> Base {
    Base { address: addr.parse().unwrap(), interface_index: 1 }
}

/// A controlled peer that answers checks with its password and nominates nothing.
fn answer(check: &[u8], from: SocketAddr, password: &str) -> Vec<u8> {
    let d = Message::decode(check).unwrap();
    Message::new(Class::Success, method::BINDING, d.message.transaction)
        .with(attr::XOR_MAPPED_ADDRESS, stun::encode_xor_address(from, &d.message.transaction))
        .encode(Some(password.as_bytes()), false)
}

#[test]
fn host_only_gathering_and_nomination() {
    let t0 = Instant::now();
    let mut a =
        IceAgent::new(t0, vec![base("192.168.1.20:5000")], IceConfig { stun: vec![], turn: None, relay_only: false });
    assert_eq!(a.poll_event(), Some(IceEvent::GatheringComplete));

    let d = a.local_description();
    assert_eq!(d.candidates.len(), 1);
    assert_eq!(d.username.len(), 12);
    assert_eq!(d.password.len(), 44);

    let phone: SocketAddr = "192.168.1.30:6000".parse().unwrap();
    let remote = SessionDescription::local(
        "phone".into(),
        "phonepass".into(),
        vec![Candidate::local(CandidateType::Host, phone, phone, None, 65_535)],
    );

    a.set_remote(t0, remote);
    a.handle_timeout(t0);

    let check = a.poll_transmit().unwrap();
    assert_eq!(check.to, phone);
    let m = Message::decode(&check.data).unwrap();
    m.verify_integrity(b"phonepass").unwrap();
    assert!(!m.message.has(attr::FINGERPRINT));
    assert_eq!(m.message.text(attr::USERNAME).unwrap(), format!("phone:{}", a.credentials.ufrag));
    assert_eq!(m.message.u32_attribute(attr::PRIORITY), Some(d.candidates[0].priority));
    assert!(m.message.has(attr::ICE_CONTROLLING));
    assert!(m.message.has(attr::TRANSACTION_TRANSMIT_COUNTER));

    a.handle_datagram(t0, 0, phone, &answer(&check.data, "192.168.1.20:5000".parse().unwrap(), "phonepass"));
    let nominate = a.poll_transmit().unwrap();
    assert!(Message::decode(&nominate.data).unwrap().message.has(attr::USE_CANDIDATE));

    a.handle_datagram(t0, 0, phone, &answer(&nominate.data, "192.168.1.20:5000".parse().unwrap(), "phonepass"));
    assert_eq!(a.poll_event(), Some(IceEvent::Selected { local: "192.168.1.20:5000".parse().unwrap(), remote: phone }));

    assert_eq!(a.handle_datagram(t0, 0, phone, &[0x80, 0x23, 0, 0]), Some(vec![0x80, 0x23, 0, 0]));
    assert_eq!(a.handle_datagram(t0, 0, "1.2.3.4:5".parse().unwrap(), &[0x80, 0, 0, 0]), None);
}

#[test]
fn answers_the_phones_checks() {
    let t0 = Instant::now();
    let mut a =
        IceAgent::new(t0, vec![base("192.168.1.20:5000")], IceConfig { stun: vec![], turn: None, relay_only: false });
    let phone: SocketAddr = "192.168.1.30:6000".parse().unwrap();
    let tx = [3u8; 12];
    let request = Message::new(Class::Request, method::BINDING, tx)
        .with(attr::TRANSACTION_TRANSMIT_COUNTER, stun::transmit_counter(2, 0))
        .encode(Some(a.credentials.password.as_bytes()), false);

    a.handle_datagram(t0, 0, phone, &request);
    let reply = a.poll_transmit().unwrap();
    let d = Message::decode(&reply.data).unwrap();
    d.verify_integrity(a.credentials.password.as_bytes()).unwrap();
    assert_eq!(d.message.mapped_address(), Some(phone));
    assert!(d.message.has(attr::MAPPED_ADDRESS));
    assert_eq!(d.message.get(attr::TRANSACTION_TRANSMIT_COUNTER), Some(&[0, 0, 2, 0][..]));

    let bad = Message::new(Class::Request, method::BINDING, tx).encode(Some(b"wrong"), false);
    a.handle_datagram(t0, 0, phone, &bad);
    assert!(a.poll_transmit().is_none());
}

#[test]
fn stun_timeouts_end_gathering_and_srflx_is_added() {
    let t0 = Instant::now();
    let server: SocketAddr = "198.51.100.1:3478".parse().unwrap();
    let mut a = IceAgent::new(
        t0,
        vec![base("192.168.1.20:5000")],
        IceConfig { stun: vec![server, "198.51.100.2:3478".parse().unwrap()], turn: None, relay_only: false },
    );

    let first = a.poll_transmit().unwrap();
    let _second = a.poll_transmit().unwrap();
    let d = Message::decode(&first.data).unwrap();
    assert!(!d.message.has(attr::USERNAME) && !d.has_integrity(), "a plain Binding request");

    let mapped: SocketAddr = "203.0.113.9:40000".parse().unwrap();
    let ok = Message::new(Class::Success, method::BINDING, d.message.transaction)
        .with(attr::XOR_MAPPED_ADDRESS, stun::encode_xor_address(mapped, &d.message.transaction))
        .encode(None, false);
    a.handle_datagram(t0, 0, first.to, &ok);
    assert!(!a.gathering_complete(), "the other server is pending");

    let mut t = t0;
    for _ in 0..10 {
        t += Duration::from_secs(4);
        a.handle_timeout(t);
    }

    assert!(a.gathering_complete());
    let kinds: Vec<CandidateType> = a.local_description().candidates.iter().map(|c| c.kind).collect();
    assert_eq!(kinds, vec![CandidateType::Host, CandidateType::ServerReflexive]);
}

#[test]
fn turn_allocation_with_credentials() {
    let t0 = Instant::now();
    let server: SocketAddr = "198.51.100.7:3478".parse().unwrap();
    let turn = TurnServer { address: server, username: "u".into(), password: "p".into() };
    let mut a = IceAgent::new(
        t0,
        vec![base("192.168.1.20:5000")],
        IceConfig { stun: vec![], turn: Some(turn), relay_only: false },
    );

    let first = a.poll_transmit().unwrap();
    let d = Message::decode(&first.data).unwrap();
    assert_eq!(d.message.get(attr::MS_CLIENT_CONTEXT), Some(CLIENT_CONTEXT));
    assert!(d.message.has(attr::DONT_FRAGMENT) && !d.message.has(attr::USERNAME));

    let challenge = Message::new(Class::Error, method::ALLOCATE, d.message.transaction)
        .with(attr::ERROR_CODE, stun::encode_error(401, "Unauthorized"))
        .with(attr::REALM, b"realm".to_vec())
        .with(attr::NONCE, b"nonce".to_vec())
        .encode(None, false);
    a.handle_datagram(t0, 0, server, &challenge);
    let second = a.poll_transmit().unwrap();
    let d2 = Message::decode(&second.data).unwrap();
    d2.verify_integrity(&stun::long_term_key("u", "realm", "p")).unwrap();

    let relayed: SocketAddr = "198.51.100.7:50000".parse().unwrap();
    let ok = Message::new(Class::Success, method::ALLOCATE, d2.message.transaction)
        .with(attr::XOR_RELAYED_ADDRESS, stun::encode_xor_address(relayed, &d2.message.transaction))
        .with(attr::LIFETIME, 3600u32.to_be_bytes().to_vec())
        .encode(None, false);
    a.handle_datagram(t0, 0, server, &ok);
    assert!(a.gathering_complete());
    let c = a.local_description().candidates;
    assert_eq!(c[1].kind, CandidateType::Relayed);
    assert_eq!(c[1].relay_standard.as_deref(), Some("0"));
    assert_eq!(a.poll_timeout(), Some(t0 + TURN_REFRESH_CAP));
}

#[test]
fn relay_only_offers_and_pairs_relayed_candidates() {
    let t0 = Instant::now();
    let server: SocketAddr = "198.51.100.7:3478".parse().unwrap();
    let turn = TurnServer { address: server, username: "u".into(), password: "p".into() };
    let config = IceConfig { stun: vec![], turn: Some(turn), relay_only: true };
    let mut a = IceAgent::new(t0, vec![base("192.168.1.20:5000")], config);

    let allocate = Message::decode(&a.poll_transmit().unwrap().data).unwrap().message.transaction;
    let relayed: SocketAddr = "198.51.100.7:50000".parse().unwrap();
    let ok = Message::new(Class::Success, method::ALLOCATE, allocate)
        .with(attr::XOR_RELAYED_ADDRESS, stun::encode_xor_address(relayed, &allocate))
        .encode(None, false);
    a.handle_datagram(t0, 0, server, &ok);

    let offered = a.local_description().candidates;
    assert_eq!(offered.len(), 1);
    assert_eq!(offered[0].kind, CandidateType::Relayed);

    let phone: SocketAddr = "192.168.1.30:6000".parse().unwrap();
    a.set_remote(
        t0,
        SessionDescription::local(
            "phone".into(),
            "pw".into(),
            vec![Candidate::local(CandidateType::Host, phone, phone, None, 65_535)],
        ),
    );

    a.handle_timeout(t0);
    let first = a.poll_transmit().unwrap();
    assert_eq!(first.to, server, "the only pair goes through the relay: CreatePermission first");
    let m = Message::decode(&first.data).unwrap().message;
    assert_eq!(m.method, method::CREATE_PERMISSION);
}

#[test]
fn turn_allocation_follows_an_alternate_server() {
    let t0 = Instant::now();
    let server: SocketAddr = "198.51.100.7:3478".parse().unwrap();
    let alternate: SocketAddr = "198.51.100.8:3478".parse().unwrap();
    let turn = TurnServer { address: server, username: "u".into(), password: "p".into() };
    let mut a = IceAgent::new(
        t0,
        vec![base("192.168.1.20:5000")],
        IceConfig { stun: vec![], turn: Some(turn), relay_only: false },
    );

    let first = a.poll_transmit().unwrap();
    let tx = Message::decode(&first.data).unwrap().message.transaction;
    let redirect = Message::new(Class::Error, method::ALLOCATE, tx)
        .with(attr::ERROR_CODE, stun::encode_error(300, "Try Alternate"))
        .with(attr::ALTERNATE_SERVER, stun::encode_address(alternate))
        .encode(None, false);

    a.handle_datagram(t0, 0, server, &redirect);
    let again = a.poll_transmit().unwrap();
    assert_eq!(again.to, alternate);
    assert!(!a.gathering_complete());
}

#[test]
fn pair_priorities() {
    assert_eq!(pair_priority(10, 20), (1 << 32) * 10 + 40);
    assert_eq!(pair_priority(20, 10), (1 << 32) * 10 + 40 + 1);
}

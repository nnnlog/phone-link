use super::*;

fn ip(s: &str) -> IpAddr {
    s.parse().unwrap()
}

#[test]
fn address_classes() {
    let cases = [
        ("8.8.8.8", AddressType::Unicast),
        ("100.64.1.1", AddressType::Unicast),
        ("224.0.0.1", AddressType::Multicast),
        ("255.255.255.255", AddressType::Invalid),
        ("127.0.0.1", AddressType::Loopback),
        ("10.1.2.3", AddressType::Private),
        ("172.31.0.1", AddressType::Private),
        ("172.32.0.1", AddressType::Unicast),
        ("192.168.1.20", AddressType::Private),
        ("169.254.3.4", AddressType::LinkLocal),
        ("0.1.2.3", AddressType::Unspecified),
        ("2001:db8::1", AddressType::Unicast),
        ("ff02::1", AddressType::Multicast),
        ("::1", AddressType::Loopback),
        ("fd00::1", AddressType::Private),
        ("fe80::1", AddressType::LinkLocal),
        ("fec0::1", AddressType::SiteLocal),
        ("::ffff:1.2.3.4", AddressType::Ipv4Mapped),
        ("::1.2.3.4", AddressType::Ipv4Compatible),
        ("::", AddressType::Unspecified),
        ("100::1", AddressType::Invalid),
    ];
    for (a, t) in cases {
        assert_eq!(classify(ip(a)), t, "{a}");
    }
}

#[test]
fn priorities() {
    assert_eq!(priority(CandidateType::Host, 65_535), 2_130_706_431);
    assert_eq!(priority(CandidateType::Relayed, local_preference(3)), 65_532 << 8 | 255);
}

#[test]
fn addresses_round_trip() {
    for a in ["192.168.1.20:51000", "[2001:db8::1]:3478"] {
        assert_eq!(format_address(parse_address(a).unwrap()), a);
    }

    assert_eq!(parse_address("[fe80::1%4]:9").unwrap().port(), 9);
    assert!(parse_address("host:1").is_none());
}

#[test]
fn description_round_trips() {
    let base: SocketAddr = "192.168.1.20:51000".parse().unwrap();
    let mut candidates = vec![
        Candidate::local(CandidateType::Host, base, base, None, 65_535),
        Candidate::local(
            CandidateType::ServerReflexive,
            "203.0.113.5:4000".parse().unwrap(),
            base,
            Some("198.51.100.1:3478".parse().unwrap()),
            65_535,
        ),
    ];

    assign_foundations(&mut candidates);
    assert_eq!((candidates[0].foundation, candidates[1].foundation), (1, 2));

    let d = SessionDescription::local("AbCdEfGhIjKl".into(), "p".repeat(44), candidates);
    let text = d.to_json_string();
    assert!(text.contains(r#""transportAddress":"192.168.1.20:51000""#), "{text}");
    assert!(text.contains(r#""priority":"2130706431""#));
    assert!(text.contains(r#""serverAddress":"""#));
    assert_eq!(SessionDescription::parse(&text).unwrap(), d);
}

#[test]
fn the_ipv6_flag_is_kept_as_written() {
    let text = r#"{"Version":"1","Username":"u","Password":"p","Candidates":{"count":"1","0":{
            "transportAddress":"203.0.113.9:4000","baseAddress":"[2001:db8::5]:4000","serverAddress":"198.51.100.1:3478",
            "ipv6":"1","type":"1","addressType":"0","priority":"1","foundation":"2","transport":"udp"}}}"#;

    let d = SessionDescription::parse(text).unwrap();
    assert!(d.candidates[0].ipv6, "an IPv4 address with the IPv6 flag of its base");
    assert!(d.candidates[0].transport_address.is_ipv4());
}

#[test]
fn description_rules() {
    let bad = |t: &str| SessionDescription::parse(t).unwrap_err();
    assert!(matches!(bad(r#"{"Version":"2","Username":"u","Password":"p"}"#), DescriptionError::Version(_)));
    assert!(matches!(
        bad(r#"{"Version":"1","Username":"","Password":"p","Candidates":{"count":"0"}}"#),
        DescriptionError::Empty("Username")
    ));
    assert_eq!(
        bad(r#"{"Version":"1","Username":"u","Password":"p","Candidates":{"count":"0"}}"#),
        DescriptionError::NoCandidates
    );
}

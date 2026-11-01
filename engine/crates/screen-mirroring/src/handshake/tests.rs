use super::*;

#[test]
fn handshake_values() {
    assert!(NanoHandshake::diagnostics_from(&NanoConfig::default()).is_none());

    let config = NanoConfig { mirroring_diagnostics_servers: "a, b".into(), ..Default::default() };
    assert_eq!(NanoHandshake::diagnostics_from(&config).as_deref(), Some("a,b"));

    let (a, b) = (next_sequence_number(), next_sequence_number());
    assert!(b > a && a > phonelink_core::time::UNIX_EPOCH_FILETIME);

    assert_eq!(answer_contract_version(&json!({"contractVersion":{"versionMax":9}})), 6);
    assert_eq!(answer_contract_version(&json!({})), 0);
}

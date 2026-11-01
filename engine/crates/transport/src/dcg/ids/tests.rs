use super::*;
use phonelink_core::types::Environment;

#[test]
fn counters_start_at_one_and_targets_keep_their_session() {
    let ids = DcgIdentifiers::new();
    let phone = DcgClient::new("phone", Environment::Prod);
    let other = DcgClient::new("phone", Environment::Beta);

    assert_eq!((ids.next_message_id(), ids.next_message_id()), (1, 2));
    assert_eq!(ids.target(&phone).next_sequence_number(), 1);
    assert_eq!(ids.target(&phone).next_sequence_number(), 2);
    assert_eq!(ids.target(&other).next_sequence_number(), 1, "per target (id and environment)");

    assert_eq!(ids.session_id(&phone), ids.session_id(&phone));
    assert_ne!(ids.session_id(&phone), ids.session_id(&other));

    assert!(Arc::ptr_eq(&DcgIdentifiers::process("WEA"), &DcgIdentifiers::process("WEA")), "one per process");
    let (pl, wea) = (DcgIdentifiers::process("PL"), DcgIdentifiers::process("WEA"));
    assert!(!Arc::ptr_eq(&pl, &wea), "one per PC identity");
    assert_ne!(pl.session_id(&phone), wea.session_id(&phone), "two identities never share a SessionId");
}

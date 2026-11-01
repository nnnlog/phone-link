use super::*;
use crate::auth::crypto::KeyPair;

fn policy() -> TrustPolicy {
    TrustPolicy::from_config(&AuthConfig::default())
}

fn t0() -> SystemTime {
    SystemTime::UNIX_EPOCH + Duration::from_secs(1_792_454_400)
}

fn record(partner: &str, ty: CryptoTrustType, cid: &str, now: SystemTime) -> TrustRelationship {
    let pair = KeyPair::generate(&trust_client_id(partner), now).unwrap();
    new_relationship(
        "trust_self",
        &trust_client_id(partner),
        cid,
        ty,
        &pair.cert,
        Environment::Prod,
        partner,
        now,
        &policy(),
    )
    .unwrap()
}

#[test]
fn conflicts_and_capacity() {
    let p = policy();
    let mut store = TrustStore::default();
    store.add(record("a", CryptoTrustType::AsyncD2D, "cid", t0()), t0(), &p).unwrap();

    let mut dup = record("A", CryptoTrustType::AsyncD2D, "cid", t0());
    assert!(matches!(store.add(dup.clone(), t0(), &p), Err(TrustStoreError::PartnerClientIdConflict(_))));
    dup.partner_client_id = "trust_other".into();
    assert!(matches!(store.add(dup, t0(), &p), Err(TrustStoreError::PartnerDcgClientConflict(_))));

    let small = TrustPolicy { max_relationships: 1, ..p };
    assert_eq!(
        store.add(record("b", CryptoTrustType::AsyncD2D, "cid", t0()), t0(), &small),
        Err(TrustStoreError::Full(1))
    );
}

#[test]
fn async_d2d_follows_its_a2d_only_while_one_exists() {
    let p = policy();
    let mut store = TrustStore::default();
    store.add(record("phone", CryptoTrustType::AsyncD2D, "cid", t0()), t0(), &p).unwrap();

    // No A2D at all: trusted.
    assert!(store.is_valid(store.get(0), t0(), &p));

    let mut a2d = record("account", CryptoTrustType::A2D, "cid", t0());
    a2d.partner_key_expiration_time = t0() + Duration::from_secs(60);
    store.add(a2d, t0(), &p).unwrap();
    let phone = DcgClient::new("phone", Environment::Prod);
    assert!(store.is_valid(store.get(0), t0(), &p));
    assert_eq!(store.trusted_index_by_dcg_client(&phone, t0(), &p), Some(0));

    // While the expired A2D is still stored, the AsyncD2D of its CID is not trusted…
    let later = t0() + Duration::from_secs(120);
    assert!(!store.is_valid(store.get(0), later, &p));
    assert_eq!(store.trusted_index_by_dcg_client(&phone, later, &p), None);
    assert!(TrustStore::is_unexpired(store.get(0), later, &p), "…but it is not expired");
    assert!(store.has_unexpired(later, &p), "the snap rule counts unexpired records");

    // …and pruning removes only the expired A2D: the AsyncD2D stays, trusted again.
    assert!(store.prune(later, &p));
    assert_eq!(store.len(), 1);
    assert_eq!(store.get(0).trust_type, CryptoTrustType::AsyncD2D);
    assert!(store.is_valid(store.get(0), later, &p));
    assert_eq!(store.trusted_index_by_dcg_client(&phone, later, &p), Some(0));

    // An A2D of another CID is not this record's anchor.
    let mut foreign = record("account-2", CryptoTrustType::A2D, "other-cid", later);
    foreign.partner_key_expiration_time = later + Duration::from_secs(60);
    store.add(foreign, later, &p).unwrap();
    assert!(store.is_valid(store.get(0), later + Duration::from_secs(120), &p));
}

#[test]
fn prune_keeps_unexpired_records_whatever_their_anchor() {
    let p = policy();
    let mut store = TrustStore::default();
    store.add(record("phone", CryptoTrustType::AsyncD2D, "cid", t0()), t0(), &p).unwrap();
    store.add(record("account", CryptoTrustType::A2D, "cid", t0()), t0(), &p).unwrap();

    // The A2D is never touched; the phone's record is (validated traffic, every 12 h+).
    let mut now = t0();
    for _ in 0..7 {
        now += Duration::from_secs(26 * 86_400);
        assert!(store.touch(0, now, &p));
        store.prune(now, &p);
    }

    // 182 d: the A2D's last access is older than 180 d; only it is gone.
    assert_eq!(store.len(), 1);
    assert_eq!(store.get(0).partner_client_id, "trust_phone");
    assert!(store.is_valid(store.get(0), now, &p));
}

#[test]
fn last_access_expiry_and_cooldown() {
    let p = policy();
    let mut store = TrustStore::default();
    store.add(record("phone", CryptoTrustType::AsyncD2D, "cid", t0()), t0(), &p).unwrap();

    assert!(!store.touch(0, t0() + Duration::from_secs(3600), &p));
    assert!(store.touch(0, t0() + Duration::from_secs(13 * 3600), &p));

    let far = t0() + Duration::from_secs(13 * 3600) + p.relationship_expiration;
    assert!(!store.is_valid(store.get(0), far, &p));
}

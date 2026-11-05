//! Trust synchronization (peers → `AsyncD2D`), the trust validation rules and the `TrustGate` seam
//! against the fake DCG server and device-list fixtures.

use crate::assert_matches;
use crate::support::auth_world;
use crate::support::session_log;

use std::time::{Duration, SystemTime};

use auth_world::harness::{Harness, eventually};
use auth_world::jwt;
use phonelink_engine::auth::crypto::{self, KeyPair};
use phonelink_engine::auth::trust::{CryptoTrustType, TrustPolicy, TrustRelationship, new_relationship};
use phonelink_engine::auth::{AuthError, FetchMode, PeerEvent};
use phonelink_engine::edge::Clock;
use phonelink_engine::seams::{TrustGate, TrustRotationError};
use phonelink_engine::types::{DcgClient, Environment, new_guid_lower};

const DAY: u64 = 86_400;
const FIXTURES: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/auth");

fn fixture(name: &str, vars: &[(&str, String)]) -> String {
    let mut text = std::fs::read_to_string(format!("{FIXTURES}/{name}")).unwrap();
    for (k, v) in vars {
        text = text.replace(&format!("{{{{{k}}}}}"), v);
    }
    assert!(!text.contains("{{"), "unfilled placeholder in {name}");
    text
}

fn rel<'a>(rels: &'a [TrustRelationship], partner_id: &str) -> Option<&'a TrustRelationship> {
    rels.iter().find(|r| r.partner_client_id == format!("trust_{partner_id}"))
}

fn prod(id: &str) -> DcgClient {
    DcgClient::new(id, Environment::Prod)
}

#[tokio::test]
async fn device_list_fixture_becomes_trust_records() {
    let h = Harness::new().await;
    let (service, p) = h.provisioned().await;
    let mut events = service.subscribe_peer_events();

    let own_cert = jwt::b64(&h.trust_cert(Environment::Prod));
    let now = h.clock.now();
    let a = KeyPair::generate("trust_a", now).unwrap();
    let b_self = KeyPair::generate("trust_b", now).unwrap();
    let b_pki = KeyPair::generate("trust_b", now).unwrap();
    let (id_a, id_b, id_old, id_none, id_nulls) =
        (new_guid_lower(), new_guid_lower(), new_guid_lower(), new_guid_lower(), new_guid_lower());

    let list = fixture(
        "device_list_mixed.json",
        &[
            ("OWN_ID", p.own.id.clone()),
            ("OWN_CERT", own_cert),
            ("PHONE_A", id_a.clone()),
            ("PHONE_A_CERT", jwt::b64(&a.cert.der)),
            ("PHONE_B", id_b.clone()),
            ("PHONE_B_SELF_SIGNED", jwt::b64(&b_self.cert.der)),
            ("PHONE_B_PKI", jwt::b64(&b_pki.cert.der)),
            ("JUNK_CERT", "bm90IGEgY2VydA==".into()),
            ("OLD_PC", id_old.clone()),
            ("OLD_PC_CERT", jwt::b64(&KeyPair::generate("trust_old", now).unwrap().cert.der)),
            ("NO_CERT", id_none.clone()),
            ("NULLS", id_nulls.clone()),
        ],
    );

    h.dcg.with(|s| s.device_list_override = Some(list));
    h.dcg.take_requests();
    h.advance(Duration::from_secs(60));

    let peers = service.sync_trust(FetchMode::ForceCloud).await.unwrap();
    assert_eq!(
        h.dcg.lines(),
        [
            "GET /DeviceAuthProxy/GetDeviceInfoList?api-version=1.5.0&filter=&top=20",
            "GET /DeviceAuthProxy/GetAccountCert?api-version=1.0.0",
        ],
        "the own entry matches: no SetDeviceInfo"
    );

    let ids: Vec<&str> = peers.iter().map(|p| p.client.id.as_str()).collect();
    assert_eq!(ids, [id_a.as_str(), id_b.as_str()], "linked peers with a certificate");
    assert!(peers.iter().all(|p| p.client.env == Environment::Prod));
    assert_eq!(peers[0].info.capabilities.as_ref().unwrap()[1], "RINGER_MODE");
    assert_eq!(peers[0].info.distinguished_device_id.as_deref(), Some("dd-phone-a"));

    // The own entry is not a peer, but is kept with the id the service assigned this record.
    assert!(peers.iter().all(|peer| peer.client.id != p.own.id));
    let own = service.own_entry().expect("own entry kept");
    assert_eq!(own.id, p.own.id);
    assert_eq!(own.distinguished_device_id.as_deref(), Some("l:1234567890123456"));

    // A restart within the refresh interval does not fetch the list again; the entry is kept.
    let restarted = h.service();
    assert_eq!(restarted.own_entry().and_then(|e| e.distinguished_device_id).as_deref(), Some("l:1234567890123456"));

    let rels = service.trust_relationships();
    assert_eq!(rels.len(), 3, "A2D + two AsyncD2D: {rels:#?}");
    let account = service.local_account().unwrap();
    let ra = rel(&rels, &id_a).unwrap();
    let synced_at = h.clock.now();

    assert_eq!(ra.trust_type, CryptoTrustType::AsyncD2D);
    assert_eq!(ra.self_client_id, format!("trust_{}", p.own.id));
    assert_eq!(ra.msa_cid.as_deref(), Some(account.cid.as_str()));
    assert_eq!(ra.partner_certificate, a.cert.der);
    assert_eq!(ra.attributes.get("dcg_prod_client_id"), Some(&id_a));
    assert_eq!(ra.partner_key_expiration_time, a.cert.not_after.min(synced_at + Duration::from_secs(365 * DAY)));
    assert_eq!(ra.relationship_last_access_time, synced_at);
    assert!(ra.partner_temporary_certificate.is_none());

    assert_eq!(rel(&rels, &id_b).unwrap().partner_certificate, b_pki.cert.der, "PKI[0] over SelfSigned[0]");
    for skipped in [&id_old, &id_none, &id_nulls] {
        assert!(rel(&rels, skipped).is_none());
    }

    // The cache holds the linked peers (with or without a usable certificate).
    let cache = h.store_json("auth/peer-cache");
    let cached: Vec<&str> =
        cache[&account.account_id].as_array().unwrap().iter().map(|d| d["id"].as_str().unwrap()).collect();
    assert_eq!(cached, [id_a.as_str(), id_b.as_str(), id_none.as_str()]);
    assert_eq!(service.account_info().unwrap().last_get_trusted_peer_clients_time, Some(synced_at));

    // Events.
    let mut added = Vec::new();
    while let Ok(e) = events.try_recv() {
        match e {
            PeerEvent::Added(p) => added.push(p.client.id),
            other => panic!("unexpected {other:?}"),
        }
    }
    assert_eq!(added, [id_a.clone(), id_b.clone()]);

    // TrustGate.
    assert!(service.is_trusted(&prod(&id_a)));
    assert!(service.is_trusted(&prod(&id_a.to_ascii_uppercase())), "case-insensitive");
    assert!(!service.is_trusted(&DcgClient::new(id_a.clone(), Environment::Beta)));
    assert!(!service.is_trusted(&prod(&id_none)));
    assert!(!service.is_trusted(&prod(&id_old)));
}

#[tokio::test]
async fn unknown_certificate_key_invalidates_the_whole_list() {
    let h = Harness::new().await;
    let (service, p, phone, peers) = h.with_phone().await;
    assert_eq!(peers.len(), 1);

    let list = fixture(
        "device_list_unknown_cert_key.json",
        &[
            ("OWN_ID", p.own.id.clone()),
            ("OWN_CERT", jwt::b64(&h.trust_cert(Environment::Prod))),
            ("PHONE_A", new_guid_lower()),
            ("PHONE_A_CERT", jwt::b64(&phone.pair.cert.der)),
        ],
    );

    h.dcg.with(|s| s.device_list_override = Some(list));
    let before = service.trust_relationships();
    let err = service.sync_trust(FetchMode::ForceCloud).await.unwrap_err();
    assert!(
        matches!(&err, AuthError::Rest(e) if matches!(**e, phonelink_engine::auth::dcg_rest::RestError::InvalidResponse { .. })),
        "{err:?}"
    );
    assert_eq!(service.trust_relationships(), before, "nothing changed");

    // Default mode falls back to the cached peers.
    h.advance(Duration::from_secs(DAY + 1));
    let cached = service.sync_trust(FetchMode::Default).await.unwrap();
    assert_eq!(cached.len(), 1);
    assert_eq!(cached[0].client.id, phone.id);
}

#[tokio::test]
async fn own_entry_is_repaired_with_set_device_info() {
    let h = Harness::new().await;
    let (service, p, _, _) = h.with_phone().await;
    let trust_b64 = jwt::b64(&h.trust_cert(Environment::Prod));

    // A stale certificate in the own entry → SetDeviceInfo with the current one.
    let stale = jwt::b64(&KeyPair::generate("trust_x", h.clock.now()).unwrap().cert.der);
    h.dcg.own_entry_mut(&p.own.id, |e| {
        e.as_mut().unwrap().certificates.as_mut().unwrap().self_signed = Some(vec![stale])
    });
    h.dcg.take_requests();
    assert_eq!(service.sync_trust(FetchMode::ForceCloud).await.unwrap().len(), 1);

    let ops: Vec<String> = h.dcg.requests().iter().map(|r| r.op().to_string()).collect();
    assert_eq!(ops, ["GetDeviceInfoList", "SetDeviceInfo", "GetAccountCert"]);
    let set = &h.dcg.of("SetDeviceInfo")[0];
    assert_eq!(set.json()["certificates"]["SelfSigned"][0], trust_b64);
    assert_eq!(set.query("pop-device-key").as_deref(), Some(""));

    // A stale certificate whose repair fails: logged, the sync goes on.
    let stale = jwt::b64(&KeyPair::generate("trust_y", h.clock.now()).unwrap().cert.der);
    h.dcg.own_entry_mut(&p.own.id, |e| {
        e.as_mut().unwrap().certificates.as_mut().unwrap().self_signed = Some(vec![stale])
    });

    h.dcg.script("SetDeviceInfo", 500, "");
    assert_eq!(service.sync_trust(FetchMode::ForceCloud).await.unwrap().len(), 1);
    assert!(service.account_info().is_some());

    // A missing own entry: SetDeviceInfo recreates it.
    h.dcg.own_entry_mut(&p.own.id, |e| *e = None);
    assert_eq!(service.sync_trust(FetchMode::ForceCloud).await.unwrap().len(), 1);
    assert!(h.dcg.with(|s| s.enrolled.contains_key(&p.own.id)));

    // A stale entry whose repair finds the identity rejected: `auth::identity_rules`.

    // A missing own entry whose repair fails: "enrollment expired" → deprovisioned, Unauthorized.
    h.dcg.own_entry_mut(&p.own.id, |e| *e = None);
    h.dcg.script("SetDeviceInfo", 500, "");
    let err = service.sync_trust(FetchMode::ForceCloud).await.unwrap_err();
    assert!(matches!(&err, AuthError::Unauthorized(m) if m.contains("enrollment expired")), "{err:?}");
    assert!(service.account_info().is_none());
    assert!(service.peers().is_empty());
}

#[tokio::test]
async fn unlinked_peers_lose_their_relationship() {
    let h = Harness::new().await;
    let (service, _, a, _) = h.with_phone().await;

    let b = h.phone("Galaxy");
    h.dcg.add_peer(b.item.clone());
    let mut events = service.subscribe_peer_events();

    assert_eq!(service.sync_trust(FetchMode::ForceCloud).await.unwrap().len(), 2);
    assert!(matches!(events.try_recv(), Ok(PeerEvent::Added(p)) if p.client.id == b.id));

    h.dcg.peer_mut(&b.id, |p| p.is_linked = Some(false));
    let peers = service.sync_trust(FetchMode::ForceCloud).await.unwrap();
    assert_eq!(peers.iter().map(|p| p.client.id.as_str()).collect::<Vec<_>>(), [a.id.as_str()]);
    assert!(rel(&service.trust_relationships(), &b.id).is_none());
    assert!(!service.is_trusted(&prod(&b.id)));
    assert!(matches!(events.try_recv(), Ok(PeerEvent::Removed(c)) if c.id == b.id));

    // The user's unlink: UnlinkDevice, then a forced refresh.
    h.dcg.take_requests();
    let peers = service.unlink_device(&prod(&a.id)).await.unwrap();
    assert!(peers.is_empty());

    let ops: Vec<String> = h.dcg.requests().iter().map(|r| r.line()).collect();
    assert_eq!(
        ops,
        [
            format!("POST /DeviceAuthProxy/UnlinkDevice?api-version=1.5.0&device-id={}", a.id),
            "GET /DeviceAuthProxy/GetDeviceInfoList?api-version=1.5.0&filter=&top=20".into(),
            "GET /DeviceAuthProxy/GetAccountCert?api-version=1.0.0".into(),
        ]
    );

    let unlink = &h.dcg.of("UnlinkDevice")[0];
    assert!(unlink.header("Dcg-Token").is_some());
    assert_eq!(unlink.header("Authorization-Type"), Some("MSA"));
    assert_eq!(service.trust_relationships().len(), 1, "only the A2D remains");
}

#[tokio::test]
async fn partner_and_account_certificates_rotate_on_a_new_thumbprint() {
    let h = Harness::new().await;
    let (service, _, phone, _) = h.with_phone().await;
    let mut events = service.subscribe_peer_events();

    let new_pair = KeyPair::generate(&format!("trust_{}", phone.id), h.clock.now()).unwrap();
    h.dcg.peer_mut(&phone.id, |p| {
        p.certificates.as_mut().unwrap().self_signed = Some(vec![jwt::b64(&new_pair.cert.der)])
    });

    let new_account = KeyPair::generate(&h.dcg.account_id(), h.clock.now()).unwrap();
    let old_account = h.dcg.account_cert();
    h.dcg.with(|s| s.account_cert = new_account.cert.der.clone());

    h.advance(Duration::from_secs(30));
    service.sync_trust(FetchMode::ForceCloud).await.unwrap();

    let now = h.clock.now();
    let rels = service.trust_relationships();
    let r = rel(&rels, &phone.id).unwrap();
    assert_eq!(r.partner_certificate, new_pair.cert.der);
    assert_eq!(r.partner_temporary_certificate.as_deref(), Some(phone.pair.cert.der.as_slice()));
    assert_eq!(r.partner_temporary_key_expiration_time, Some(now + Duration::from_secs(600)));
    assert!(matches!(events.try_recv(), Ok(PeerEvent::Updated(p)) if p.client.id == phone.id));

    let a2d = rels.iter().find(|r| r.trust_type == CryptoTrustType::A2D).unwrap();
    assert_eq!(a2d.partner_certificate, new_account.cert.der, "GetAccountCert rotates the A2D record");
    assert_eq!(a2d.partner_temporary_certificate.as_deref(), Some(old_account.as_slice()));

    // The temporary certificate is dropped after its window.
    h.advance(Duration::from_secs(601));
    assert!(service.is_trusted(&prod(&phone.id)));
    assert!(rel(&service.trust_relationships(), &phone.id).unwrap().partner_temporary_certificate.is_none());
}

#[tokio::test]
async fn trust_gate_expiry_and_last_access() {
    let h = Harness::new().await;
    let (service, _, phone, _) = h.with_phone().await;
    let synced = h.clock.now();
    let client = prod(&phone.id);

    assert_eq!(
        service.partner_key_expiration(&client),
        Some(phone.pair.cert.not_after.min(synced + Duration::from_secs(365 * DAY)))
    );
    assert_eq!(service.partner_key_expiration(&prod("unknown")), None);

    // RelationshipLastAccessTime is refreshed on validated use at most once per 12 h.
    h.advance(Duration::from_secs(3600));
    assert!(service.is_trusted(&client));
    assert_eq!(rel(&service.trust_relationships(), &phone.id).unwrap().relationship_last_access_time, synced);

    h.advance(Duration::from_secs(12 * 3600));
    assert!(service.is_trusted(&client));
    let touched = h.clock.now();
    assert_eq!(rel(&service.trust_relationships(), &phone.id).unwrap().relationship_last_access_time, touched);

    let persisted = h.store_json("auth/trust-relationships");
    assert!(persisted.as_array().unwrap().iter().any(|r| r["RelationshipLastAccessTime"]
        == serde_json::json!(touched.duration_since(SystemTime::UNIX_EPOCH).unwrap().as_millis() as u64)));

    // 180 d without use: expired and removed on access.
    h.advance(Duration::from_secs(181 * DAY));
    assert!(!service.is_trusted(&client));
    assert!(service.trust_relationships().is_empty(), "the unused A2D expired as well");
    assert!(service.peers().is_empty());
}

/// The trust store's A2D record (the account anchor).
fn a2d(rels: &[TrustRelationship]) -> Option<&TrustRelationship> {
    rels.iter().find(|r| r.trust_type == CryptoTrustType::A2D)
}

#[tokio::test]
async fn an_expired_a2d_is_removed_alone_and_its_async_d2d_stays_trusted() {
    let h = Harness::new().await;

    // An account certificate that expires in one hour.
    let short = KeyPair::generate(&h.dcg.account_id(), h.clock.now() - Duration::from_secs(365 * DAY - 3600)).unwrap();
    h.dcg.with(|s| s.account_cert = short.cert.der.clone());

    let (service, _, phone, _) = h.with_phone().await;
    let client = prod(&phone.id);
    assert!(service.is_trusted(&client));
    assert!(a2d(&service.trust_relationships()).is_some());

    h.advance(Duration::from_secs(2 * 3600));
    // Only expired records are removed; with no A2D left the AsyncD2D record is trusted.
    assert!(service.is_trusted(&client), "the phone stays trusted");
    let rels = service.trust_relationships();
    assert_eq!(rels.len(), 1, "{rels:#?}");
    assert_eq!(rel(&rels, &phone.id).unwrap().trust_type, CryptoTrustType::AsyncD2D);
    assert_eq!(service.peers().len(), 1);

    // The next cloud refresh recreates the A2D from a current account certificate.
    let fresh = KeyPair::generate(&h.dcg.account_id(), h.clock.now()).unwrap();
    h.dcg.with(|s| s.account_cert = fresh.cert.der.clone());
    service.sync_trust(FetchMode::ForceCloud).await.unwrap();

    let rels = service.trust_relationships();
    assert_eq!(a2d(&rels).unwrap().partner_certificate, fresh.cert.der);
    assert_eq!(a2d(&rels).unwrap().relationship_last_access_time, h.clock.now());
    assert!(service.is_trusted(&client));

    // With no A2D record at all an AsyncD2D record is trusted.
    let h = Harness::new().await;
    let (service, _, phone, _) = h.with_phone().await;
    drop(service);

    let mut rels = h.store_json("auth/trust-relationships");
    rels.as_array_mut().unwrap().retain(|r| r["TrustType"] != "A2D");
    h.put_store_json("auth/trust-relationships", &rels);

    let restarted = h.service();
    assert!(restarted.is_trusted(&prod(&phone.id)));
}

#[tokio::test]
async fn the_a2d_expires_180_days_after_creation_and_the_phone_stays_trusted() {
    let h = Harness::new().await;
    let (service, _, phone, _) = h.with_phone().await;
    let client = prod(&phone.id);
    let created = a2d(&service.trust_relationships()).unwrap().relationship_last_access_time;
    let mut events = service.subscribe_peer_events();

    // Traffic validates the phone's record only (every 26 d, so each check refreshes its last
    // access); nothing ever touches the A2D, which expires once 180 d have passed since creation.
    for step in 1..=7u64 {
        h.advance(Duration::from_secs(26 * DAY));
        assert!(service.is_trusted(&client), "day {}", 26 * step);
        let rels = service.trust_relationships();
        assert_eq!(rel(&rels, &phone.id).unwrap().relationship_last_access_time, h.clock.now());
        assert_eq!(a2d(&rels).is_some(), 26 * step < 180, "day {}: {rels:#?}", 26 * step);
    }

    let rels = service.trust_relationships();
    assert_eq!(rels.len(), 1, "only the expired A2D was removed");
    assert_eq!(rel(&rels, &phone.id).unwrap().trust_type, CryptoTrustType::AsyncD2D);
    assert_eq!(service.peers().len(), 1);

    // A data JWT the phone signs still validates.
    let exp = h.clock.now().duration_since(SystemTime::UNIX_EPOCH).unwrap().as_secs() + 60;
    let claims = serde_json::json!({"iss": format!("trust_{}", phone.id), "Data": "{}", "Scope": "wake", "exp": exp});
    let jwt = crypto::sign_jws(&phone.pair.key, &claims).unwrap();
    assert_eq!(service.validate_inbound_jwt(&jwt, Some("wake")).unwrap().signer, vec![client.clone()]);

    // The next cloud refresh (Default: the last one is older than 1 d) recreates the A2D.
    h.dcg.take_requests();
    assert_eq!(service.sync_trust(FetchMode::Default).await.unwrap().len(), 1);
    assert_eq!(h.dcg.count("GetAccountCert"), 1);

    let rels = service.trust_relationships();
    let anchor = a2d(&rels).expect("A2D recreated");
    assert_eq!(anchor.partner_certificate, h.dcg.account_cert());
    assert_eq!(anchor.relationship_last_access_time, h.clock.now());
    assert!(anchor.relationship_last_access_time > created);
    assert!(service.is_trusted(&client));
    assert!(events.try_recv().is_err(), "the phone was never removed: no peer event");
}

#[tokio::test]
async fn rotate_partner_certificate_through_the_trust_gate() {
    let h = Harness::new().await;
    let (service, _, phone, _) = h.with_phone().await;
    let client = prod(&phone.id);
    let next = KeyPair::generate(&format!("trust_{}", phone.id), h.clock.now()).unwrap();

    service.rotate_partner_certificate(&client, &next.cert.der).unwrap();

    let r = rel(&service.trust_relationships(), &phone.id).unwrap().clone();
    assert_eq!(r.partner_certificate, next.cert.der);
    assert_eq!(r.partner_temporary_certificate.as_deref(), Some(phone.pair.cert.der.as_slice()));
    assert_eq!(r.partner_temporary_key_expiration_time, Some(h.clock.now() + Duration::from_secs(600)));
    assert_eq!(
        service.partner_key_expiration(&client),
        Some(next.cert.not_after.min(h.clock.now() + Duration::from_secs(365 * DAY)))
    );

    // Data JWTs of the old key validate during the 10-minute window only.
    let issuer = format!("trust_{}", phone.id);
    let jwt_by = |pair: &KeyPair| {
        let exp = h.clock.now().duration_since(SystemTime::UNIX_EPOCH).unwrap().as_secs() + 60;
        crypto::sign_jws(&pair.key, &serde_json::json!({"iss": issuer, "Data": "{}", "Scope": "wake", "exp": exp}))
            .unwrap()
    };

    let ok = service.validate_inbound_jwt(&jwt_by(&phone.pair), Some("wake")).unwrap();
    assert_eq!(ok.signer, vec![client.clone()]);

    h.advance(Duration::from_secs(601));
    assert!(service.validate_inbound_jwt(&jwt_by(&phone.pair), Some("wake")).is_err());
    assert!(service.validate_inbound_jwt(&jwt_by(&next), Some("wake")).is_ok());

    // Errors.
    assert_eq!(service.rotate_partner_certificate(&prod("nobody"), &next.cert.der), Err(TrustRotationError::NotFound));
    assert_eq!(service.rotate_partner_certificate(&client, b"garbage"), Err(TrustRotationError::InvalidCertificate));

    let expired = KeyPair::generate("trust_x", h.clock.now() - Duration::from_secs(400 * DAY)).unwrap();
    assert_eq!(
        service.rotate_partner_certificate(&client, &expired.cert.der),
        Err(TrustRotationError::InvalidCertificate)
    );

    // Persisted.
    drop(service);
    let restarted = h.service();
    assert_eq!(rel(&restarted.trust_relationships(), &phone.id).unwrap().partner_certificate, next.cert.der);
}

#[tokio::test]
async fn fetch_modes() {
    let h = Harness::new().await;
    let (service, _, phone, _) = h.with_phone().await;
    h.dcg.take_requests();
    assert_eq!(service.sync_trust(FetchMode::LocalOnly).await.unwrap()[0].client.id, phone.id);
    assert_eq!(service.sync_trust(FetchMode::Default).await.unwrap().len(), 1);
    assert!(h.dcg.requests().is_empty(), "LocalOnly, and Default within 1 d, make no request");

    h.advance(Duration::from_secs(DAY + 1));
    service.sync_trust(FetchMode::Default).await.unwrap();
    assert_eq!(h.dcg.count("GetDeviceInfoList"), 1);

    // A failed list: Default returns the cache, ForceCloud fails.
    h.advance(Duration::from_secs(DAY + 1));
    h.dcg.script("GetDeviceInfoList", 503, "");
    assert_eq!(service.sync_trust(FetchMode::Default).await.unwrap().len(), 1);
    h.dcg.script("GetDeviceInfoList", 503, "");
    assert!(service.sync_trust(FetchMode::ForceCloud).await.is_err());

    // 403/13 on the list deprovisions whatever the mode.
    h.dcg.script("GetDeviceInfoList", 403, &auth_world::fake_dcg::error_body(Some(13), "Unlinked"));
    assert_matches!(service.sync_trust(FetchMode::Default).await.unwrap_err(), AuthError::UnlinkedDevice);
    assert!(service.account_info().is_none());
    assert!(matches!(service.sync_trust(FetchMode::LocalOnly).await, Err(AuthError::NotProvisioned(_))));
}

#[tokio::test]
async fn untrusted_sources_trigger_a_budgeted_forced_refresh() {
    // One thread: the captured log holds every line of this service.
    let log = session_log::capture_thread();
    let h = Harness::new().await;
    let (service, _, _, _) = h.with_phone().await;

    h.dcg.take_requests();
    let stranger = prod("7d4f1e0a-0000-4000-8000-000000000001");

    for _ in 0..7 {
        service.on_untrusted_source(&stranger);
    }

    // Each report takes from the budget as it is made: five refreshes start, two find none left.
    assert_eq!(log.count("forced peer refresh budget exhausted"), 2, "at most 5 per source per hour");
    assert!(eventually(Duration::from_secs(5), || h.dcg.count("GetDeviceInfoList") >= 5).await);
    assert_eq!(h.dcg.count("GetDeviceInfoList"), 5, "one request per refresh");

    service.on_untrusted_source(&prod("7d4f1e0a-0000-4000-8000-000000000002"));
    assert!(
        eventually(Duration::from_secs(5), || h.dcg.count("GetDeviceInfoList") == 6).await,
        "another source has its own budget"
    );
}

#[tokio::test]
async fn pairing_era_records_are_replaced_only_for_async_trust_peers() {
    for (custom, os, replaced) in [
        (r#"{"yppCapabilities":31}"#, "Android", false),
        (r#"{"yppCapabilities":32}"#, "Android", true),
        ("not json", "iOS", true),
    ] {
        let h = Harness::new().await;
        let (service, p) = h.provisioned().await;
        drop(service);

        let mut phone = h.phone("legacy");
        phone.item.custom_data = Some(custom.into());
        phone.item.os_name = Some(os.into());

        let policy = TrustPolicy::from_config(&h.config.auth);
        let mut legacy = new_relationship(
            &format!("trust_{}", p.own.id),
            "legacy-pairing-partner",
            "",
            CryptoTrustType::ClassicPairingD2D,
            &phone.pair.cert,
            Environment::Prod,
            &phone.id,
            h.clock.now(),
            &policy,
        )
        .unwrap();

        legacy.msa_cid = None;
        let mut rels = h.store_json("auth/trust-relationships");
        rels.as_array_mut().unwrap().push(serde_json::to_value(&legacy).unwrap());
        h.put_store_json("auth/trust-relationships", &rels);

        h.dcg.add_peer(phone.item.clone());
        let service = h.service();
        let peers = service.sync_trust(FetchMode::ForceCloud).await.unwrap();

        let rels = service.trust_relationships();
        let legacy_kept = rels.iter().any(|r| r.partner_client_id == "legacy-pairing-partner");
        assert_eq!(legacy_kept, !replaced, "{custom} {os}");
        assert_eq!(rel(&rels, &phone.id).is_some(), replaced, "{custom} {os}");
        assert_eq!(peers.len(), usize::from(replaced));
        assert!(service.is_trusted(&prod(&phone.id)), "either record binds the phone");
    }
}

#[tokio::test]
async fn account_certificate_failures_do_not_deprovision() {
    let h = Harness::new().await;
    let (service, _, phone, _) = h.with_phone().await;

    // 403 + 13 means "unlinked" only on the calls the protocol names; GetAccountCert is not one.
    h.dcg.script("GetAccountCert", 403, &auth_world::fake_dcg::error_body(Some(13), "Unlinked"));
    let err = service.sync_trust(FetchMode::ForceCloud).await.unwrap_err();
    assert!(matches!(&err, AuthError::Rest(e) if e.is_unlinked()), "{err:?}");
    assert!(service.account_info().is_some(), "the account stays");
    assert_eq!(service.peers().len(), 1, "the peer part of the refresh was kept");

    // An absent account certificate is AccountTrustSyncFailure.
    h.dcg.script("GetAccountCert", 200, "{}");
    assert!(matches!(service.sync_trust(FetchMode::ForceCloud).await, Err(AuthError::AccountTrustSyncFailure(_))));

    // Partner ids resolve with or without the trust_ prefix.
    assert_eq!(service.resolve_partner(&phone.id), Some(prod(&phone.id)));
    assert_eq!(service.resolve_partner(&format!("trust_{}", phone.id)), Some(prod(&phone.id)));
    assert_eq!(service.resolve_partner("nobody"), None);
}

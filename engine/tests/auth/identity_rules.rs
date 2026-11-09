//! The identity rules both identities keep (`WEA`, the default, and `PL`, `Harness::phone_link`): a
//! lost identity is created anew in place and its account enrolled again, an expired identity keeps
//! signing in, and a cleared identity takes only the account enrolled with it.

use crate::assert_matches;
use crate::support::auth_world;

use std::time::Duration;

use auth_world::fake_dcg::error_body;
use auth_world::harness::{Harness, eventually};
use phonelink_engine::auth::crypto::KeyPair;
use phonelink_engine::auth::store::AccountState;
use phonelink_engine::auth::trust::TrustRelationship;
use phonelink_engine::auth::{AuthError, AuthService, FetchMode};
use phonelink_engine::edge::{Clock, SecureStore};
use phonelink_engine::seams::{TokenSource, TrustGate};
use phonelink_engine::types::{DcgClient, Environment, new_guid_lower};

const DAY: u64 = 86_400;
const PROD_SCOPE: &str = "service::msatoken.dcg.microsoft.com::MBI_SSL";

fn prod(id: &str) -> DcgClient {
    DcgClient::new(id, Environment::Prod)
}

fn rel<'a>(rels: &'a [TrustRelationship], partner_id: &str) -> Option<&'a TrustRelationship> {
    rels.iter().find(|r| r.partner_client_id == format!("trust_{partner_id}"))
}

/// The `deviceId` of every `GenerateNonce` so far.
fn nonce_device_ids(h: &Harness) -> Vec<String> {
    h.dcg.of("GenerateNonce").iter().map(|r| r.json()["deviceId"].as_str().unwrap().to_string()).collect()
}

/// After the identity `old` was created anew as `new`: the account is enrolled under `new`, its
/// peers synchronized for it (the phone trusted through a relationship of `new`), and nothing is
/// left bound to `old`.
async fn assert_enrolled_again(h: &Harness, service: &AuthService, old: &str, new: &str, phone: &str) {
    let enroll = h.dcg.of("EnrollDevice");
    let enroll = enroll.last().expect("EnrollDevice sent for the new identity");
    assert_eq!(enroll.query("pop-device-key").as_deref(), Some(""), "the silent path's empty key");
    assert!(enroll.header("Authorization").unwrap().starts_with("Bearer msa-at-"), "the DCG-scope token");
    assert_eq!(h.dcg.with(|s| s.tokens.get(enroll.header("Dcg-Token").unwrap()).cloned()).as_deref(), Some(new));
    assert_eq!(
        auth_world::jwt::cert_cn(&auth_world::jwt::unb64(
            enroll.json()["certificates"]["SelfSigned"][0].as_str().unwrap()
        )),
        format!("trust_{new}"),
        "a trust certificate of the new identity"
    );
    assert!(h.msa.scopes().iter().all(|scope| scope == PROD_SCOPE), "no strong-auth token on this path");

    let info = service.account_info().unwrap();
    assert_eq!((info.state, info.dcg_client_id.as_deref()), (AccountState::Enrolled, Some(new)));

    let synced = eventually(Duration::from_secs(5), || service.is_trusted(&prod(phone))).await;
    assert!(synced, "the background peer refresh trusts the phone again");
    let rels = service.trust_relationships();
    assert_eq!(rel(&rels, phone).unwrap().self_client_id, format!("trust_{new}"));
    assert!(rels.iter().all(|r| r.self_client_id != format!("trust_{old}")), "nothing bound to the old identity");
    assert_eq!(service.own_entry().map(|e| e.id).as_deref(), Some(new));
}

/// `SignIn` rejecting the certificate, or the key rotation after a successful `SignIn` rejecting it,
/// does not clear the identity: a new one is created in place, the token request returns its token,
/// the account is enrolled again under it and its peers synchronized. The same for both identities.
#[tokio::test]
async fn a_rejected_identity_is_created_anew_in_place_and_enrolled_again() {
    for (phone_link, rejected_by) in [(false, "SignIn"), (false, "RotateKeys"), (true, "SignIn"), (true, "RotateKeys")]
    {
        let mut h = if phone_link { Harness::phone_link().await } else { Harness::new().await };
        h.config.auth.identity_key_rotation = Duration::from_secs(2 * DAY);
        h.dcg.with(|s| s.key_valid_remaining_days = None);
        let (service, p, phone, _) = h.with_phone().await;
        if rejected_by == "RotateKeys" {
            h.advance(Duration::from_secs(2 * DAY + 1));
        }

        h.dcg.take_requests();
        let mut removed = service.subscribe_account_removed();
        h.dcg.script(rejected_by, 400, &error_body(Some(2), "InvalidCertificate"));
        let token = service.services_token_for(Environment::Prod, true).await.unwrap();

        let new = service.own_client().expect("an identity").id;
        assert_ne!(new, p.own.id, "{rejected_by}: a new dcgClientId");
        assert_eq!(h.dcg.with(|s| s.tokens.get(&token).cloned()).as_deref(), Some(new.as_str()), "{rejected_by}");

        let mut expected = vec!["GenerateNonce", "SignIn"];
        if rejected_by == "RotateKeys" {
            expected.extend(["GenerateNonce", "RotateKeys"]);
        }
        expected.extend(["GenerateNonce", "CreateIdentity", "EnrollDevice"]);
        let ops: Vec<String> = h.dcg.requests().iter().map(|r| r.op().to_string()).take(expected.len()).collect();
        assert_eq!(ops, expected, "{rejected_by}");
        assert_eq!(nonce_device_ids(&h).last(), Some(&new), "{rejected_by}: CreateIdentity's nonce names the new id");

        assert_enrolled_again(&h, &service, &p.own.id, &new, &phone.id).await;
        assert_eq!(h.dcg.count("CreateIdentity"), 1, "{rejected_by}");
        assert!(removed.try_recv().is_err(), "{rejected_by}: the account was not deprovisioned");

        // A restart signs in as the new identity.
        drop(service);
        h.dcg.take_requests();
        let restarted = h.service();
        assert_eq!(restarted.ensure_provisioned().await.unwrap().own.id, new, "{rejected_by}");
        restarted.services_token(Environment::Prod, false).await.unwrap();
        assert_eq!(nonce_device_ids(&h), std::slice::from_ref(&new), "{rejected_by}");
    }
}

/// An identity whose key + certificate entry is missing or unusable is kept when the state loads; the
/// next services-token request asks a nonce for it, finds nothing to sign it with, creates the
/// identity anew and enrolls the account again.
#[tokio::test]
async fn an_identity_without_its_key_is_created_anew_at_the_next_token_request() {
    const PAIR: &str = "auth/prod/identity-pair";
    for case in ["a missing entry", "a torn entry", "the pair of another identity"] {
        let h = Harness::new().await;
        let (service, p, phone, _) = h.with_phone().await;
        drop(service);

        match case {
            "a missing entry" => h.store.delete(PAIR.into()).unwrap(),
            "a torn entry" => h.store.store(PAIR.into(), br#"{"key":"MIG2AgEA"#.to_vec()).unwrap(),
            _ => {
                let other = KeyPair::generate(&new_guid_lower(), h.clock.now()).unwrap();
                h.put_pair_entry(PAIR, &other.pkcs8().unwrap(), &other.cert.der);
            }
        }

        let restarted = h.service();
        assert_eq!(restarted.own_client(), Some(p.own.clone()), "{case}: kept at load");
        assert!(h.store_bytes("auth/prod/identity").is_some(), "{case}");

        h.dcg.take_requests();
        assert_eq!(restarted.ensure_provisioned().await.unwrap().own, p.own, "{case}: enrolled, nothing to send");
        assert!(h.dcg.requests().is_empty(), "{case}");

        restarted.services_token(Environment::Prod, false).await.unwrap();
        let new = restarted.own_client().unwrap().id;
        assert_ne!(new, p.own.id, "{case}");

        let ops: Vec<String> = h.dcg.requests().iter().map(|r| r.op().to_string()).take(4).collect();
        assert_eq!(ops, ["GenerateNonce", "GenerateNonce", "CreateIdentity", "EnrollDevice"], "{case}");
        assert_eq!(nonce_device_ids(&h)[..2], [p.own.id.clone(), new.clone()], "{case}");
        assert_enrolled_again(&h, &restarted, &p.own.id, &new, &phone.id).await;
        assert!(h.store_bytes(PAIR).is_some(), "{case}: the new identity's key is stored");
    }
}

/// A lost identity that cannot be created anew is kept: without a signed-in account whose id the
/// environment stores, or when `CreateIdentity` fails, the request fails and the identity stays; an
/// unlinked device on `CreateIdentity` clears it with its account.
#[tokio::test]
async fn a_lost_identity_that_cannot_be_created_anew_is_kept() {
    // The environment's identity belongs to another account than the signed-in one.
    let h = Harness::new().await;
    let (service, p) = h.provisioned().await;
    drop(service);
    h.store.store("auth/prod/account-id".into(), b"someone-else".to_vec()).unwrap();

    let restarted = h.service();
    h.dcg.script("SignIn", 400, &error_body(Some(2), "InvalidCertificate"));
    let err = restarted.services_token_for(Environment::Prod, true).await.unwrap_err();
    assert_matches!(err, AuthError::NotProvisioned(m) if m.contains("no signed-in account"));
    assert_eq!(h.dcg.count("CreateIdentity"), 1, "only the first provisioning created one");
    assert_eq!(restarted.own_client(), Some(p.own.clone()), "kept");

    // CreateIdentity fails: kept, and the next refresh signs it in (here the service took it back).
    let h = Harness::new().await;
    let (service, p) = h.provisioned().await;
    h.dcg.script("SignIn", 400, &error_body(Some(2), "InvalidCertificate"));
    h.dcg.script("CreateIdentity", 503, "");
    let err = service.services_token_for(Environment::Prod, true).await.unwrap_err();
    assert!(matches!(&err, AuthError::Rest(_)), "{err:?}");
    assert_eq!(service.own_client(), Some(p.own.clone()));
    assert_eq!(h.dcg.count("EnrollDevice"), 1, "nothing enrolled again");
    assert!(service.services_token_for(Environment::Prod, true).await.is_ok());
    assert_eq!(service.own_client(), Some(p.own.clone()));

    // CreateIdentity answers that the device is unlinked: the identity and its account go.
    let h = Harness::new().await;
    let (service, _) = h.provisioned().await;
    h.dcg.script("SignIn", 400, &error_body(Some(2), "InvalidCertificate"));
    h.dcg.script("CreateIdentity", 403, &error_body(Some(13), "DeviceUnlinked"));
    assert_matches!(service.services_token_for(Environment::Prod, true).await.unwrap_err(), AuthError::UnlinkedDevice);
    assert!(service.own_client().is_none());
    assert!(service.account_info().is_none());
    assert!(h.store_bytes("auth/prod/identity").is_none());
}

/// A re-enrollment that fails after the identity was created anew is retried by the next trust
/// synchronization; until then the new identity counts as not enrolled.
#[tokio::test]
async fn a_failed_re_enrollment_is_retried_by_the_next_trust_synchronization() {
    let h = Harness::new().await;
    let (service, p, phone, _) = h.with_phone().await;

    h.dcg.script("SignIn", 400, &error_body(Some(2), "InvalidCertificate"));
    h.dcg.script("EnrollDevice", 503, "");
    assert!(service.services_token_for(Environment::Prod, true).await.is_ok(), "the token is the new identity's");
    let new = service.own_client().unwrap().id;
    assert_eq!(service.account_info().unwrap().dcg_client_id.as_deref(), Some(p.own.id.as_str()));
    assert!(!service.is_trusted(&prod(&phone.id)), "the old identity's relationships are gone");

    // A client-info refresh does not set device info for an identity that is not enrolled.
    service.set_capabilities(vec!["DEVICE_STATUS".into()]);
    assert_matches!(service.refresh_client_info().await.unwrap_err(), AuthError::NotProvisioned(_));
    assert_eq!(h.dcg.count("SetDeviceInfo"), 0);

    h.dcg.take_requests();
    let peers = service.sync_trust(FetchMode::Default).await.unwrap();
    let ops: Vec<String> = h.dcg.requests().iter().map(|r| r.op().to_string()).collect();
    assert_eq!(ops, ["EnrollDevice", "GetDeviceInfoList", "GetAccountCert"]);
    // The old identity's record stays on the account as long as the service lists it.
    let ids: Vec<&str> = peers.iter().map(|peer| peer.client.id.as_str()).collect();
    assert!(ids.contains(&phone.id.as_str()) && !ids.contains(&new.as_str()), "{ids:?}");
    assert_enrolled_again(&h, &service, &p.own.id, &new, &phone.id).await;
}

/// A stale own device-list entry whose repair finds the identity rejected: the identity is created
/// anew and enrolled, and the refresh fetches the device list again for it.
#[tokio::test]
async fn an_own_entry_repair_that_creates_the_identity_anew_fetches_the_list_again() {
    let h = Harness::new().await;
    let (service, p, phone, _) = h.with_phone().await;
    let stale = auth_world::jwt::b64(&KeyPair::generate("trust_z", h.clock.now()).unwrap().cert.der);
    h.dcg.own_entry_mut(&p.own.id, |e| {
        e.as_mut().unwrap().certificates.as_mut().unwrap().self_signed = Some(vec![stale])
    });

    h.advance(Duration::from_secs(25 * 3600));
    h.dcg.script("SignIn", 400, &error_body(Some(2), "InvalidCertificate"));
    h.dcg.take_requests();
    let peers = service.sync_trust(FetchMode::ForceCloud).await.unwrap();

    let new = service.own_client().unwrap().id;
    assert_ne!(new, p.own.id);
    // The old identity's record stays on the account as long as the service lists it.
    let ids: Vec<&str> = peers.iter().map(|peer| peer.client.id.as_str()).collect();
    assert!(ids.contains(&phone.id.as_str()) && !ids.contains(&new.as_str()), "{ids:?}");
    assert_eq!(h.dcg.count("GetDeviceInfoList"), 2, "fetched again for the new identity");
    assert_eq!(rel(&service.trust_relationships(), &phone.id).unwrap().self_client_id, format!("trust_{new}"));
    assert_enrolled_again(&h, &service, &p.own.id, &new, &phone.id).await;
}

/// An identity unused for 180 d is not deprovisioned or replaced while its account is enrolled: it
/// keeps signing in under its id.
#[tokio::test]
async fn an_expired_identity_keeps_signing_in() {
    let h = Harness::new().await;
    let (service, p, phone, _) = h.with_phone().await;
    drop(service);

    h.advance(Duration::from_secs(181 * DAY));
    h.dcg.take_requests();
    let restarted = h.service();
    assert_eq!(restarted.own_client(), Some(p.own.clone()), "still the PC's own dcgClientId");
    assert_eq!(restarted.ensure_provisioned().await.unwrap(), p);

    let ops: Vec<String> = h.dcg.requests().iter().map(|r| r.op().to_string()).collect();
    assert_eq!(ops, ["GenerateNonce", "SignIn", "SetDeviceInfo"], "the client-info refresh is due; no new identity");
    assert_eq!(nonce_device_ids(&h), std::slice::from_ref(&p.own.id));
    assert_eq!(restarted.account_info().unwrap().state, AccountState::Enrolled);
    assert!(restarted.peers().iter().all(|peer| peer.client.id == phone.id));
}

/// An unlinked device clears the identity and deprovisions only the account enrolled with it, for
/// both identities.
#[tokio::test]
async fn an_unlinked_device_auto_relinks_the_signed_in_account_only() {
    for phone_link in [false, true] {
        let h = if phone_link { Harness::phone_link().await } else { Harness::new().await };
        let (service, _) = h.provisioned().await;
        drop(service);

        // A record of another account, enrolled with another identity.
        let mut infos = h.store_json("auth/account-info");
        let mut other = infos.as_object().unwrap().values().next().unwrap().clone();
        other["account_id"] = "someone-else".into();
        other["dcg_client_id"] = new_guid_lower().into();
        infos["someone-else"] = other;
        h.put_store_json("auth/account-info", &infos);

        let restarted = h.service();
        h.dcg.script("SignIn", 403, &error_body(Some(13), "DeviceUnlinked"));
        let token = restarted.services_token_for(Environment::Prod, true).await.expect("auto re-link");
        assert!(!token.is_empty());

        // The signed-in account is re-enrolled under a new identity; the other account is untouched.
        let left: Vec<String> = h.store_json("auth/account-info").as_object().unwrap().keys().cloned().collect();
        assert_eq!(left.len(), 2, "phone_link={phone_link}: the signed-in account and the other one");
        assert!(left.iter().any(|k| k == "someone-else"), "phone_link={phone_link}");
        assert!(restarted.account_info().is_some(), "phone_link={phone_link}: the signed-in account is enrolled again");
        assert!(restarted.own_client().is_some(), "phone_link={phone_link}: a new identity exists");
    }
}

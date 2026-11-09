//! The startup and provisioning sequence and the identity lifecycle against the fake DCG server,
//! with the MSA tokens from the device-bound broker over the fake account service and web view.

use crate::assert_matches;
use crate::support::auth_world;

use std::sync::Arc;
use std::time::Duration;

use auth_world::fake_dcg::error_body;
use auth_world::fake_msa::Reply;
use auth_world::harness::{Harness, Phone, SEED_PUID, USER_KEY, da_property_json};
use auth_world::jwt;
use phonelink_engine::auth::crypto::KeyPair;
use phonelink_engine::auth::dcg_rest::ServiceRequestStatus;
use phonelink_engine::auth::store::AccountState;
use phonelink_engine::auth::trust::{CryptoTrustType, TrustPolicy, new_relationship};
use phonelink_engine::auth::{AuthError, AuthService, FetchMode, Provisioned, metadata_hash};
use phonelink_engine::edge::{Clock, SecureStore, WebSignInKind};
use phonelink_engine::seams::{TokenError, TokenSource, TrustGate};
use phonelink_engine::tap::TapLayer;
use phonelink_engine::types::{DcgClient, Environment, OpenStatus, new_guid_lower};

const DAY: u64 = 86_400;
const WEA_CLIENT_ID: &str = "ca3b40e4-3001-4842-8f21-49c0045404f8";
/// The RPS DCG scope of Prod, Beta, and Prod's strong-auth scope.
const PROD_SCOPE: &str = "service::msatoken.dcg.microsoft.com::MBI_SSL";
const BETA_SCOPE: &str = "service::msatoken.dcg-beta.microsoft.com::MBI_SSL";
const SAPI_SCOPE: &str = "service::msatoken.dcg.microsoft.com::SAPI";
/// The PUID of a second Microsoft account.
const ACCOUNT_B: &str = "0003BFFD0ACCOUNTB";

/// The trust certificate a `EnrollDevice` / `SetDeviceInfo` request published.
fn published_trust_cert(r: &auth_world::fake_dcg::Recorded) -> Vec<u8> {
    jwt::unb64(r.json()["certificates"]["SelfSigned"][0].as_str().unwrap())
}

fn is_guid_lower(s: &str) -> bool {
    let parts: Vec<&str> = s.split('-').collect();
    parts.iter().map(|p| p.len()).collect::<Vec<_>>() == [8, 4, 4, 4, 12]
        && s.chars().all(|c| c == '-' || c.is_ascii_digit() || ('a'..='f').contains(&c))
}

fn is_hex(s: &str, len: usize) -> bool {
    s.len() == len && s.chars().all(|c| c.is_ascii_digit() || ('a'..='f').contains(&c))
}

#[tokio::test]
async fn silent_provisioning_discovers_creates_the_identity_and_enrolls_with_one_silent_token() {
    let h = Harness::new().await;
    h.provisioned().await;

    let lines: Vec<String> = h.dcg.requests().iter().map(|r| r.line()).collect();
    assert_eq!(
        lines,
        [
            "GET /EnvironmentDiscovery/UserServiceEnvironment?api-version=1.0.0",
            "POST /Auth/GenerateNonce?api-version=1.1.0",
            "POST /Auth/CreateIdentity?api-version=1.1.0",
            "POST /DeviceAuthProxy/EnrollDevice?api-version=1.5.0&pop-device-key=",
        ],
        "steps 2, 5 and 6; the token cached from CreateIdentity serves the enrollment; step 7 is not due"
    );

    // MSA: one silent exchange for the prod DCG scope, as the WEA client; no web view.
    let exchanges = h.msa.exchanges();
    assert_eq!(exchanges.len(), 1);
    assert_eq!(exchanges[0].client_id, WEA_CLIENT_ID);
    assert_eq!(exchanges[0].addresses, [format!("scope={PROD_SCOPE}&api-version=2.0&clientid={WEA_CLIENT_ID}")]);
    assert_eq!((exchanges[0].inline_ux.as_str(), exchanges[0].inline_ft.as_deref()), ("Silent", None));
    assert!(h.web.kinds().is_empty());
}

#[tokio::test]
async fn every_provisioning_call_carries_the_client_headers_the_tokens_and_its_own_trace() {
    let h = Harness::new().await;
    h.provisioned().await;

    let ident = h.ident();
    let reqs = h.dcg.requests();
    let msa = "msa-at-1";
    let mut correlation_ids = Vec::new();
    for r in &reqs {
        for (name, value) in ident.headers() {
            assert_eq!(r.header(name), Some(value), "{name} on {}", r.path);
        }

        let tp = r.header("traceparent").expect("traceparent");
        let parts: Vec<&str> = tp.split('-').collect();
        assert_eq!(parts.len(), 4, "{tp}");
        assert_eq!(parts[0], "00");
        assert!(is_hex(parts[1], 32) && is_hex(parts[2], 16), "{tp}");
        assert_eq!(parts[3], "00");
        correlation_ids.push(parts[2].to_string());

        let ts = r.header("tracestate").expect("tracestate");
        assert_eq!(ts, format!("yppHttpParent={}", parts[2]));
        assert_eq!(r.header("DCG-TraceState"), Some(ts));

        assert_eq!(r.header("AuthorizationPortal"), Some("MSM"), "the RPS scope family's portal");
        assert_eq!(r.header("Authorization"), Some(format!("Bearer {msa}").as_str()));
        assert_eq!(r.header("Authorization-Type"), Some("MSA"));

        let auth_call = r.path.starts_with("Auth/");
        assert_eq!(r.header("UserIdentityType"), auth_call.then_some("MSA"), "{}", r.path);
        assert_eq!(r.header("UserIdentityToken"), auth_call.then_some(msa), "{}", r.path);
        let dcg_token = r.op() == "EnrollDevice";
        assert_eq!(r.header("Dcg-Token"), dcg_token.then_some("svc-token-1"), "{}", r.path);
        assert_eq!(r.header("Content-Type"), (r.method == "POST").then_some("application/json"), "{}", r.path);
        assert_eq!(r.env, "Prod");
    }

    correlation_ids.sort();
    correlation_ids.dedup();
    assert_eq!(correlation_ids.len(), reqs.len(), "a trace context (correlation id) per request");

    let nonce_trace = reqs[1].header("traceparent").unwrap().split('-').nth(1).unwrap().to_string();
    let create_trace = reqs[2].header("traceparent").unwrap().split('-').nth(1).unwrap().to_string();
    assert_eq!(nonce_trace, create_trace, "GenerateNonce and CreateIdentity share one scenario trace id");
}

/// `GenerateNonce(newGuid)` → `CreateIdentity` with a nonce JWT of `CN=<newGuid>`, whose key is
/// stored with its certificate.
#[tokio::test]
async fn create_identity_signs_the_nonce_with_a_new_identity_certificate() {
    let h = Harness::new().await;
    let (_, p) = h.provisioned().await;

    let reqs = h.dcg.requests();
    let device_id = p.own.id.clone();
    assert!(is_guid_lower(&device_id), "{device_id}");
    assert_eq!(reqs[1].body, format!(r#"{{"deviceId":"{device_id}"}}"#));

    let create = reqs[2].json();
    assert_eq!(create.as_object().unwrap().len(), 1);
    let j = jwt::decode(create["certificateJWT"].as_str().unwrap());
    assert_eq!(j.header, r#"{"alg":"ES384","typ":"JWT"}"#);
    assert_eq!(j.str("iss"), device_id);
    assert_eq!(j.str("Nonce"), "nonce-1");

    let identity_der = jwt::unb64(j.str("Certificate"));
    assert_eq!(jwt::cert_cn(&identity_der), device_id);
    assert!(jwt::verifies(&j, &identity_der));

    assert_eq!(h.identity_cert(Environment::Prod), identity_der);
    let (key, _) = h.pair_entry("auth/prod/identity-pair").unwrap();
    assert_eq!(
        KeyPair::from_parts(&key, &identity_der).unwrap().cn(),
        device_id,
        "the key is stored with its certificate"
    );
}

#[tokio::test]
async fn enroll_device_publishes_the_metadata_and_the_trust_certificate() {
    let h = Harness::new().await;
    let (_, p) = h.provisioned().await;

    let device_id = p.own.id.clone();
    let enroll = &h.dcg.of("EnrollDevice")[0];
    let trust_b64 = enroll.json()["certificates"]["SelfSigned"][0].as_str().unwrap().to_string();
    assert_eq!(
        enroll.body,
        format!(
            concat!(
                r#"{{"metadata":{{"capabilities":["DEVICE_STATUS","UNLINKABLE_V1"],"clientType":"WEA","#,
                r#""clientVersion":"1.26092.74.0","consentVersion":0,"customData":"","displayName":"test-mac","#,
                r#""isEnabled":true,"manufacture":"Unknown","modelName":"Unknown","modelVersion":"Unknown","#,
                r#""osName":"Windows","osVersion":"10.0.26100"}},"certificates":{{"SelfSigned":["{}"]}}}}"#
            ),
            trust_b64
        )
    );

    let trust_der = jwt::unb64(&trust_b64);
    assert_eq!(jwt::cert_cn(&trust_der), format!("trust_{device_id}"));
    assert_eq!(h.trust_cert(Environment::Prod), trust_der, "persisted after the enrollment succeeded");
}

#[tokio::test]
async fn an_enrollment_records_the_account_and_its_a2d_relationship() {
    let h = Harness::new().await;
    let (service, p) = h.provisioned().await;
    let device_id = p.own.id.clone();
    let enroll = &h.dcg.of("EnrollDevice")[0];

    assert_eq!(p.env, Environment::Prod);
    assert_eq!(service.own_client(), Some(p.own.clone()));
    assert_eq!(service.environment(), Some(Environment::Prod));

    let account = service.local_account().unwrap();
    let info = service.account_info().unwrap();
    let now = h.clock.now();
    assert_eq!(info.state, AccountState::Enrolled);
    assert_eq!(info.environment, Some(Environment::Prod));
    assert_eq!(info.dcg_client_id.as_deref(), Some(device_id.as_str()));
    assert_eq!(info.last_update_time, Some(now));
    assert_eq!(info.last_attempt_update_time, Some(now));
    assert_eq!(info.msa_scope.as_deref(), Some(PROD_SCOPE));

    let sent: phonelink_engine::auth::dcg_rest::DeviceInfoBody = serde_json::from_str(&enroll.body).unwrap();
    assert_eq!(info.metadata_hash, Some(metadata_hash(&sent.metadata)));

    let rels = service.trust_relationships();
    assert_eq!(rels.len(), 1);
    let a2d = &rels[0];
    let account_dcg_id = h.dcg.account_id();
    assert_eq!(a2d.trust_type, CryptoTrustType::A2D);
    assert_eq!(a2d.self_client_id, format!("trust_{device_id}"));
    assert_eq!(a2d.partner_client_id, format!("trust_{account_dcg_id}"));
    assert_eq!(a2d.msa_cid.as_deref(), Some(account.cid.as_str()));
    assert_eq!(a2d.partner_certificate, h.dcg.account_cert());
    assert_eq!(a2d.attributes.get("dcg_prod_client_id"), Some(&account_dcg_id));
    assert_eq!(a2d.attributes.len(), 1);
    assert_eq!(a2d.relationship_last_access_time, now);

    assert_eq!(h.dcg.with(|s| s.enrolled.len()), 1);
}

#[tokio::test]
async fn every_dcg_call_is_tapped_with_its_credentials_redacted() {
    let h = Harness::new().await;
    h.provisioned().await;

    let rest: Vec<_> = h.tap.snapshot().into_iter().filter(|e| e.layer == TapLayer::Rest).collect();
    assert_eq!(rest.len(), 4, "the four DCG calls");
    let text = serde_json::to_string(&rest).unwrap();
    assert!(!text.contains("msa-at-1") && !text.contains("svc-token-1"), "{text}");

    let enroll_tap = rest.iter().find(|e| e.summary["call"] == "EnrollDevice").unwrap();
    assert_eq!(enroll_tap.summary["status"], 200);
    assert_eq!(enroll_tap.summary["method"], "POST");
    assert!(
        enroll_tap.summary["url"]
            .as_str()
            .unwrap()
            .ends_with("/DeviceAuthProxy/EnrollDevice?api-version=1.5.0&pop-device-key=")
    );

    let create_tap = rest.iter().find(|e| e.summary["call"] == "CreateIdentity").unwrap();
    assert!(create_tap.summary["response_body"]["accessToken"].as_str().unwrap().starts_with("<redacted"));
}

#[tokio::test]
async fn provisioning_persists_what_survives_a_restart_but_not_the_services_token() {
    let h = Harness::new().await;
    h.provisioned().await;

    let keys = h.store.keys();
    for key in [
        "auth/environment",
        "auth/account",
        "auth/account-info",
        "auth/trust-relationships",
        "auth/prod/identity",
        "auth/prod/account-id",
        "auth/prod/identity-pair",
        "auth/prod/trust-pair",
        USER_KEY,
    ] {
        assert!(keys.iter().any(|k| k == key), "{key} persisted: {keys:?}");
    }

    assert!(!keys.iter().any(|k| k == "auth/prod/identity-pair-pending"), "no key rotation is pending");

    for key in &keys {
        let value = h.store.load(key.clone()).unwrap().unwrap();
        assert!(!String::from_utf8_lossy(&value).contains("svc-token"), "{key} holds a services token");
    }
}

#[tokio::test]
async fn restart_skips_enrollment_and_re_signs_in() {
    let h = Harness::new().await;
    let (service, p) = h.provisioned().await;
    let identity_cert = h.identity_cert(Environment::Prod);
    drop(service);
    h.dcg.take_requests();

    let restarted = h.service();
    assert_eq!(restarted.ensure_provisioned().await.unwrap(), p);
    assert!(h.dcg.requests().is_empty(), "an Enrolled account makes no network call");

    let token = restarted.services_token(Environment::Prod, false).await.unwrap();
    assert_eq!(token, "svc-token-2");
    assert_eq!(h.dcg.lines(), ["POST /Auth/GenerateNonce?api-version=1.1.0", "POST /Auth/SignIn?api-version=1.1.0"]);

    let sign_in = &h.dcg.of("SignIn")[0];
    let j = jwt::decode(sign_in.json()["certificateJWT"].as_str().unwrap());
    assert_eq!(j.str("iss"), p.own.id);
    assert_eq!(jwt::unb64(j.str("Certificate")), identity_cert, "the existing identity certificate");
    assert_eq!(sign_in.header("UserIdentityToken"), Some("msa-at-2"), "the resumed MSA token");
    assert_eq!(h.dcg.of("GenerateNonce")[0].body, format!(r#"{{"deviceId":"{}"}}"#, p.own.id));
    assert_eq!(restarted.cached_token_device_id(Environment::Prod).as_deref(), Some(p.own.id.as_str()));
}

#[tokio::test]
async fn beta_environment_from_discovery() {
    let h = Harness::new().await;
    h.dcg.with(|s| s.service_environment = Some("Beta".into()));
    let (service, p) = h.provisioned().await;

    assert_eq!(p.env, Environment::Beta);
    assert_eq!(p.own.env, Environment::Beta);

    let reqs = h.dcg.requests();
    assert_eq!(reqs[0].op(), "UserServiceEnvironment");
    assert_eq!(reqs[0].env, "Prod", "discovery always uses the prod base");
    assert_eq!(reqs[0].header("Authorization"), Some("Bearer msa-at-1"), "with the prod-scope token");
    for r in &reqs[1..] {
        assert_eq!(r.env, "Beta", "{}", r.path);
        assert_eq!(r.header("Authorization"), Some("Bearer msa-at-2"), "the Beta-scope token on {}", r.path);
    }

    assert_eq!(h.msa.scopes(), [PROD_SCOPE, BETA_SCOPE]);
    assert_eq!(h.store_bytes("auth/environment"), Some(b"Beta".to_vec()));

    let a2d = &service.trust_relationships()[0];
    assert!(a2d.attributes.contains_key("dcg_beta_client_id"));
    assert!(h.store_bytes("auth/beta/identity").is_some());
    assert!(h.store_bytes("auth/prod/identity").is_none());
}

fn set_account_state(h: &Harness, state: &str) {
    let mut infos = h.store_json("auth/account-info");
    for (_, info) in infos.as_object_mut().unwrap() {
        info["state"] = serde_json::json!(state);
    }
    h.put_store_json("auth/account-info", &infos);
}

#[tokio::test]
async fn defaulted_environment_keeps_the_saved_one_only_with_identity_and_trust() {
    let h = Harness::new().await;
    h.dcg.with(|s| s.service_environment = Some("Beta".into()));
    let (service, p) = h.provisioned().await;
    drop(service);

    // Provisioned (not Enrolled) in Beta, discovery failing → Defaulted(Prod) → Beta is kept.
    set_account_state(&h, "Provisioned");
    h.dcg.take_requests();
    h.dcg.script("UserServiceEnvironment", 500, "");
    let restarted = h.service();
    let again = restarted.ensure_provisioned().await.unwrap();
    assert_eq!(again, p, "same environment and identity");

    let lines: Vec<(String, &str)> = h.dcg.requests().iter().map(|r| (r.op().to_string(), r.env)).collect();
    assert_eq!(
        lines,
        [
            ("UserServiceEnvironment".to_string(), "Prod"),
            ("GenerateNonce".to_string(), "Beta"),
            ("SignIn".to_string(), "Beta"),
            ("EnrollDevice".to_string(), "Beta"),
        ],
        "the active identity is reused"
    );
    drop(restarted);

    // Without an unexpired relationship the snap goes to Prod, which conflicts with the
    // Provisioned record in Beta.
    set_account_state(&h, "Provisioned");
    h.put_store_json("auth/trust-relationships", &serde_json::json!([]));
    h.dcg.script("UserServiceEnvironment", 503, "");
    let restarted = h.service();
    let err = restarted.ensure_provisioned().await.unwrap_err();
    assert!(matches!(err, AuthError::MultipleAccountNotSupported(_)), "{err:?}");
}

#[tokio::test]
async fn discovery_failures() {
    // 401 aborts.
    let h = Harness::new().await;
    h.dcg.script("UserServiceEnvironment", 401, &error_body(Some(1), "InvalidAuthHeader"));
    let service = h.signed_in().await;
    assert!(matches!(service.ensure_provisioned().await, Err(AuthError::Unauthorized(_))));
    assert_eq!(h.dcg.requests().len(), 1);

    // An unknown or missing value is Defaulted(Prod).
    for value in [Some("Mars".to_string()), None] {
        let h = Harness::new().await;
        h.dcg.with(|s| s.service_environment = value.clone());
        let (_, p) = h.provisioned().await;
        assert_eq!(p.env, Environment::Prod, "{value:?}");
    }
}

#[tokio::test]
async fn create_identity_collision_retries_with_a_new_guid() {
    let h = Harness::new().await;
    h.dcg.script("CreateIdentity", 403, "");
    let (_, p) = h.provisioned().await;

    let nonces = h.dcg.of("GenerateNonce");
    assert_eq!(nonces.len(), 2);
    let first = nonces[0].json()["deviceId"].as_str().unwrap().to_string();
    let second = nonces[1].json()["deviceId"].as_str().unwrap().to_string();
    assert_ne!(first, second, "a device-id collision retries with a new GUID");
    assert_eq!(p.own.id, second);

    let creates = h.dcg.of("CreateIdentity");
    assert_eq!(jwt::decode(creates[0].json()["certificateJWT"].as_str().unwrap()).str("iss"), first);
    assert_eq!(jwt::decode(creates[1].json()["certificateJWT"].as_str().unwrap()).str("iss"), second);
}

#[tokio::test]
async fn create_identity_invalid_nonce_retries_with_a_new_nonce() {
    let h = Harness::new().await;
    h.dcg.script("CreateIdentity", 400, &error_body(Some(2), "InvalidNonce"));
    let (_, p) = h.provisioned().await;

    let nonces = h.dcg.of("GenerateNonce");
    assert_eq!(nonces.len(), 2);
    assert_eq!(nonces[0].body, nonces[1].body, "same GUID");

    let creates = h.dcg.of("CreateIdentity");
    let n1 = jwt::decode(creates[0].json()["certificateJWT"].as_str().unwrap()).str("Nonce").to_string();
    let n2 = jwt::decode(creates[1].json()["certificateJWT"].as_str().unwrap()).str("Nonce").to_string();
    assert_eq!((n1.as_str(), n2.as_str()), ("nonce-1", "nonce-2"));
    assert_eq!(p.own.id, nonces[0].json()["deviceId"].as_str().unwrap());
}

#[tokio::test]
async fn create_identity_gives_up_after_three_retries() {
    let h = Harness::new().await;
    for _ in 0..4 {
        h.dcg.script("CreateIdentity", 403, "");
    }

    let service = h.signed_in().await;
    let err = service.ensure_provisioned().await.unwrap_err();

    match err {
        AuthError::Rest(e) => assert_eq!(e.status(), Some(ServiceRequestStatus::ForbiddenDcgRequest)),
        other => panic!("{other:?}"),
    }

    assert_eq!(h.dcg.count("CreateIdentity"), 4, "first attempt + 3 retries");
    assert_eq!(h.dcg.count("EnrollDevice"), 0);
    assert!(service.own_client().is_none());

    let h = Harness::new().await;
    h.dcg.script("CreateIdentity", 400, &error_body(Some(2), "InvalidCertificate"));
    let service = h.signed_in().await;
    assert_matches!(service.ensure_provisioned().await.unwrap_err(), AuthError::InvalidIdentity);
}

#[tokio::test]
async fn unlinked_device_on_auth_deprovisions_then_auto_relinks() {
    let h = Harness::new().await;
    let (service, _, _, _) = h.with_phone().await;

    // The server unlinks the device: the refresh auto re-links once (a new identity, enrolled again).
    h.dcg.script("SignIn", 403, &error_body(Some(13), "DeviceUnlinked"));
    let token = service.services_token_for(Environment::Prod, true).await.expect("auto re-link");
    assert!(!token.is_empty());
    assert!(service.account_info().is_some(), "the account is enrolled again under the new identity");
    assert!(service.own_client().is_some(), "a new identity exists");

    // The recovery is bounded: a second unlink falls back to the unlinked error.
    h.dcg.script("SignIn", 403, &error_body(Some(13), "DeviceUnlinked"));
    let err = service.services_token_for(Environment::Prod, true).await.unwrap_err();
    assert_matches!(err, AuthError::UnlinkedDevice);
}

#[tokio::test]
async fn client_info_refresh_rules() {
    let h = Harness::new().await;
    let (service, _) = h.provisioned().await;
    let enroll_body = h.dcg.of("EnrollDevice")[0].body.clone();
    drop(service);
    h.dcg.take_requests();

    // Not due on the next start.
    let s = h.service();
    s.ensure_provisioned().await.unwrap();
    assert!(h.dcg.requests().is_empty());

    // 7 d after the last update (and > 1 d after the last attempt): SetDeviceInfo.
    h.advance(Duration::from_secs(7 * DAY + 1));
    s.ensure_provisioned().await.unwrap();
    assert_eq!(
        h.dcg.lines(),
        [
            "POST /Auth/GenerateNonce?api-version=1.1.0",
            "POST /Auth/SignIn?api-version=1.1.0",
            "POST /DeviceAuthProxy/SetDeviceInfo?api-version=1.5.0&pop-device-key=",
        ]
    );

    let set = &h.dcg.of("SetDeviceInfo")[0];
    assert_eq!(set.body, enroll_body, "same metadata and trust certificate");
    assert_eq!(set.header("Dcg-Token"), Some("svc-token-2"));
    assert_eq!(set.header("Authorization-Type"), Some("MSA"));
    assert!(set.header("Authorization").unwrap().starts_with("Bearer msa-at-"));
    assert_eq!(set.header("UserIdentityToken"), None);

    let now = h.clock.now();
    let info = s.account_info().unwrap();
    assert_eq!((info.last_update_time, info.last_attempt_update_time), (Some(now), Some(now)));
    h.dcg.take_requests();

    // 6 d later: not due.
    h.advance(Duration::from_secs(6 * DAY));
    s.ensure_provisioned().await.unwrap();
    assert_eq!(h.dcg.count("SetDeviceInfo"), 0);

    // A capability change (an options change) changes the hash: due at once.
    s.set_capabilities(vec!["DEVICE_STATUS".into(), "UNLINKABLE_V1".into(), "CLIPBOARD".into()]);
    s.refresh_client_info().await.unwrap();

    let set = h.dcg.of("SetDeviceInfo");
    assert_eq!(set.len(), 1);
    assert_eq!(
        set[0].json()["metadata"]["capabilities"],
        serde_json::json!(["DEVICE_STATUS", "UNLINKABLE_V1", "CLIPBOARD"])
    );
    let hash_after = s.account_info().unwrap().metadata_hash;

    // A failure only moves LastAttemptUpdateTime.
    h.advance(Duration::from_secs(60));
    let before = s.account_info().unwrap();
    s.set_capabilities(vec!["DEVICE_STATUS".into(), "UNLINKABLE_V1".into()]);
    h.dcg.script("SetDeviceInfo", 500, "");
    s.refresh_client_info().await.unwrap();

    let after = s.account_info().unwrap();
    assert_eq!(after.last_update_time, before.last_update_time);
    assert_eq!(after.last_attempt_update_time, Some(h.clock.now()));
    assert_eq!(after.metadata_hash, hash_after);
    assert_eq!(after.state, AccountState::Enrolled);

    // A token failure is not a SetDeviceInfo result: nothing is recorded or deprovisioned.
    h.advance(Duration::from_secs(25 * 3600));
    s.set_capabilities(vec!["DEVICE_STATUS".into()]);
    h.dcg.script("SignIn", 401, &error_body(Some(1), "InvalidAuthHeader"));
    let before = s.account_info().unwrap();
    s.refresh_client_info().await.unwrap();
    assert_eq!(s.account_info().unwrap(), before);
    assert_eq!(h.dcg.count("SetDeviceInfo"), 2, "no request was sent");

    // A SetDeviceInfo Unauthorized deprovisions the account.
    h.dcg.script("SetDeviceInfo", 401, &error_body(Some(1), "InvalidAuthHeader"));
    assert!(matches!(s.refresh_client_info().await, Err(AuthError::Unauthorized(_))));
    assert!(s.account_info().is_none());
}

#[tokio::test]
async fn unparsable_account_certificate_is_account_trust_sync_failure() {
    let h = Harness::new().await;
    h.dcg.script("EnrollDevice", 200, r#"{"accountCert":"%%% not base64"}"#);
    let service = h.signed_in().await;

    let err = service.ensure_provisioned().await.unwrap_err();
    assert!(matches!(err, AuthError::AccountTrustSyncFailure(_)), "{err:?}");
    assert_eq!(service.account_info().unwrap().state, AccountState::Provisioned, "the account stays Provisioned");
    assert!(service.trust_relationships().is_empty());
    assert!(h.store_bytes("auth/prod/trust-pair").is_some(), "the trust certificate was accepted by the server");

    // The next attempt reuses the identity and enrolls.
    let p = service.ensure_provisioned().await.unwrap();
    assert_eq!(service.account_info().unwrap().state, AccountState::Enrolled);
    assert_eq!(h.dcg.count("CreateIdentity"), 1);
    assert_eq!(h.dcg.count("EnrollDevice"), 2);
    let trust_certs: Vec<String> = h
        .dcg
        .of("EnrollDevice")
        .iter()
        .map(|r| r.json()["certificates"]["SelfSigned"][0].as_str().unwrap().to_string())
        .collect();
    assert_eq!(trust_certs[0], trust_certs[1], "the persisted trust certificate is reused");
    assert_eq!(p.own.id, h.dcg.of("GenerateNonce")[0].json()["deviceId"].as_str().unwrap());
}

#[tokio::test]
async fn enroll_failures() {
    let h = Harness::new().await;
    h.dcg.script("EnrollDevice", 403, &error_body(Some(13), "DeviceUnlinked"));
    let service = h.signed_in().await;
    assert_matches!(service.ensure_provisioned().await.unwrap_err(), AuthError::UnlinkedDevice);
    assert!(service.own_client().is_none());
    assert!(h.store_bytes("auth/prod/identity").is_none());

    let h = Harness::new().await;
    h.dcg.script("EnrollDevice", 500, "");
    let service = h.signed_in().await;
    assert!(matches!(service.ensure_provisioned().await, Err(AuthError::Rest(_))));
    assert_eq!(h.dcg.count("EnrollDevice"), 1, "only Unauthorized is retried with a strong-auth token");
    assert!(h.store_bytes("auth/prod/trust-pair").is_none(), "a rejected trust certificate is not persisted");
    assert_eq!(service.account_info().unwrap().state, AccountState::Provisioned);
}

#[tokio::test]
async fn enroll_unauthorized_is_retried_with_a_silent_strong_auth_token() {
    let h = Harness::new().await;
    h.dcg.script("EnrollDevice", 401, &error_body(Some(4), "StrongAuthRequired"));
    let (service, p) = h.provisioned().await;

    let enrolls = h.dcg.of("EnrollDevice");
    assert_eq!(enrolls.len(), 2);
    assert_eq!(enrolls[0].header("Authorization"), Some("Bearer msa-at-1"), "the DCG-scope token first");
    assert_eq!(enrolls[1].header("Authorization"), Some("Bearer msa-at-2"), "then the strong-auth token");

    for e in &enrolls {
        assert_eq!(e.header("Dcg-Token"), Some("svc-token-1"), "Dcg-Token unchanged");
        assert_eq!(e.header("Authorization-Type"), Some("MSA"));
        assert_eq!(e.query("pop-device-key").as_deref(), Some(""));
    }
    assert_eq!(enrolls[0].body, enrolls[1].body, "the same metadata and trust certificate");

    // The strong-auth token came from a silent exchange for the SAPI scope; nobody was asked.
    assert_eq!(h.msa.scopes(), [PROD_SCOPE, SAPI_SCOPE]);
    let exchanges = h.msa.exchanges();
    assert!(exchanges.iter().all(|e| e.client_id == WEA_CLIENT_ID && !e.is_interactive()), "{exchanges:#?}");
    assert!(h.web.kinds().is_empty(), "no web view");
    assert_eq!(service.account_info().unwrap().state, AccountState::Enrolled);
    assert_eq!(h.trust_cert(Environment::Prod), published_trust_cert(&enrolls[1]));
    assert_eq!(service.own_client(), Some(p.own));
}

/// The service refuses the silent strong-auth exchange (it no longer accepts the session's user
/// token): the broker signs in again through the web view, as the same account, and the exchange
/// that follows spends the new page's inline flow token.
#[tokio::test]
async fn a_refused_strong_auth_exchange_signs_in_again() {
    let h = Harness::new().await;
    h.dcg.script("EnrollDevice", 401, &error_body(Some(1), "InvalidAuthHeader"));
    let service = h.signed_in_interactively().await;
    assert_eq!(h.web.kinds(), [WebSignInKind::Login]);

    h.msa.with(|s| s.refuse_silent = true);
    service.ensure_provisioned().await.unwrap();

    assert_eq!(h.web.kinds(), [WebSignInKind::Login, WebSignInKind::Login], "the sign-in, then the sign-in again");
    let exchanges: Vec<(String, String, Option<String>)> = h
        .msa
        .exchanges()
        .into_iter()
        .map(|e| (e.scope().to_string(), e.inline_ux.clone(), e.inline_ft.clone()))
        .collect();
    let expected = [
        (PROD_SCOPE, "TokenBroker", Some("ft-1")),
        (SAPI_SCOPE, "Silent", None),
        (SAPI_SCOPE, "TokenBroker", Some("ft-2")),
    ]
    .map(|(scope, ux, ft)| (scope.to_string(), ux.to_string(), ft.map(str::to_string)));
    assert_eq!(exchanges, expected, "the sign-in's page token is spent by the DCG-scope exchange");

    let enrolls = h.dcg.of("EnrollDevice");
    assert_eq!(enrolls.len(), 2);
    assert_eq!(enrolls[0].header("Authorization"), Some("Bearer msa-at-1"));
    assert_eq!(enrolls[1].header("Authorization"), Some("Bearer msa-at-2"), "the token after the sign-in again");
    assert_eq!(enrolls[1].header("Dcg-Token"), enrolls[0].header("Dcg-Token"));
    assert_eq!(service.account_info().unwrap().state, AccountState::Enrolled);
    assert_eq!(h.session_puid().as_deref(), Some(SEED_PUID));
    assert!(!service.needs_user_attention());
}

/// The strong-auth token is the account being enrolled's: when the page the broker shows for it —
/// the sign-in again after a refused silent exchange, or a consent — signs in another account, the
/// broker forgets the session and the enrollment fails as needing the user; the local account and
/// its identity stay, and no token of the other account is issued.
#[tokio::test]
async fn a_strong_auth_page_that_signs_in_another_account_is_refused() {
    for case in ["sign-in again", "consent"] {
        let h = Harness::new().await;
        h.dcg.script("EnrollDevice", 401, &error_body(Some(1), "InvalidAuthHeader"));
        let service = h.signed_in().await;

        // The DCG-scope token is cached; only the strong-auth exchange meets the other account.
        service.msa().access_token(Environment::Prod).await.unwrap();
        if case == "sign-in again" {
            h.msa.with(|s| s.refuse_silent = true);
            h.web.sign_in_as(ACCOUNT_B);
        } else {
            h.msa.script(Reply::Consent);
            h.web.queue(Ok(da_property_json(ACCOUNT_B)));
        }

        let err = service.ensure_provisioned().await.unwrap_err();
        assert!(matches!(err, AuthError::UserInteractionRequired(_)), "{case}: {err:?}");
        assert_eq!(err.to_token_error().open_status(), OpenStatus::MsaTokenUserInteractionRequired, "{case}");
        assert_eq!(h.dcg.count("EnrollDevice"), 1, "{case}: no enrollment with that account's token");
        assert_eq!(h.msa.with(|s| s.issued), 1, "{case}: only the DCG-scope token was issued");
        assert_eq!(h.session_puid(), None, "{case}: the session is forgotten");
        assert!(service.needs_user_attention(), "{case}");
        assert_eq!(service.local_account().unwrap().puid.as_deref(), Some(SEED_PUID), "{case}");
        assert_eq!(service.account_info().unwrap().state, AccountState::Provisioned, "{case}");
    }
}

#[tokio::test]
async fn a_second_enroll_unauthorized_fails_and_drops_both_tokens() {
    let h = Harness::new().await;
    h.dcg.script("EnrollDevice", 401, &error_body(Some(1), "InvalidAuthHeader"));
    h.dcg.script("EnrollDevice", 401, &error_body(Some(4), "StrongAuthRequired"));

    let service = h.signed_in().await;
    assert!(matches!(service.ensure_provisioned().await, Err(AuthError::Unauthorized(_))));
    assert_eq!(h.dcg.count("EnrollDevice"), 2, "one strong-auth retry");
    assert_eq!(service.account_info().unwrap().state, AccountState::Provisioned);
    assert!(h.store_bytes("auth/prod/trust-pair").is_none(), "a rejected trust certificate is not persisted");

    // Neither rejected MSA token is reused: the next enrollment fetches fresh ones.
    h.dcg.script("EnrollDevice", 401, &error_body(Some(4), "StrongAuthRequired"));
    service.ensure_provisioned().await.unwrap();
    let bearers: Vec<String> =
        h.dcg.of("EnrollDevice").iter().map(|r| r.header("Authorization").unwrap().to_string()).collect();
    assert_eq!(bearers, ["Bearer msa-at-1", "Bearer msa-at-2", "Bearer msa-at-3", "Bearer msa-at-4"]);
    assert_eq!(h.msa.scopes(), [PROD_SCOPE, SAPI_SCOPE, PROD_SCOPE, SAPI_SCOPE]);
    assert_eq!(h.dcg.count("CreateIdentity"), 1, "the Provisioned identity is reused");
    assert_eq!(service.account_info().unwrap().state, AccountState::Enrolled);
}

#[tokio::test]
async fn another_accounts_record_is_deprovisioned_first() {
    let h = Harness::new().await;
    let (service, p) = h.provisioned().await;
    drop(service);

    let mut infos = h.store_json("auth/account-info");
    infos["someone-else"] = serde_json::json!({
        "account_id": "someone-else", "msa_cid": "other-cid", "state": "Enrolled", "environment": "Beta",
        "dcg_client_id": "x", "metadata_hash": null, "msa_scope": null
    });
    h.put_store_json("auth/account-info", &infos);

    let restarted = h.service();
    assert_eq!(restarted.ensure_provisioned().await.unwrap(), p);

    let infos = h.store_json("auth/account-info");
    assert!(infos.get("someone-else").is_none());
    assert_eq!(infos.as_object().unwrap().len(), 1);
    assert_eq!(restarted.account_info().unwrap().state, AccountState::Enrolled);
}

#[tokio::test]
async fn sign_in_is_required_before_provisioning() {
    let h = Harness::new().await;
    let service = h.service();
    assert!(matches!(service.ensure_provisioned().await, Err(AuthError::UserInteractionRequired(_))));
    assert!(h.dcg.requests().is_empty());
    assert!(h.web.kinds().is_empty() && h.msa.exchanges().is_empty(), "provisioning never signs in itself");
}

#[tokio::test]
async fn a_missing_or_unusable_trust_pair_is_replaced_on_the_next_send() {
    const PAIR: &str = "auth/prod/trust-pair";
    // Missing: the cloud refresh no longer finds the published certificate among the stored ones,
    // and SetDeviceInfo publishes a new one.
    let h = Harness::new().await;
    let (service, p, phone, _) = h.with_phone().await;
    let old = h.trust_cert(Environment::Prod);
    drop(service);

    h.store.delete(PAIR.into()).unwrap();
    let restarted = h.service();
    assert_eq!(restarted.own_client(), Some(p.own.clone()), "the identity is kept");

    h.dcg.take_requests();
    assert_eq!(restarted.sync_trust(FetchMode::ForceCloud).await.unwrap().len(), 1);
    let ops: Vec<String> = h.dcg.requests().iter().map(|r| r.op().to_string()).collect();
    assert_eq!(ops, ["GetDeviceInfoList", "GenerateNonce", "SignIn", "SetDeviceInfo", "GetAccountCert"]);

    let sent = published_trust_cert(&h.dcg.of("SetDeviceInfo")[0]);
    assert_ne!(sent, old, "a new trust certificate");
    assert_eq!(jwt::cert_cn(&sent), format!("trust_{}", p.own.id));
    assert_eq!(h.trust_cert(Environment::Prod), sent, "persisted once the server accepted it");
    assert!(restarted.is_trusted(&DcgClient::new(phone.id.clone(), Environment::Prod)));

    // A key that does not match its certificate: the next SetDeviceInfo (an options change) sends a
    // new one.
    let h = Harness::new().await;
    let (service, p) = h.provisioned().await;
    let old = h.trust_cert(Environment::Prod);
    drop(service);

    let stray = KeyPair::generate(&format!("trust_{}", p.own.id), h.clock.now()).unwrap();
    h.put_pair_entry(PAIR, &stray.pkcs8().unwrap(), &old);
    let restarted = h.service();
    assert_eq!(restarted.own_client(), Some(p.own.clone()));

    restarted.set_capabilities(vec!["DEVICE_STATUS".into(), "UNLINKABLE_V1".into(), "CLIPBOARD".into()]);
    restarted.refresh_client_info().await.unwrap();

    let sent = published_trust_cert(&h.dcg.of("SetDeviceInfo")[0]);
    assert!(sent != old && sent != stray.cert.der);
    let (key, cert) = h.pair_entry(PAIR).unwrap();
    assert_eq!(cert, sent);
    KeyPair::from_parts(&key, &cert).expect("a consistent pair");

    // Before the account is enrolled, EnrollDevice sends the new one.
    let h = Harness::new().await;
    h.dcg.script("EnrollDevice", 200, r#"{"accountCert":"%%% not base64"}"#);
    let service = h.signed_in().await;
    assert!(matches!(service.ensure_provisioned().await, Err(AuthError::AccountTrustSyncFailure(_))));
    let first = h.trust_cert(Environment::Prod);
    drop(service);

    h.store.store(PAIR.into(), b"torn".to_vec()).unwrap();
    let restarted = h.service();
    restarted.ensure_provisioned().await.unwrap();
    let sent = published_trust_cert(&h.dcg.of("EnrollDevice")[1]);
    assert_ne!(sent, first);
    assert_eq!(h.trust_cert(Environment::Prod), sent);
}

#[tokio::test]
async fn client_info_refresh_attempts_are_throttled_to_one_a_day() {
    let h = Harness::new().await;
    let (service, _) = h.provisioned().await;

    // LastUpdateTime is stale (> 7 d) and the attempt fails.
    h.advance(Duration::from_secs(7 * DAY + 1));
    h.dcg.script("SetDeviceInfo", 500, "");
    service.refresh_client_info().await.unwrap();
    assert_eq!(h.dcg.count("SetDeviceInfo"), 1);
    let info = service.account_info().unwrap();
    assert_eq!(info.last_attempt_update_time, Some(h.clock.now()));

    // Less than 1 d after the failed attempt it is not due, although LastUpdateTime is still stale:
    // neither on an options change that keeps the metadata, nor on the next start.
    h.advance(Duration::from_secs(DAY - 60));
    service.refresh_client_info().await.unwrap();
    drop(service);
    let restarted = h.service();
    restarted.ensure_provisioned().await.unwrap();
    assert_eq!(h.dcg.count("SetDeviceInfo"), 1, "throttled");
    assert_eq!(restarted.account_info().unwrap(), info);

    // Over 1 d after it: due again; the success moves both times.
    h.advance(Duration::from_secs(61));
    restarted.refresh_client_info().await.unwrap();
    assert_eq!(h.dcg.count("SetDeviceInfo"), 2);
    let info = restarted.account_info().unwrap();
    assert_eq!((info.last_update_time, info.last_attempt_update_time), (Some(h.clock.now()), Some(h.clock.now())));
}

#[tokio::test]
async fn unlinked_on_set_device_info_or_unlink_device_deprovisions_everything() {
    for op in ["SetDeviceInfo", "UnlinkDevice"] {
        let h = Harness::new().await;
        let (service, _, phone, _) = h.with_phone().await;
        h.dcg.script(op, 403, &error_body(Some(13), "DeviceUnlinked"));
        let err = if op == "SetDeviceInfo" {
            service.set_capabilities(vec!["DEVICE_STATUS".into()]);
            service.refresh_client_info().await.unwrap_err()
        } else {
            service.unlink_device(&DcgClient::new(phone.id.clone(), Environment::Prod)).await.unwrap_err()
        };

        assert_matches!(err, AuthError::UnlinkedDevice, "{op}");
        assert_eq!(h.dcg.count(op), 1, "{op}");
        assert_eq!(h.dcg.count("GetDeviceInfoList"), 1, "{op}: no refresh after the unlink");
        assert!(service.account_info().is_none(), "{op}: every account deprovisioned");
        assert_eq!(h.store_json("auth/account-info"), serde_json::json!({}), "{op}");
        assert!(service.own_client().is_none(), "{op}: identity cleared");
        assert!(service.trust_relationships().is_empty(), "{op}");
        assert!(service.peers().is_empty(), "{op}");
        for key in ["auth/prod/identity", "auth/prod/identity-pair", "auth/prod/trust-pair"] {
            assert!(h.store_bytes(key).is_none(), "{op}: {key} cleared");
        }
    }
}

/// Signs in through the web view as its current account and enrolls with one linked phone.
async fn enrolled_interactively(h: &Harness, phone_name: &str) -> (Arc<AuthService>, Provisioned, Phone) {
    let service = h.signed_in_interactively().await;
    let p = service.ensure_provisioned().await.unwrap();
    let phone = h.phone(phone_name);
    h.dcg.add_peer(phone.item.clone());
    service.sync_trust(FetchMode::ForceCloud).await.unwrap();
    (service, p, phone)
}

/// The fake DCG now serves another Microsoft account: its own account certificate and devices.
fn serve_another_account(h: &Harness, phone_name: &str) -> Phone {
    let account = new_guid_lower();
    let cert = KeyPair::generate(&account, h.clock.now()).unwrap().cert.der;
    h.dcg.with(|s| {
        s.account_id = account;
        s.account_cert = cert;
        s.peers.clear();
        s.enrolled.clear();
    });

    let phone = h.phone(phone_name);
    h.dcg.add_peer(phone.item.clone());
    phone
}

/// Account A's session is gone and the user signs in again — as account B. The sign-in page names B
/// (its PUID), so the account switches at once: step 1 deprovisions A's records (relationships,
/// trust certificate, peer cache), A's identity is cleared with every relationship of it — the broker
/// keeps only B's session, so A's identity must not be signed in again — and a new local account
/// with fresh ids is minted for B; the next provisioning creates B's own identity. A's phone is
/// neither a peer nor trusted any more, B's is.
#[tokio::test]
async fn an_interactive_sign_in_with_another_account_switches_accounts() {
    let h = Harness::new().await;
    let (service, p, phone_a) = enrolled_interactively(&h, "Pixel of A").await;
    let first = service.local_account().unwrap();
    assert_eq!(first.puid.as_deref(), Some(SEED_PUID));
    drop(service);

    // A pairing-era relationship (no CID) outlives the deprovisioning of A's records, but not the
    // switch, which clears A's identity with its relationships.
    let legacy_phone = h.phone("legacy");
    let mut legacy = new_relationship(
        &format!("trust_{}", p.own.id),
        "legacy-pairing-partner",
        "",
        CryptoTrustType::ClassicPairingD2D,
        &legacy_phone.pair.cert,
        Environment::Prod,
        &legacy_phone.id,
        h.clock.now(),
        &TrustPolicy::from_config(&h.config.auth),
    )
    .unwrap();
    legacy.msa_cid = None;
    let mut rels = h.store_json("auth/trust-relationships");
    rels.as_array_mut().unwrap().push(serde_json::to_value(&legacy).unwrap());
    h.put_store_json("auth/trust-relationships", &rels);

    h.store.delete(USER_KEY.into()).unwrap();
    h.web.sign_in_as(ACCOUNT_B);
    let phone_b = serve_another_account(&h, "Pixel of B");
    h.dcg.take_requests();
    let restarted = h.service();
    let mut removed = restarted.subscribe_account_removed();
    restarted.sign_in().await.unwrap();

    // The switch.
    assert_eq!(removed.try_recv().unwrap().account_id, first.account_id, "A was deprovisioned");
    let second = restarted.local_account().unwrap();
    assert_eq!(second.puid.as_deref(), Some(ACCOUNT_B));
    assert_ne!(second.account_id, first.account_id, "B does not inherit A's account id");
    assert_ne!(second.cid, first.cid, "nor its CID");
    assert!(restarted.account_info().is_none());
    assert_eq!(h.store_json("auth/account-info"), serde_json::json!({}));
    assert!(restarted.peers().is_empty(), "A's phone is no peer of B");
    assert!(!restarted.is_trusted(&DcgClient::new(phone_a.id.clone(), Environment::Prod)), "nor trusted");

    let rels = restarted.trust_relationships();
    assert!(rels.is_empty(), "A's A2D and AsyncD2D, and the CID-less record of A's identity: {rels:#?}");
    for key in ["auth/prod/identity", "auth/prod/identity-pair", "auth/prod/trust-pair"] {
        assert!(h.store_bytes(key).is_none(), "{key}: A's identity and trust certificate are cleared");
    }
    assert_eq!(h.session_puid().as_deref(), Some(ACCOUNT_B), "B's session");

    // Before B is provisioned no services token exists: A's identity is never signed in with B's
    // MSA token.
    assert_eq!(restarted.services_token(Environment::Prod, false).await, Err(TokenError::NotProvisioned));
    assert!(h.dcg.requests().is_empty(), "the switch sends nothing to DCG");

    // Steps 2–6 for B.
    let p2 = restarted.ensure_provisioned().await.unwrap();
    assert_eq!(p2.account_id, second.account_id);
    assert_ne!(p2.own.id, p.own.id, "A's identity is not reused");

    let reqs = h.dcg.requests();
    let ops: Vec<&str> = reqs.iter().map(|r| r.op()).collect();
    assert_eq!(ops, ["UserServiceEnvironment", "GenerateNonce", "CreateIdentity", "EnrollDevice"]);
    assert!(reqs.iter().all(|r| r.header("Authorization") == Some("Bearer msa-at-2")), "B's MSA token");
    assert_eq!(h.store_bytes("auth/prod/account-id"), Some(second.account_id.clone().into_bytes()));

    let rels = restarted.trust_relationships();
    assert!(rels.iter().all(|r| r.self_client_id == format!("trust_{}", p2.own.id)), "{rels:#?}");
    assert!(rels.iter().all(|r| r.msa_cid.as_deref() == Some(second.cid.as_str())));
    assert_eq!(restarted.account_info().unwrap().state, AccountState::Enrolled);

    let peers = restarted.sync_trust(FetchMode::Default).await.unwrap();
    assert_eq!(peers.iter().map(|p| p.client.id.as_str()).collect::<Vec<_>>(), [phone_b.id.as_str()]);
    assert!(!restarted.is_trusted(&DcgClient::new(phone_a.id.clone(), Environment::Prod)));
}

/// The same Microsoft account signing in again keeps everything: the local account, the enrolled
/// identity, its trust and its phone; only the stored session changes.
#[tokio::test]
async fn an_interactive_sign_in_with_the_same_account_keeps_it() {
    let h = Harness::new().await;
    let (service, p, phone) = enrolled_interactively(&h, "Pixel").await;
    let first = service.local_account().unwrap();
    let rels = service.trust_relationships();
    drop(service);

    h.store.delete(USER_KEY.into()).unwrap();
    h.dcg.take_requests();
    let restarted = h.service();
    let mut removed = restarted.subscribe_account_removed();
    restarted.sign_in().await.unwrap();

    assert_eq!(h.web.kinds(), [WebSignInKind::Login, WebSignInKind::Login]);
    assert_eq!(restarted.local_account().unwrap(), first);
    assert!(removed.try_recv().is_err(), "nothing was deprovisioned");
    assert_eq!(h.session_puid().as_deref(), Some(SEED_PUID));
    assert_eq!(restarted.ensure_provisioned().await.unwrap(), p, "the same identity, still Enrolled");
    assert!(h.dcg.requests().is_empty(), "no provisioning call: {:?}", h.dcg.lines());
    assert_eq!(restarted.trust_relationships(), rels);
    assert_eq!(restarted.peers().iter().map(|p| p.client.id.as_str()).collect::<Vec<_>>(), [phone.id.as_str()]);
}

/// A local account a store from before the device-bound sign-in recorded (an ID token's `sub`, no
/// PUID) matches no sign-in, so the first device-bound sign-in replaces it — silent or not.
#[tokio::test]
async fn a_local_account_without_a_puid_is_replaced_by_the_next_sign_in() {
    let h = Harness::new().await;
    let (service, p, _, _) = h.with_phone().await;
    let first = service.local_account().unwrap();
    drop(service);

    let mut account = h.store_json("auth/account");
    let fields = account.as_object_mut().unwrap();
    fields.remove("puid").expect("the device-bound sign-in recorded a PUID");
    fields.insert("subject".into(), serde_json::json!("AAAAAAAAAAAAAAAAAAAAAFakeSubjectAccountA"));
    h.put_store_json("auth/account", &account);

    let restarted = h.service();
    assert_eq!(restarted.local_account().unwrap().puid, None);
    let mut removed = restarted.subscribe_account_removed();
    restarted.sign_in().await.unwrap();

    assert!(h.web.kinds().is_empty(), "the stored session signs in silently");
    let second = restarted.local_account().unwrap();
    assert_eq!(second.puid.as_deref(), Some(SEED_PUID));
    assert_ne!(second.account_id, first.account_id);
    assert_eq!(removed.try_recv().unwrap().account_id, first.account_id);

    let p2 = restarted.ensure_provisioned().await.unwrap();
    assert_ne!(p2.own.id, p.own.id);
    assert_eq!(restarted.account_info().unwrap().state, AccountState::Enrolled);
}

#[tokio::test]
async fn the_environment_snap_counts_an_async_d2d_whose_a2d_expired() {
    let h = Harness::new().await;
    h.dcg.with(|s| s.service_environment = Some("Beta".into()));

    // An account certificate that expires in one hour.
    let short = KeyPair::generate(&h.dcg.account_id(), h.clock.now() - Duration::from_secs(365 * DAY - 3600)).unwrap();
    h.dcg.with(|s| s.account_cert = short.cert.der.clone());

    let (service, p, _, _) = h.with_phone().await;
    assert_eq!(p.env, Environment::Beta);
    drop(service);

    // Two hours later the A2D has expired, the phone's AsyncD2D has not. Provisioned (not Enrolled)
    // with discovery failing → Defaulted(Prod): the active identity and the unexpired relationship
    // keep Beta.
    h.advance(Duration::from_secs(2 * 3600));
    set_account_state(&h, "Provisioned");
    let fresh = KeyPair::generate(&h.dcg.account_id(), h.clock.now()).unwrap();
    h.dcg.with(|s| s.account_cert = fresh.cert.der.clone());
    h.dcg.script("UserServiceEnvironment", 500, "");

    let restarted = h.service();
    assert_eq!(restarted.ensure_provisioned().await.unwrap(), p, "Beta and the same identity");
    assert_eq!(h.store_bytes("auth/environment"), Some(b"Beta".to_vec()));
}

#[tokio::test]
async fn trust_certificate_rotates_before_expiry_and_persists_only_on_success() {
    let h = Harness::new().await;
    let (service, _) = h.provisioned().await;
    let first = h.trust_cert(Environment::Prod);

    // Keep the identity active (a SignIn every 100 d) until notAfter − 182 d has passed.
    h.advance(Duration::from_secs(100 * DAY));
    service.services_token_for(Environment::Prod, true).await.unwrap();
    h.advance(Duration::from_secs(84 * DAY));
    service.services_token_for(Environment::Prod, true).await.unwrap();

    // 184 d after issue: due. A failed SetDeviceInfo keeps the old certificate.
    service.set_capabilities(vec!["DEVICE_STATUS".into(), "UNLINKABLE_V1".into(), "CLIPBOARD".into()]);
    h.dcg.script("SetDeviceInfo", 500, "");
    service.refresh_client_info().await.unwrap();
    let rejected = jwt::unb64(h.dcg.of("SetDeviceInfo")[0].json()["certificates"]["SelfSigned"][0].as_str().unwrap());
    assert_ne!(rejected, first, "a new trust certificate was offered");
    assert_eq!(h.trust_cert(Environment::Prod), first, "not persisted before the server accepted it");

    // The next attempt succeeds and persists the (again new) certificate.
    service.refresh_client_info().await.unwrap();
    let accepted = jwt::unb64(h.dcg.of("SetDeviceInfo")[1].json()["certificates"]["SelfSigned"][0].as_str().unwrap());
    assert_ne!(accepted, first);
    assert_eq!(h.trust_cert(Environment::Prod), accepted);
    assert_eq!(jwt::cert_cn(&accepted), jwt::cert_cn(&first), "same subject CN=trust_<dcgClientId>");

    // Relationships keep their SelfClientId.
    let own = service.own_client().unwrap();
    assert!(service.trust_relationships().iter().all(|r| r.self_client_id == format!("trust_{}", own.id)));
}

#[tokio::test]
async fn concurrent_token_requests_share_one_sign_in() {
    let h = Harness::new().await;
    let (service, _) = h.provisioned().await;
    h.advance(Duration::from_secs(25 * 3600));
    h.dcg.take_requests();

    let (a, b, c) = tokio::join!(
        service.services_token(Environment::Prod, false),
        service.services_token(Environment::Prod, false),
        service.services_token(Environment::Prod, false),
    );

    let (a, b, c) = (a.unwrap(), b.unwrap(), c.unwrap());
    assert!(a == b && b == c);
    assert_eq!(h.dcg.count("SignIn"), 1);
}

#[tokio::test]
async fn a_malformed_base_url_fails_loudly() {
    let mut h = Harness::new().await;
    h.config.endpoints.discovery_base = h.config.endpoints.discovery_base.trim_end_matches('/').to_string();
    let service = h.signed_in().await;
    let err = service.ensure_provisioned().await.unwrap_err();
    assert!(matches!(&err, AuthError::Config(m) if m.contains("must end in '/'")), "{err:?}");
    assert!(h.dcg.requests().is_empty());
}

/// `DeleteDeviceInfo` removes this identity's own record from the account: a `DELETE` with an empty
/// `device-id` and the identity's tokens.
#[tokio::test]
async fn delete_own_device_info_removes_the_own_record() {
    let h = Harness::new().await;
    let (service, p, _) = enrolled_interactively(&h, "Pixel").await;
    let own = p.own.id.to_ascii_lowercase();
    assert!(h.dcg.with(|s| s.enrolled.contains_key(&own)), "enrolled");

    service.delete_own_device_info().await.expect("deleted");
    assert!(!h.dcg.with(|s| s.enrolled.contains_key(&own)), "the own record is gone");

    let call = h.dcg.requests().into_iter().find(|c| c.path_and_query.contains("DeleteDeviceInfo")).expect("sent");
    assert_eq!(call.method, "DELETE");
    assert_eq!(call.query("device-id").as_deref(), Some(""));
}

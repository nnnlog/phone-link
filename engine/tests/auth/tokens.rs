//! The services token refresh policy and identity key rotation, and the `TokenSource` seam the
//! relay consumes.

use crate::support::auth_world;

use std::sync::{Arc, Mutex};
use std::time::{Duration, UNIX_EPOCH};

use auth_world::fake_dcg::error_body;
use auth_world::fake_msa::Reply;
use auth_world::harness::{Harness, pair_entry_in, snapshot};
use auth_world::jwt;
use phonelink_engine::auth::AuthError;
use phonelink_engine::auth::crypto::KeyPair;
use phonelink_engine::auth::dcg_rest::{RestError, ServiceFailure, ServiceRequestStatus, TransportCause};
use phonelink_engine::auth::msa::MsaError;
use phonelink_engine::constants::ScopeFamily;
use phonelink_engine::edge::{Clock, SecureStore};
use phonelink_engine::seams::{TokenError, TokenSource};
use phonelink_engine::types::{Environment, OpenStatus};

const DAY: u64 = 86_400;

fn ms(h: &Harness) -> u64 {
    h.clock.now().duration_since(UNIX_EPOCH).unwrap().as_millis() as u64
}

fn identity_record(h: &Harness) -> serde_json::Value {
    h.store_json("auth/prod/identity")
}

const PENDING: &str = "auth/prod/identity-pair-pending";

/// Records the incoming (pending) identity certificate on disk at the moment the server receives
/// each `RotateKeys`.
fn pending_at_rotate_keys(h: &Harness) -> Arc<Mutex<Vec<Option<Vec<u8>>>>> {
    let seen = Arc::new(Mutex::new(Vec::new()));
    let (store, log) = (h.store.clone(), seen.clone());
    h.dcg.on_request("RotateKeys", move |_| log.lock().unwrap().push(pair_entry_in(&store, PENDING).map(|(_, c)| c)));
    seen
}

/// The certificate each `SignIn` so far proved.
fn sign_in_certificates(h: &Harness) -> Vec<Vec<u8>> {
    h.dcg
        .of("SignIn")
        .iter()
        .map(|r| jwt::unb64(jwt::decode(r.json()["certificateJWT"].as_str().unwrap()).str("Certificate")))
        .collect()
}

#[tokio::test]
async fn services_token_is_cached_until_exp_minus_ten_minutes() {
    let h = Harness::new().await;
    let (service, _) = h.provisioned().await;
    h.dcg.take_requests();
    assert_eq!(
        service.services_token(Environment::Prod, false).await.unwrap(),
        "svc-token-1",
        "cached from CreateIdentity"
    );

    // The fake's tokens live 24 h; at exp − 10 min the token is still used…
    h.advance(Duration::from_secs(24 * 3600 - 600));
    assert_eq!(service.services_token(Environment::Prod, false).await.unwrap(), "svc-token-1");
    assert!(h.dcg.requests().is_empty());

    // …one second later it is refreshed with GenerateNonce + SignIn.
    h.advance(Duration::from_secs(1));
    assert_eq!(service.services_token(Environment::Prod, false).await.unwrap(), "svc-token-2");
    let ops: Vec<String> = h.dcg.requests().iter().map(|r| r.op().to_string()).collect();
    assert_eq!(ops, ["GenerateNonce", "SignIn"]);
    assert_eq!(identity_record(&h)["last_accessed"], ms(&h), "a SignIn rewrites LastAccessed");
    assert_eq!(service.services_token(Environment::Prod, false).await.unwrap(), "svc-token-2");
    assert_eq!(h.dcg.requests().len(), 2);
}

#[tokio::test]
async fn force_refresh_signs_in_again() {
    let h = Harness::new().await;
    let (service, _) = h.provisioned().await;
    h.dcg.take_requests();
    assert_eq!(service.services_token(Environment::Prod, true).await.unwrap(), "svc-token-2");
    assert_eq!(service.services_token(Environment::Prod, true).await.unwrap(), "svc-token-3");
    assert_eq!(h.dcg.count("SignIn"), 2);
    assert_eq!(service.services_token(Environment::Prod, false).await.unwrap(), "svc-token-3");
}

#[tokio::test]
async fn sign_in_device_id_echo_mismatch_is_a_hard_failure() {
    let h = Harness::new().await;
    let (service, p) = h.provisioned().await;

    h.dcg.with(|s| s.sign_in_device_id_override = Some("0b0b0b0b-0000-0000-0000-000000000000".into()));
    let err = service.services_token_for(Environment::Prod, true).await.unwrap_err();
    assert!(matches!(err, AuthError::Protocol(_)), "{err:?}");
    assert_eq!(err.to_token_error().open_status(), OpenStatus::NonTransientAuthFailure);
    assert_eq!(service.own_client(), Some(p.own.clone()), "the identity is not cleared");

    // The same id in another letter case is the same GUID.
    h.dcg.with(|s| s.sign_in_device_id_override = Some(p.own.id.to_ascii_uppercase()));
    assert!(service.services_token_for(Environment::Prod, true).await.is_ok());
}

#[tokio::test]
async fn invalid_nonce_on_sign_in_is_retried_three_times() {
    let h = Harness::new().await;
    let (service, _) = h.provisioned().await;
    for _ in 0..3 {
        h.dcg.script("SignIn", 400, &error_body(Some(2), "InvalidNonce"));
    }
    h.dcg.take_requests();

    assert!(service.services_token_for(Environment::Prod, true).await.is_ok());
    assert_eq!(h.dcg.count("SignIn"), 4);
    assert_eq!(h.dcg.count("GenerateNonce"), 4, "a fresh nonce for every attempt");

    for _ in 0..4 {
        h.dcg.script("SignIn", 400, &error_body(Some(2), "InvalidNonce"));
    }
    let err = service.services_token_for(Environment::Prod, true).await.unwrap_err();
    assert!(matches!(&err, AuthError::Rest(e) if e.is_invalid_nonce()), "{err:?}");
    assert_eq!(err.to_token_error().open_status(), OpenStatus::AuthHttpRequestFailure);
}

#[tokio::test]
async fn token_errors_map_onto_open_status() {
    let h = Harness::new().await;
    let (service, _) = h.provisioned().await;

    // No identity in Beta.
    let err = service.services_token(Environment::Beta, false).await.unwrap_err();
    assert_eq!(err, TokenError::NotProvisioned);
    assert_eq!(err.open_status(), OpenStatus::NonTransientAuthFailure);

    // A DCG 401 on SignIn.
    h.dcg.script("SignIn", 401, &error_body(Some(1), "InvalidAuthHeader"));
    let err = service.services_token(Environment::Prod, true).await.unwrap_err();
    assert!(matches!(err, TokenError::Unauthorized(_)), "{err:?}");
    assert_eq!(err.open_status(), OpenStatus::AuthTransientFailure);

    // An account service that cannot be reached is the auth layer's internet failure, which the
    // relay retries and opens as InternetError.
    drop(service);
    let unreachable = Harness::new().await;
    unreachable.seed_session();
    unreachable.msa.with(|s| s.network_down = true);
    let err = unreachable.service().msa().access_token(Environment::Prod).await.unwrap_err();
    assert_eq!(err, MsaError::Internet);
    let token_error = AuthError::from(err).to_token_error();
    assert_eq!(token_error, TokenError::Internet);
    assert_eq!(token_error.open_status(), OpenStatus::InternetError);

    // Any other MSA token-provider failure is not retried by the relay and is fatal for it.
    let other = AuthError::from(MsaError::Endpoint("token HTTP 502".into())).to_token_error();
    assert!(matches!(other, TokenError::Msa(_)), "{other:?}");
    assert_eq!(other.open_status(), OpenStatus::NonTransientAuthFailure);

    // With no session and no usable web sign-in, the MSA token needs the user.
    h.store.delete("auth/live/user".into()).unwrap();
    h.web.queue(Err(phonelink_engine::edge::WebSignInError::Failed { reason: "no view".into() }));
    let fresh = h.service();
    let err = fresh.services_token(Environment::Prod, false).await.unwrap_err();
    assert_eq!(err, TokenError::UserInteractionRequired);
    assert_eq!(err.open_status(), OpenStatus::MsaTokenUserInteractionRequired);
}

#[tokio::test]
async fn identity_key_rotation_by_timer() {
    let mut h = Harness::new().await;
    h.config.auth.identity_key_rotation = Duration::from_secs(2 * DAY);
    h.dcg.with(|s| s.key_valid_remaining_days = None);
    let (service, p) = h.provisioned().await;
    assert_eq!(identity_record(&h)["key_validity_remaining_days"], -1, "absent → -1 → timer-based");
    assert_eq!(identity_record(&h)["next_key_rotation"], ms(&h) + 2 * DAY * 1000);
    let old_cert = h.identity_cert(Environment::Prod);
    let pending_seen = pending_at_rotate_keys(&h);

    h.advance(Duration::from_secs(DAY));
    service.services_token_for(Environment::Prod, true).await.unwrap();
    assert_eq!(h.dcg.count("RotateKeys"), 0, "not due yet");

    h.advance(Duration::from_secs(DAY + 1));
    h.dcg.take_requests();
    service.services_token_for(Environment::Prod, true).await.unwrap();
    let ops: Vec<String> = h.dcg.requests().iter().map(|r| r.op().to_string()).collect();
    assert_eq!(ops, ["GenerateNonce", "SignIn", "GenerateNonce", "RotateKeys"], "the check runs after the SignIn");

    let rotate = &h.dcg.of("RotateKeys")[0];
    assert_eq!(rotate.path_and_query, "/Auth/RotateKeys?api-version=1.1.0");
    assert_eq!(rotate.header("UserIdentityType"), Some("MSA"));
    assert!(rotate.header("UserIdentityToken").is_some());

    let body = rotate.json();
    assert_eq!(body.as_object().unwrap().len(), 2);
    let old = jwt::decode(body["oldCertificateJWT"].as_str().unwrap());
    let new = jwt::decode(body["newCertificateJWT"].as_str().unwrap());
    assert_eq!(jwt::unb64(old.str("Certificate")), old_cert, "old JWT carries the current certificate");
    assert!(jwt::verifies(&old, &old_cert));

    let new_cert = jwt::unb64(new.str("Certificate"));
    assert_ne!(new_cert, old_cert);
    assert!(jwt::verifies(&new, &new_cert));
    assert!(!jwt::verifies(&new, &old_cert));
    assert_eq!(old.str("Nonce"), new.str("Nonce"), "the same nonce");
    assert_eq!(old.str("iss"), p.own.id);
    assert_eq!(new.str("iss"), p.own.id);
    assert_eq!(jwt::cert_cn(&new_cert), p.own.id, "same subject CN=<dcgClientId>");

    assert_eq!(h.identity_cert(Environment::Prod), new_cert, "the new certificate replaces the old");
    assert_eq!(
        *pending_seen.lock().unwrap(),
        [Some(new_cert.clone())],
        "the incoming key + certificate was on disk before RotateKeys reached the server"
    );
    assert_eq!(h.pending_identity_cert(Environment::Prod), None, "promoted: no pending key remains");

    let (key, _) = h.pair_entry("auth/prod/identity-pair").unwrap();
    let new_pair = KeyPair::from_parts(&key, &new_cert).expect("the new key is stored with its certificate");
    assert_eq!(new_pair.cn(), p.own.id);
    assert_eq!(identity_record(&h)["next_key_rotation"], ms(&h) + 2 * DAY * 1000);
    assert_eq!(h.dcg.with(|s| s.rotations), 1);

    // The next SignIn proves the new key (the fake checks the registered certificate).
    service.services_token_for(Environment::Prod, true).await.unwrap();
    let sign_in = h.dcg.of("SignIn").pop().unwrap();
    let j = jwt::decode(sign_in.json()["certificateJWT"].as_str().unwrap());
    assert_eq!(jwt::unb64(j.str("Certificate")), new_cert);

    // The trust certificate is not rotated by RotateKeys.
    assert_eq!(h.dcg.count("SetDeviceInfo"), 0);
}

#[tokio::test]
async fn identity_key_rotation_by_key_valid_remaining_days() {
    let h = Harness::new().await;
    h.dcg.with(|s| s.key_valid_remaining_days = Some(200));
    let (service, _) = h.provisioned().await;

    service.services_token_for(Environment::Prod, true).await.unwrap();
    assert_eq!(identity_record(&h)["key_validity_remaining_days"], 200);
    assert_eq!(h.dcg.count("RotateKeys"), 0, "200 > 182");

    h.dcg.with(|s| s.key_valid_remaining_days = Some(182));
    service.services_token_for(Environment::Prod, true).await.unwrap();
    assert_eq!(identity_record(&h)["key_validity_remaining_days"], 182);
    assert_eq!(h.dcg.count("RotateKeys"), 1, "≤ 182 rotates although NextKeyRotation is 182.5 d away");

    // A negative value falls back to the timer (not due).
    h.dcg.with(|s| s.key_valid_remaining_days = Some(-5));
    service.services_token_for(Environment::Prod, true).await.unwrap();
    assert_eq!(identity_record(&h)["key_validity_remaining_days"], -1);
    assert_eq!(h.dcg.count("RotateKeys"), 1);
}

/// A failed rotation keeps the current key; a rotation that finds the identity certificate rejected
/// is the identity rules' case (`auth::identity_rules`).
#[tokio::test]
async fn identity_key_rotation_failures() {
    let mut h = Harness::new().await;
    h.config.auth.identity_key_rotation = Duration::from_secs(2 * DAY);
    h.dcg.with(|s| s.key_valid_remaining_days = None);
    let (service, _) = h.provisioned().await;
    let cert = h.identity_cert(Environment::Prod);
    let pending_seen = pending_at_rotate_keys(&h);
    h.advance(Duration::from_secs(2 * DAY + 1));

    // A failed rotation does not fail the token, drops the incoming key and re-checks in 1 d.
    h.dcg.script("RotateKeys", 500, "");
    assert!(service.services_token_for(Environment::Prod, true).await.is_ok());
    assert_eq!(identity_record(&h)["next_key_rotation"], ms(&h) + DAY * 1000);
    assert_eq!(h.identity_cert(Environment::Prod), cert);
    assert!(pending_seen.lock().unwrap()[0].is_some(), "the incoming key was pending while RotateKeys ran");
    assert_eq!(h.pending_identity_cert(Environment::Prod), None, "a rejected rotation drops it");

    h.advance(Duration::from_secs(3600));
    service.services_token_for(Environment::Prod, true).await.unwrap();
    assert_eq!(h.dcg.count("RotateKeys"), 1);

    // InvalidNonce is retried with a new nonce.
    h.advance(Duration::from_secs(DAY));
    h.dcg.script("RotateKeys", 400, &error_body(Some(2), "InvalidNonce"));
    service.services_token_for(Environment::Prod, true).await.unwrap();
    assert_eq!(h.dcg.count("RotateKeys"), 3);
    assert_ne!(h.identity_cert(Environment::Prod), cert, "rotated on the retry");

    let seen = pending_seen.lock().unwrap().clone();
    assert_eq!(seen[1], seen[2], "the InvalidNonce retry offers the same incoming key");
    assert_eq!(seen[2].as_ref(), Some(&h.identity_cert(Environment::Prod)));
    assert_eq!(h.pending_identity_cert(Environment::Prod), None);
}

#[tokio::test]
async fn a_crash_after_the_server_took_rotate_keys_resumes_with_the_incoming_key() {
    let mut h = Harness::new().await;
    h.config.auth.identity_key_rotation = Duration::from_secs(2 * DAY);
    h.dcg.with(|s| s.key_valid_remaining_days = None);
    let (service, p) = h.provisioned().await;
    let old_cert = h.identity_cert(Environment::Prod);

    // What a crash at the instant the server takes the rotation would leave on disk.
    let on_disk = Arc::new(Mutex::new(None));
    let (store, slot) = (h.store.clone(), on_disk.clone());
    h.dcg.on_request("RotateKeys", move |_| *slot.lock().unwrap() = Some(snapshot(&store)));

    h.advance(Duration::from_secs(2 * DAY + 1));
    service.services_token_for(Environment::Prod, true).await.unwrap();
    assert_eq!(h.dcg.with(|s| s.rotations), 1);

    let new_cert = h.identity_cert(Environment::Prod);
    assert_ne!(new_cert, old_cert);
    drop(service);

    // The process "restarts" from that state: the old key is still the identity key, the incoming
    // one is pending, and the server holds the incoming one.
    let on_disk = on_disk.lock().unwrap().take().unwrap();
    h.restore_store(&on_disk);
    assert_eq!(h.identity_cert(Environment::Prod), old_cert);
    assert_eq!(h.pending_identity_cert(Environment::Prod), Some(new_cert.clone()));

    h.dcg.with(|s| s.key_valid_remaining_days = Some(365));
    h.dcg.take_requests();
    let restarted = h.service();
    assert_eq!(restarted.ensure_provisioned().await.unwrap(), p);
    restarted.services_token(Environment::Prod, false).await.unwrap();

    let ops: Vec<String> = h.dcg.requests().iter().map(|r| r.op().to_string()).collect();
    assert_eq!(ops, ["GenerateNonce", "SignIn", "GenerateNonce", "SignIn"]);
    assert_eq!(
        sign_in_certificates(&h),
        [old_cert, new_cert.clone()],
        "InvalidCertificate for the old key, then the incoming key signs in"
    );

    assert_eq!(restarted.own_client(), Some(p.own.clone()), "the same dcgClientId");
    assert_eq!(h.identity_cert(Environment::Prod), new_cert, "the incoming key is promoted");
    assert_eq!(h.pending_identity_cert(Environment::Prod), None);
    assert_eq!(identity_record(&h)["last_accessed"], ms(&h));

    // A later restart signs in with the promoted key at once.
    drop(restarted);
    h.dcg.take_requests();
    h.service().services_token(Environment::Prod, false).await.unwrap();
    assert_eq!(sign_in_certificates(&h), [new_cert]);
}

#[tokio::test]
async fn rotate_keys_without_a_reply_keeps_the_incoming_key_pending() {
    for applied in [true, false] {
        let mut h = Harness::new().await;
        h.config.auth.identity_key_rotation = Duration::from_secs(2 * DAY);
        h.dcg.with(|s| s.key_valid_remaining_days = None);
        let (service, p) = h.provisioned().await;
        let old_cert = h.identity_cert(Environment::Prod);
        h.advance(Duration::from_secs(2 * DAY + 1));

        // The service applies RotateKeys, but its reply is lost.
        h.dcg.lose_next_reply("RotateKeys");
        assert!(service.services_token_for(Environment::Prod, true).await.is_ok(), "the token is not failed");
        assert_eq!(h.dcg.with(|s| s.rotations), 1);
        let incoming = h.pending_identity_cert(Environment::Prod).expect("no reply: the incoming key stays pending");
        assert_eq!(h.identity_cert(Environment::Prod), old_cert);
        assert_eq!(identity_record(&h)["next_key_rotation"], ms(&h) + DAY * 1000, "re-checked in 1 d");

        if !applied {
            // As if the request never reached the service.
            h.dcg.with(|s| s.identities.insert(p.own.id.clone(), old_cert.clone()));
        }

        drop(service);
        h.dcg.with(|s| s.key_valid_remaining_days = Some(365));
        h.dcg.take_requests();
        let restarted = h.service();
        restarted.services_token(Environment::Prod, false).await.unwrap();

        assert_eq!(restarted.own_client(), Some(p.own.clone()), "applied={applied}");
        assert_eq!(h.pending_identity_cert(Environment::Prod), None, "applied={applied}");

        if applied {
            assert_eq!(sign_in_certificates(&h), [old_cert, incoming.clone()]);
            assert_eq!(h.identity_cert(Environment::Prod), incoming, "the server held it: promoted");
        } else {
            assert_eq!(sign_in_certificates(&h), std::slice::from_ref(&old_cert));
            assert_eq!(h.identity_cert(Environment::Prod), old_cert, "never taken: dropped");
        }
    }
}

/// When the service holds neither the identity key nor the incoming one, the identity is lost and
/// created anew in place.
#[tokio::test]
async fn a_pending_key_the_server_also_rejects_loses_the_identity() {
    let mut h = Harness::new().await;
    h.config.auth.identity_key_rotation = Duration::from_secs(2 * DAY);
    h.dcg.with(|s| s.key_valid_remaining_days = None);
    let (service, p) = h.provisioned().await;
    h.advance(Duration::from_secs(2 * DAY + 1));
    h.dcg.lose_next_reply("RotateKeys");
    service.services_token_for(Environment::Prod, true).await.unwrap();
    assert!(h.pending_identity_cert(Environment::Prod).is_some());

    // The service holds neither key.
    let other = KeyPair::generate(&p.own.id, h.clock.now()).unwrap();
    h.dcg.with(|s| s.identities.insert(p.own.id.clone(), other.cert.der.clone()));

    drop(service);
    let restarted = h.service();
    restarted.services_token(Environment::Prod, false).await.unwrap();
    assert_eq!(h.dcg.count("SignIn"), 3, "the old key, then the incoming key");
    assert_eq!(h.dcg.count("CreateIdentity"), 2, "then a new identity");
    let new = restarted.own_client().unwrap();
    assert_ne!(new.id, p.own.id);
    assert_eq!(h.pending_identity_cert(Environment::Prod), None, "the lost identity's incoming key is gone");
}

#[tokio::test]
async fn msa_token_provider_failures_are_token_error_msa() {
    for e in [
        MsaError::Endpoint("HTTP 500".into()),
        MsaError::InvalidResponse("no access_token".into()),
        MsaError::Store("keychain".into()),
        MsaError::Declined,
        MsaError::UnsupportedScopeFamily(ScopeFamily::Rps),
    ] {
        let err = AuthError::from(e.clone()).to_token_error();
        assert!(matches!(err, TokenError::Msa(_)), "{e:?} → {err:?}");
    }

    // An unreachable token endpoint is the auth layer's internet failure, retried by the relay.
    assert_eq!(AuthError::from(MsaError::Internet).to_token_error(), TokenError::Internet);
    assert_eq!(
        AuthError::from(MsaError::InteractionRequired("login_required".into())).to_token_error(),
        TokenError::UserInteractionRequired
    );

    // DCG REST failures keep their mapping.
    let offline = RestError::Transport { call: "SignIn", cause: TransportCause::Internet, detail: "down".into() };
    assert_eq!(AuthError::from_rest(offline).to_token_error(), TokenError::Internet);
    let status = |http: u16, status: ServiceRequestStatus| {
        RestError::Status(Box::new(ServiceFailure {
            call: "SignIn",
            http_status: http,
            status,
            error: None,
            body: String::new(),
        }))
    };

    assert!(matches!(
        AuthError::from_rest(status(401, ServiceRequestStatus::UnauthorizedRequest)).to_token_error(),
        TokenError::Unauthorized(_)
    ));
    assert!(matches!(
        AuthError::from_rest(status(503, ServiceRequestStatus::ServiceError)).to_token_error(),
        TokenError::Http(_)
    ));

    // Through the TokenSource seam: the MSA token of a provisioned identity cannot be obtained.
    // An account service that answers with an unusable (non-token) body is an MSA token-provider
    // failure …
    let h = Harness::new().await;
    let (service, _) = h.provisioned().await;
    drop(service);

    h.msa.script(Reply::Body("upstream down".into()));
    let restarted = h.service();
    h.dcg.take_requests();
    let err = restarted.services_token(Environment::Prod, true).await.unwrap_err();
    assert!(matches!(&err, TokenError::Msa(_)), "a token-provider failure: {err:?}");

    // … one that cannot be reached is the internet failure, online or not.
    h.msa.with(|s| s.network_down = true);
    let err = restarted.services_token(Environment::Prod, true).await.unwrap_err();
    assert_eq!(err, TokenError::Internet, "online, a refused connection");

    h.msa.with(|s| s.network_down = false);
    h.reachability.set(false);
    let err = restarted.services_token(Environment::Prod, true).await.unwrap_err();
    assert_eq!(err, TokenError::Internet, "offline");
    assert!(h.dcg.requests().is_empty(), "no DCG call without an MSA token");
}

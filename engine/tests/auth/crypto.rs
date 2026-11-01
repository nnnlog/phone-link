//! Certificates, keys and JWS (the crypto parameters, the nonce JWT `certificateJWT`, inbound JWT
//! validation), checked with `x509-cert` / `p384` directly rather than through the client's own
//! parser.

use crate::support::auth_world;

use std::time::{Duration, SystemTime, UNIX_EPOCH};

use auth_world::jwt;
use p384::ecdsa::signature::Verifier;
use p384::ecdsa::{DerSignature, SigningKey};
use phonelink_engine::auth::crypto::{self, KeyPair};
use phonelink_engine::auth::trust::{
    CryptoTrustType, InboundJwtError, TrustPolicy, TrustStore, new_relationship, trust_client_id,
};
use phonelink_engine::config::AuthConfig;
use phonelink_engine::types::Environment;
use x509_cert::Certificate;
use x509_cert::certificate::Version;
use x509_cert::der::{Decode, Encode};
use x509_cert::ext::pkix::{BasicConstraints, KeyUsage, KeyUsages};

fn t0() -> SystemTime {
    UNIX_EPOCH + Duration::from_secs(1_792_454_400)
}

fn secs(t: SystemTime) -> i64 {
    t.duration_since(UNIX_EPOCH).unwrap().as_secs() as i64
}

#[test]
fn self_signed_certificate_fields() {
    let key: SigningKey = crypto::generate_signing_key();
    let cn = "0f2a63f2-7a43-4a65-9d4c-8f7b6d2e1a10";
    let der = crypto::self_signed_certificate(&key, cn, t0()).unwrap();
    let cert = Certificate::from_der(&der).unwrap();
    let tbs = cert.tbs_certificate();

    assert_eq!(tbs.version(), Version::V3);
    assert_eq!(tbs.subject().to_string(), format!("CN={cn}"));
    assert_eq!(tbs.issuer().to_string(), format!("CN={cn}"), "self-signed: subject = issuer");
    assert_eq!(
        tbs.validity().not_before.to_system_time(),
        t0() - Duration::from_secs(12 * 3600),
        "notBefore = now − 12 h"
    );
    assert_eq!(
        tbs.validity().not_after.to_system_time(),
        t0() + Duration::from_secs(365 * 86_400),
        "notAfter = now + 365 d"
    );

    // ecdsa-with-SHA384 in both places.
    assert_eq!(cert.signature_algorithm().oid.to_string(), "1.2.840.10045.4.3.3");
    assert_eq!(tbs.signature().oid.to_string(), "1.2.840.10045.4.3.3");

    // P-384 public key.
    let spki = tbs.subject_public_key_info();
    assert_eq!(spki.algorithm.oid.to_string(), "1.2.840.10045.2.1");
    let curve =
        spki.algorithm.parameters.as_ref().unwrap().decode_as::<x509_cert::der::asn1::ObjectIdentifier>().unwrap();
    assert_eq!(curve.to_string(), "1.3.132.0.34");

    // Positive serial.
    let serial = tbs.serial_number().as_bytes();
    assert!(!serial.is_empty() && serial[0] & 0x80 == 0 && serial[0] != 0, "positive serial {serial:02x?}");

    // Exactly the two extensions, both non-critical.
    let exts = tbs.extensions().expect("extensions");
    assert_eq!(exts.len(), 2);

    let bc = exts.iter().find(|e| e.extn_id.to_string() == "2.5.29.19").expect("BasicConstraints");
    assert!(!bc.critical, "BasicConstraints non-critical");
    let bc_value = BasicConstraints::from_der(bc.extn_value.as_bytes()).unwrap();
    assert!(!bc_value.ca);
    assert_eq!(bc_value.path_len_constraint, None);

    let ku = exts.iter().find(|e| e.extn_id.to_string() == "2.5.29.15").expect("KeyUsage");
    assert!(!ku.critical, "KeyUsage non-critical");
    let ku_value = KeyUsage::from_der(ku.extn_value.as_bytes()).unwrap();
    assert!(ku_value.digital_signature());
    assert_eq!(ku_value, KeyUsage(KeyUsages::DigitalSignature.into()), "digitalSignature only");
    assert_eq!(ku_value.0.bits(), 1);

    // The signature verifies with the certificate's own key over the TBS DER.
    let tbs_der = tbs.to_der().unwrap();
    let sig = DerSignature::from_bytes(cert.signature().raw_bytes()).unwrap();
    key.verifying_key().verify(&tbs_der, &sig).unwrap();
    assert_eq!(jwt::cert_key(&der), *key.verifying_key());
}

#[test]
fn trust_certificate_subject_and_parse_back() {
    let pair = KeyPair::generate("trust_0f2a63f2-7a43-4a65-9d4c-8f7b6d2e1a10", t0()).unwrap();
    assert_eq!(pair.cn(), "trust_0f2a63f2-7a43-4a65-9d4c-8f7b6d2e1a10");
    assert_eq!(pair.cert.issuer_cn.as_deref(), Some(pair.cn()));
    assert_eq!(jwt::cert_cn(&pair.cert.der), pair.cn());
    let parsed = crypto::parse_certificate(&pair.cert.der).unwrap();
    assert_eq!(parsed.not_after, t0() + Duration::from_secs(365 * 86_400));
}

#[test]
fn pkcs8_round_trip_and_mismatch() {
    let pair = KeyPair::generate("a", t0()).unwrap();
    let pkcs8 = pair.pkcs8().unwrap();
    let back = KeyPair::from_parts(&pkcs8, &pair.cert.der).unwrap();
    assert_eq!(back.key.verifying_key(), pair.key.verifying_key());
    let other = KeyPair::generate("a", t0()).unwrap();
    assert!(KeyPair::from_parts(&pkcs8, &other.cert.der).is_err(), "a certificate of another key is rejected");
}

#[test]
fn nonce_jwt_shape_and_signature() {
    let cn = "6a4bd2a6-0b2a-4a0e-9a8f-0c9d0e1f2a3b";
    let pair = KeyPair::generate(cn, t0()).unwrap();
    let token = crypto::nonce_jwt(&pair, "server-nonce-1", t0()).unwrap();

    assert!(!token.contains('='), "base64url without padding");
    let j = jwt::decode(&token);
    assert_eq!(j.header, r#"{"alg":"ES384","typ":"JWT"}"#);
    let now = secs(t0());
    assert_eq!(
        j.claims_text,
        format!(
            r#"{{"iss":"{cn}","Nonce":"server-nonce-1","Certificate":"{}","nbf":{},"exp":{}}}"#,
            jwt::b64(&pair.cert.der),
            now - 43_200,
            now + 43_200
        ),
        "exactly iss, Nonce, Certificate, nbf, exp"
    );
    assert_eq!(j.claim_order(), ["iss", "Nonce", "Certificate", "nbf", "exp"]);
    assert_eq!(jwt::unb64(j.str("Certificate")), pair.cert.der, "standard base64 of the DER");
    assert_eq!(j.signature.len(), 96, "raw r ‖ s");
    assert!(jwt::verifies(&j, &pair.cert.der));

    let other = KeyPair::generate(cn, t0()).unwrap();
    assert!(!jwt::verifies(&j, &other.cert.der));
}

#[test]
fn data_jwt_claims() {
    let pair = KeyPair::generate("trust_me", t0()).unwrap();
    let token = crypto::data_jwt(&pair.key, "trust_me", r#"{"a":"b"}"#, "me", "wake", t0()).unwrap();

    let j = jwt::decode(&token);
    assert_eq!(j.header, r#"{"alg":"ES384","typ":"JWT"}"#);
    assert_eq!(j.claim_order(), ["iss", "Data", "SourceId", "Scope", "nbf", "exp"]);
    assert_eq!(j.str("Data"), r#"{"a":"b"}"#);
    assert_eq!(j.str("SourceId"), "me");
    assert_eq!(j.str("Scope"), "wake");
    assert_eq!(j.i64("exp") - j.i64("nbf"), 86_400);
    assert!(jwt::verifies(&j, &pair.cert.der));
}

fn policy() -> TrustPolicy {
    TrustPolicy::from_config(&AuthConfig::default())
}

/// A store with one `AsyncD2D` relationship to `phone` (`trust_phone`), self `trust_pc`.
fn store_with(phone: &KeyPair) -> TrustStore {
    let mut store = TrustStore::default();
    let record = new_relationship(
        "trust_pc",
        &trust_client_id("phone"),
        "cid",
        CryptoTrustType::AsyncD2D,
        &phone.cert,
        Environment::Prod,
        "phone",
        t0(),
        &policy(),
    )
    .unwrap();
    store.add(record, t0(), &policy()).unwrap();
    store
}

fn phone_jwt(key: &SigningKey, claims: serde_json::Value) -> String {
    crypto::sign_jws(key, &claims).unwrap()
}

#[test]
fn inbound_jwt_validation_against_a_relationship() {
    let p = policy();
    let phone = KeyPair::generate("trust_phone", t0()).unwrap();
    let mut store = store_with(&phone);
    let now = secs(t0());
    let good = phone_jwt(
        &phone.key,
        serde_json::json!({"iss": "trust_phone", "Data": "{\"x\":1}", "SourceId": "phone", "Scope": "wake", "nbf": now - 60, "exp": now + 60}),
    );

    let ok = store.validate_inbound_jwt(&good, Some("wake"), t0(), &p).unwrap();
    assert_eq!(ok.data.as_deref(), Some("{\"x\":1}"));
    assert_eq!(ok.signer.len(), 1);
    assert_eq!(ok.signer[0].id, "phone");
    assert_eq!(ok.signer[0].env, Environment::Prod);

    // Scope mismatch, unknown issuer, wrong key.
    assert!(matches!(
        store.validate_inbound_jwt(&good, Some("hotspot.request"), t0(), &p),
        Err(InboundJwtError::WrongScope { .. })
    ));

    let stranger = KeyPair::generate("trust_stranger", t0()).unwrap();
    let unknown = phone_jwt(&stranger.key, serde_json::json!({"iss": "trust_stranger", "exp": now + 60}));
    assert!(matches!(store.validate_inbound_jwt(&unknown, None, t0(), &p), Err(InboundJwtError::Untrusted(_))));

    let forged = phone_jwt(&stranger.key, serde_json::json!({"iss": "trust_phone", "exp": now + 60}));
    assert!(matches!(store.validate_inbound_jwt(&forged, None, t0(), &p), Err(InboundJwtError::Invalid(_))));

    // Lifetime with the 12 h skew.
    let stale = phone_jwt(&phone.key, serde_json::json!({"iss": "trust_phone", "exp": now - 11 * 3600}));
    assert!(store.validate_inbound_jwt(&stale, None, t0(), &p).is_ok(), "within the 12 h skew");

    let expired = phone_jwt(&phone.key, serde_json::json!({"iss": "trust_phone", "exp": now - 13 * 3600}));
    assert!(store.validate_inbound_jwt(&expired, None, t0(), &p).is_err());

    let future = phone_jwt(
        &phone.key,
        serde_json::json!({"iss": "trust_phone", "nbf": now + 13 * 3600, "exp": now + 14 * 3600}),
    );
    assert!(store.validate_inbound_jwt(&future, None, t0(), &p).is_err());

    let no_exp = phone_jwt(&phone.key, serde_json::json!({"iss": "trust_phone"}));
    assert!(store.validate_inbound_jwt(&no_exp, None, t0(), &p).is_err());

    // Audience: optional, but when present it must be the record's SelfClientId.
    let aud_ok = phone_jwt(&phone.key, serde_json::json!({"iss": "trust_phone", "aud": "trust_pc", "exp": now + 60}));
    assert!(store.validate_inbound_jwt(&aud_ok, None, t0(), &p).is_ok());

    let aud_bad =
        phone_jwt(&phone.key, serde_json::json!({"iss": "trust_phone", "aud": "trust_other", "exp": now + 60}));
    assert!(store.validate_inbound_jwt(&aud_bad, None, t0(), &p).is_err());
}

#[test]
fn inbound_jwt_temporary_certificate_window() {
    let p = policy();
    let old = KeyPair::generate("trust_phone", t0()).unwrap();
    let new = KeyPair::generate("trust_phone", t0()).unwrap();
    let mut store = store_with(&old);

    let index = store.index_by_partner_client_id("TRUST_PHONE").expect("case-insensitive lookup");
    assert!(store.rotate_partner_certificate(index, &new.cert, t0(), &p).unwrap());

    let r = store.get(index);
    assert_eq!(r.partner_certificate, new.cert.der);
    assert_eq!(r.partner_temporary_certificate.as_deref(), Some(old.cert.der.as_slice()));
    assert_eq!(r.partner_temporary_key_expiration_time, Some(t0() + Duration::from_secs(600)));

    let at = |offset: u64| t0() + Duration::from_secs(offset);
    let signed_by_old =
        |when: SystemTime| phone_jwt(&old.key, serde_json::json!({"iss": "trust_phone", "exp": secs(when) + 60}));

    assert!(store.validate_inbound_jwt(&signed_by_old(at(300)), None, at(300), &p).is_ok(), "old key within 10 min");
    assert!(store.validate_inbound_jwt(&signed_by_old(at(601)), None, at(601), &p).is_err(), "old key after 10 min");
    assert!(store.get(index).partner_temporary_certificate.is_none(), "the temporary certificate was dropped");

    let signed_by_new = phone_jwt(&new.key, serde_json::json!({"iss": "trust_phone", "exp": secs(at(601)) + 60}));
    assert!(store.validate_inbound_jwt(&signed_by_new, None, at(601), &p).is_ok());

    // Rotating to the same certificate changes nothing; an expired certificate is rejected.
    assert!(!store.rotate_partner_certificate(index, &new.cert, at(700), &p).unwrap());

    let expired = KeyPair::generate("trust_phone", t0() - Duration::from_secs(400 * 86_400)).unwrap();
    assert!(store.rotate_partner_certificate(index, &expired.cert, at(700), &p).is_err());
}

#[test]
fn partner_key_expiration_is_capped() {
    let p = policy();
    // A certificate valid for 365 d from 12 h ago, received 30 d later: notAfter wins.
    let phone = KeyPair::generate("trust_phone", t0()).unwrap();
    let later = t0() + Duration::from_secs(30 * 86_400);
    let record = new_relationship(
        "trust_pc",
        "trust_phone",
        "cid",
        CryptoTrustType::AsyncD2D,
        &phone.cert,
        Environment::Prod,
        "phone",
        later,
        &p,
    )
    .unwrap();
    assert_eq!(record.partner_key_expiration_time, phone.cert.not_after);

    // Received at once: now + 365 d is earlier than notAfter? No: notAfter = t0 + 365 d = cap.
    let record = new_relationship(
        "trust_pc",
        "trust_phone",
        "cid",
        CryptoTrustType::AsyncD2D,
        &phone.cert,
        Environment::Prod,
        "phone",
        t0(),
        &p,
    )
    .unwrap();
    assert_eq!(record.partner_key_expiration_time, phone.cert.not_after.min(t0() + Duration::from_secs(365 * 86_400)));

    // A certificate valid far into the future is capped at now + 365 d.
    let long = KeyPair::generate("trust_phone", t0() + Duration::from_secs(200 * 86_400)).unwrap();
    let record = new_relationship(
        "trust_pc",
        "trust_phone",
        "cid",
        CryptoTrustType::AsyncD2D,
        &long.cert,
        Environment::Prod,
        "phone",
        t0(),
        &p,
    )
    .unwrap();
    assert_eq!(record.partner_key_expiration_time, t0() + Duration::from_secs(365 * 86_400));
}

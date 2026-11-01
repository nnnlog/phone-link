//! An independent reading of the client's JWS and certificates — `base64`, `p384` and `x509-cert`
//! used directly, not through `phonelink_engine::auth::crypto` — so tests do not grade the client
//! with its own code.

use base64::Engine as _;
use p384::ecdsa::signature::Verifier;
use p384::ecdsa::{Signature, VerifyingKey};
use p384::pkcs8::DecodePublicKey;
use x509_cert::Certificate;
use x509_cert::der::{Decode, Encode};

pub fn b64(bytes: &[u8]) -> String {
    base64::prelude::BASE64_STANDARD.encode(bytes)
}

pub fn unb64(text: &str) -> Vec<u8> {
    base64::prelude::BASE64_STANDARD.decode(text).expect("standard base64")
}

pub fn unb64url(text: &str) -> Vec<u8> {
    assert!(!text.contains('='), "base64url without padding: {text}");
    base64::prelude::BASE64_URL_SAFE_NO_PAD.decode(text).expect("base64url")
}

/// A compact JWS split into its parts.
#[derive(Debug, Clone)]
pub struct Jwt {
    pub header: String,
    pub claims_text: String,
    pub claims: serde_json::Value,
    pub signing_input: String,
    pub signature: Vec<u8>,
}

impl Jwt {
    pub fn str(&self, name: &str) -> &str {
        self.claims[name].as_str().unwrap_or_else(|| panic!("claim {name} is not a string: {}", self.claims_text))
    }

    pub fn i64(&self, name: &str) -> i64 {
        self.claims[name].as_i64().unwrap_or_else(|| panic!("claim {name} is not a number: {}", self.claims_text))
    }

    /// Claim names in the order they appear in the claims text.
    pub fn claim_order(&self) -> Vec<String> {
        let mut names: Vec<(usize, String)> = self
            .claims
            .as_object()
            .unwrap()
            .keys()
            .map(|k| (self.claims_text.find(&format!("\"{k}\":")).unwrap(), k.clone()))
            .collect();
        names.sort();
        names.into_iter().map(|(_, k)| k).collect()
    }
}

pub fn decode(token: &str) -> Jwt {
    let parts: Vec<&str> = token.split('.').collect();
    assert_eq!(parts.len(), 3, "compact JWS: {token}");

    let header = String::from_utf8(unb64url(parts[0])).unwrap();
    let claims_text = String::from_utf8(unb64url(parts[1])).unwrap();
    let claims = serde_json::from_str(&claims_text).unwrap();

    Jwt {
        header,
        claims_text,
        claims,
        signing_input: format!("{}.{}", parts[0], parts[1]),
        signature: unb64url(parts[2]),
    }
}

/// The P-384 public key of a DER certificate.
pub fn cert_key(cert_der: &[u8]) -> VerifyingKey {
    let cert = Certificate::from_der(cert_der).expect("DER certificate");
    let spki = cert.tbs_certificate().subject_public_key_info().to_der().unwrap();
    VerifyingKey::from_public_key_der(&spki).expect("P-384 key")
}

/// The ES384 raw `r ‖ s` signature verifies with the certificate's key.
pub fn verifies(jwt: &Jwt, cert_der: &[u8]) -> bool {
    if jwt.signature.len() != 96 {
        return false;
    }
    let Ok(sig) = Signature::from_slice(&jwt.signature) else { return false };
    cert_key(cert_der).verify(jwt.signing_input.as_bytes(), &sig).is_ok()
}

/// The subject CN of a DER certificate.
pub fn cert_cn(cert_der: &[u8]) -> String {
    let cert = Certificate::from_der(cert_der).expect("DER certificate");
    cert.tbs_certificate().subject().common_name().unwrap().map(|cn| cn.value().into_owned()).expect("a CN")
}

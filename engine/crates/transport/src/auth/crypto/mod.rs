//! Keys, certificates and JWS of the control plane.
//!
//! It covers ECDSA P-384 keys, the self-signed identity (`CN=<dcgClientId>`) and trust
//! (`CN=trust_<dcgClientId>`) certificates, PKCS#8 persistence, the compact `ES384` JWS, the nonce
//! JWT (`certificateJWT`) and the data JWT (as `/Dispatcher/Wake` sends it), and the
//! signature/lifetime checks of inbound JWT validation.

use std::str::FromStr;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use base64::Engine as _;
use p384::ecdsa::signature::{Signer, Verifier};
use p384::ecdsa::{DerSignature, Signature, SigningKey, VerifyingKey};
use p384::elliptic_curve::Generate;
use p384::pkcs8::{DecodePrivateKey, DecodePublicKey, EncodePrivateKey};
use rand::RngCore;
use serde::Serialize;
use x509_cert::builder::profile::BuilderProfile;
use x509_cert::builder::{Builder, CertificateBuilder};
use x509_cert::der::{Decode, Encode};
use x509_cert::ext::Extension;
use x509_cert::ext::pkix::{BasicConstraints, KeyUsage, KeyUsages};
use x509_cert::name::Name;
use x509_cert::serial_number::SerialNumber;
use x509_cert::spki::SubjectPublicKeyInfoRef;
use x509_cert::time::{Time, Validity};
use x509_cert::{Certificate, SubjectPublicKeyInfo, TbsCertificate};

/// The literal JWS header of every JWT the PC signs (nonce / key-ownership and data JWTs).
pub const JWS_HEADER: &str = r#"{"alg":"ES384","typ":"JWT"}"#;
/// JWA name of ECDSA P-384 with SHA-384.
pub const JWS_ALG: &str = "ES384";
/// `ecdsa-with-SHA384`, the certificate signature algorithm.
pub const ECDSA_WITH_SHA384_OID: &str = "1.2.840.10045.4.3.3";
/// Prefix of a trust certificate's CN and of every `partner_client_id`.
pub const TRUST_PREFIX: &str = "trust_";

/// Certificate validity and clock drift.
pub const CERTIFICATE_CLOCK_DRIFT: Duration = phonelink_core::constants::lifetime::CERTIFICATE_CLOCK_DRIFT;
pub const CERTIFICATE_VALIDITY: Duration = phonelink_core::constants::lifetime::CERTIFICATE_VALIDITY;
/// JWT lifetime around *now*: `nbf = now − 12 h`, `exp = now + 12 h`.
pub const JWT_LIFETIME: Duration = phonelink_core::constants::lifetime::JWT_LIFETIME;
/// Clock skew accepted on inbound JWT lifetimes.
pub const JWT_CLOCK_SKEW: Duration = phonelink_core::constants::lifetime::JWT_CLOCK_DRIFT;

#[derive(Debug, thiserror::Error)]
pub enum CryptoError {
    #[error("certificate build failed: {0}")]
    CertificateBuild(String),
    #[error("certificate is not parsable: {0}")]
    CertificateParse(String),
    #[error("key encoding failed: {0}")]
    KeyEncoding(String),
    #[error("malformed JWS: {0}")]
    MalformedJws(String),
    #[error("JWS signature does not verify")]
    BadSignature,
    #[error("JWS rejected: {0}")]
    Rejected(String),
    #[error("time out of range: {0}")]
    Time(String),
    /// A key the PKCS#8 codec could not encode or decode.
    #[error("key encoding failed: {0}")]
    KeyCodec(#[source] Cause),
    /// A certificate the X.509 builder or DER encoder refused.
    #[error("certificate build failed: {0}")]
    CertificateEncoding(#[source] Cause),
    #[error("certificate is not parsable: {0}")]
    CertificateDecoding(#[source] Cause),
    /// A validity time the DER encoder refused.
    #[error("time out of range: {0}")]
    TimeEncoding(#[source] Cause),
}

/// The library error behind a [`CryptoError`].
pub type Cause = Box<dyn std::error::Error + Send + Sync>;

pub fn b64_std(bytes: &[u8]) -> String {
    base64::prelude::BASE64_STANDARD.encode(bytes)
}

/// Standard base64: white space (space, tab, CR, LF) is ignored and padding is required. The
/// phone's certificates arrive line-wrapped every 76 characters (Android `Base64.DEFAULT`).
pub fn b64_std_decode(text: &str) -> Result<Vec<u8>, CryptoError> {
    let compact: String = text.chars().filter(|c| !matches!(c, ' ' | '\t' | '\r' | '\n')).collect();
    base64::prelude::BASE64_STANDARD
        .decode(compact)
        .map_err(|e| CryptoError::CertificateParse(format!("not standard base64: {e}")))
}

pub fn b64url(bytes: &[u8]) -> String {
    base64::prelude::BASE64_URL_SAFE_NO_PAD.encode(bytes)
}

fn b64url_decode(text: &str) -> Result<Vec<u8>, CryptoError> {
    base64::prelude::BASE64_URL_SAFE_NO_PAD
        .decode(text)
        .map_err(|e| CryptoError::MalformedJws(format!("not base64url: {e}")))
}

/// Seconds since the Unix epoch (JWT `NumericDate`); times before the epoch are an invariant break.
pub fn unix_seconds(t: SystemTime) -> Result<i64, CryptoError> {
    let d = t.duration_since(UNIX_EPOCH).map_err(|_| CryptoError::Time("before the Unix epoch".into()))?;
    i64::try_from(d.as_secs()).map_err(|_| CryptoError::Time("beyond i64 seconds".into()))
}

// ------------------------------------------------------------------------------------------------
// Keys
// ------------------------------------------------------------------------------------------------

/// A fresh ECDSA P-384 signing key, the algorithm of the device identity signing key.
pub fn generate_signing_key() -> SigningKey {
    SigningKey::generate()
}

/// PKCS#8 DER of `key` (the persisted form; private keys are kept in the secure store).
pub fn key_to_pkcs8(key: &SigningKey) -> Result<Vec<u8>, CryptoError> {
    key.to_pkcs8_der().map(|doc| doc.as_bytes().to_vec()).map_err(|e| CryptoError::KeyCodec(e.into()))
}

pub fn key_from_pkcs8(der: &[u8]) -> Result<SigningKey, CryptoError> {
    SigningKey::from_pkcs8_der(der).map_err(|e| CryptoError::KeyCodec(e.into()))
}

// ------------------------------------------------------------------------------------------------
// Certificates
// ------------------------------------------------------------------------------------------------

/// Subject = issuer = `CN=<cn>`; the two extensions are added by the caller with explicit
/// criticality, so the profile contributes none.
struct SelfSignedProfile {
    name: Name,
}

impl BuilderProfile for SelfSignedProfile {
    fn get_issuer(&self, _subject: &Name) -> Name {
        self.name.clone()
    }

    fn get_subject(&self) -> Name {
        self.name.clone()
    }

    fn build_extensions(
        &self,
        _spk: SubjectPublicKeyInfoRef<'_>,
        _issuer_spk: SubjectPublicKeyInfoRef<'_>,
        _tbs: &TbsCertificate,
    ) -> x509_cert::builder::Result<Vec<Extension>> {
        Ok(Vec::new())
    }
}

/// A positive random serial number (16 bytes, top bit clear, first byte non-zero).
fn random_serial() -> Result<SerialNumber, CryptoError> {
    let mut bytes = [0u8; 16];
    loop {
        rand::rng().fill_bytes(&mut bytes);
        bytes[0] &= 0x7f;
        if bytes[0] != 0 {
            break;
        }
    }

    SerialNumber::new(&bytes).map_err(|e| CryptoError::CertificateEncoding(e.into()))
}

/// The self-signed identity or trust certificate.
///
/// Subject = issuer = `CN=<cn>`, ecdsa-with-SHA384, `notBefore = now − 12 h`,
/// `notAfter = now + 365 d`, `BasicConstraints` `CA=false` and `KeyUsage` `digitalSignature`, both
/// non-critical, positive random serial.
///
/// Returns the DER.
pub fn self_signed_certificate(key: &SigningKey, cn: &str, now: SystemTime) -> Result<Vec<u8>, CryptoError> {
    if cn.is_empty() || cn.contains([',', '=', '+', '"', '\\', '<', '>', ';']) {
        return Err(CryptoError::CertificateBuild(
            "the CN has a character a distinguished name does not allow".to_owned(),
        ));
    }

    let name =
        Name::from_str(&format!("CN={cn}")).map_err(|e| CryptoError::CertificateBuild(format!("subject: {e}")))?;
    let not_before =
        now.checked_sub(CERTIFICATE_CLOCK_DRIFT).ok_or_else(|| CryptoError::Time("notBefore underflow".into()))?;
    let not_after =
        now.checked_add(CERTIFICATE_VALIDITY).ok_or_else(|| CryptoError::Time("notAfter overflow".into()))?;
    let validity = Validity::new(
        Time::try_from(not_before).map_err(|e| CryptoError::TimeEncoding(e.into()))?,
        Time::try_from(not_after).map_err(|e| CryptoError::TimeEncoding(e.into()))?,
    );

    let spki = SubjectPublicKeyInfo::from_key(key.verifying_key())
        .map_err(|e| CryptoError::CertificateBuild(format!("spki: {e}")))?;
    let mut builder = CertificateBuilder::new(SelfSignedProfile { name }, random_serial()?, validity, spki)
        .map_err(|e| CryptoError::CertificateEncoding(e.into()))?;

    builder
        .add_extension((false, &BasicConstraints { ca: false, path_len_constraint: None }))
        .map_err(|e| CryptoError::CertificateBuild(format!("BasicConstraints: {e}")))?;
    builder
        .add_extension((false, &KeyUsage(KeyUsages::DigitalSignature.into())))
        .map_err(|e| CryptoError::CertificateBuild(format!("KeyUsage: {e}")))?;

    let cert = builder.build::<_, DerSignature>(key).map_err(|e| CryptoError::CertificateEncoding(e.into()))?;
    cert.to_der().map_err(|e| CryptoError::CertificateEncoding(e.into()))
}

/// The fields of a received or stored certificate that the control plane reads.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParsedCertificate {
    pub der: Vec<u8>,
    pub subject_cn: Option<String>,
    pub issuer_cn: Option<String>,
    pub not_before: SystemTime,
    pub not_after: SystemTime,
    /// DER of the `SubjectPublicKeyInfo`.
    pub spki_der: Vec<u8>,
}

impl ParsedCertificate {
    /// The certificate's ECDSA P-384 public key (ES384 verification needs exactly this curve).
    pub fn p384_verifying_key(&self) -> Result<VerifyingKey, CryptoError> {
        VerifyingKey::from_public_key_der(&self.spki_der)
            .map_err(|e| CryptoError::CertificateParse(format!("not a P-384 public key: {e}")))
    }

    /// The certificate in standard base64 (the `Certificate` claim and `certificates.SelfSigned`).
    pub fn base64(&self) -> String {
        b64_std(&self.der)
    }
}

fn name_cn(name: &Name) -> Result<Option<String>, CryptoError> {
    name.common_name()
        .map(|cn| cn.map(|s| s.value().into_owned()))
        .map_err(|e| CryptoError::CertificateParse(format!("CN: {e}")))
}

/// Parses an X.509 certificate from DER.
pub fn parse_certificate(der: &[u8]) -> Result<ParsedCertificate, CryptoError> {
    let cert = Certificate::from_der(der).map_err(|e| CryptoError::CertificateDecoding(e.into()))?;
    let tbs = cert.tbs_certificate();
    let spki_der =
        tbs.subject_public_key_info().to_der().map_err(|e| CryptoError::CertificateParse(format!("spki: {e}")))?;

    Ok(ParsedCertificate {
        der: der.to_vec(),
        subject_cn: name_cn(tbs.subject())?,
        issuer_cn: name_cn(tbs.issuer())?,
        not_before: tbs.validity().not_before.to_system_time(),
        not_after: tbs.validity().not_after.to_system_time(),
        spki_der,
    })
}

/// A private key with its certificate (identity or trust).
#[derive(Clone)]
pub struct KeyPair {
    pub key: SigningKey,
    pub cert: ParsedCertificate,
}

impl std::fmt::Debug for KeyPair {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("KeyPair").field("cert", &self.cert.subject_cn).finish_non_exhaustive()
    }
}

impl KeyPair {
    /// New key + self-signed certificate `CN=<cn>`.
    pub fn generate(cn: &str, now: SystemTime) -> Result<Self, CryptoError> {
        let key = generate_signing_key();
        let der = self_signed_certificate(&key, cn, now)?;
        Ok(Self { key, cert: parse_certificate(&der)? })
    }

    /// Rebuilds a pair from its persisted PKCS#8 key and DER certificate; the certificate must carry
    /// the key's public half.
    pub fn from_parts(pkcs8: &[u8], cert_der: &[u8]) -> Result<Self, CryptoError> {
        let key = key_from_pkcs8(pkcs8)?;
        let cert = parse_certificate(cert_der)?;
        if cert.p384_verifying_key()? != *key.verifying_key() {
            return Err(CryptoError::KeyEncoding("certificate does not match the private key".into()));
        }
        Ok(Self { key, cert })
    }

    pub fn cn(&self) -> &str {
        self.cert.subject_cn.as_deref().unwrap_or_default()
    }

    pub fn pkcs8(&self) -> Result<Vec<u8>, CryptoError> {
        key_to_pkcs8(&self.key)
    }
}

// ------------------------------------------------------------------------------------------------
// JWS
// ------------------------------------------------------------------------------------------------

/// Compact JWS over `claims` with [`JWS_HEADER`]; the signature is the raw 96-byte `r ‖ s`.
pub fn sign_jws<C: Serialize>(key: &SigningKey, claims: &C) -> Result<String, CryptoError> {
    let payload = serde_json::to_vec(claims).map_err(|e| CryptoError::MalformedJws(format!("claims: {e}")))?;
    let signing_input = format!("{}.{}", b64url(JWS_HEADER.as_bytes()), b64url(&payload));
    let signature: Signature = key.sign(signing_input.as_bytes());
    Ok(format!("{signing_input}.{}", b64url(&signature.to_bytes())))
}

fn lifetime(now: SystemTime) -> Result<(i64, i64), CryptoError> {
    let now = unix_seconds(now)?;
    let span = JWT_LIFETIME.as_secs() as i64;
    Ok((now - span, now + span))
}

#[derive(Serialize)]
struct NonceClaims<'a> {
    iss: &'a str,
    #[serde(rename = "Nonce")]
    nonce: &'a str,
    #[serde(rename = "Certificate")]
    certificate: String,
    nbf: i64,
    exp: i64,
}

/// The nonce JWT (`certificateJWT`, `oldCertificateJWT`, `newCertificateJWT`).
///
/// `iss` = the certificate's issuer CN, `Nonce`, `Certificate` = standard base64 of the certificate
/// DER, `nbf = now − 12 h`, `exp = now + 12 h`, no other claims; signed with the certificate's key.
pub fn nonce_jwt(pair: &KeyPair, nonce: &str, now: SystemTime) -> Result<String, CryptoError> {
    let iss = pair
        .cert
        .issuer_cn
        .as_deref()
        .ok_or_else(|| CryptoError::MalformedJws("certificate has no issuer CN".into()))?;

    if nonce.is_empty() {
        return Err(CryptoError::MalformedJws("empty nonce".into()));
    }

    let (nbf, exp) = lifetime(now)?;
    sign_jws(&pair.key, &NonceClaims { iss, nonce, certificate: pair.cert.base64(), nbf, exp })
}

#[derive(Serialize)]
struct DataClaims<'a> {
    iss: &'a str,
    #[serde(rename = "Data")]
    data: &'a str,
    #[serde(rename = "SourceId")]
    source_id: &'a str,
    #[serde(rename = "Scope")]
    scope: &'a str,
    nbf: i64,
    exp: i64,
}

/// A data JWT: `iss`, `Data`, `SourceId`, `Scope`, `nbf = now − 12 h`, `exp = now + 12 h`.
pub fn data_jwt(
    key: &SigningKey,
    iss: &str,
    data: &str,
    source_id: &str,
    scope: &str,
    now: SystemTime,
) -> Result<String, CryptoError> {
    let (nbf, exp) = lifetime(now)?;
    sign_jws(key, &DataClaims { iss, data, source_id, scope, nbf, exp })
}

/// A compact JWS split into its parts; nothing is verified yet.
#[derive(Debug, Clone, PartialEq)]
pub struct DecodedJws {
    pub header: serde_json::Value,
    pub claims: serde_json::Map<String, serde_json::Value>,
    /// The exact header JSON text (for golden checks).
    pub header_text: String,
    /// The exact claims JSON text.
    pub claims_text: String,
    signing_input: String,
    signature: Vec<u8>,
}

impl DecodedJws {
    pub fn claim_str(&self, name: &str) -> Option<&str> {
        self.claims.get(name).and_then(|v| v.as_str())
    }

    pub fn claim_i64(&self, name: &str) -> Option<i64> {
        self.claims.get(name).and_then(|v| v.as_i64())
    }

    #[cfg(feature = "test-support")]
    /// The raw signature bytes (96 for ES384).
    pub fn signature(&self) -> &[u8] {
        &self.signature
    }

    /// Verifies the ES384 signature with `key`.
    pub fn verify(&self, key: &VerifyingKey) -> Result<(), CryptoError> {
        if self.header.get("alg").and_then(|v| v.as_str()) != Some(JWS_ALG) {
            return Err(CryptoError::Rejected(format!("alg is not {JWS_ALG}")));
        }

        let sig = Signature::from_slice(&self.signature).map_err(|_| CryptoError::BadSignature)?;
        key.verify(self.signing_input.as_bytes(), &sig).map_err(|_| CryptoError::BadSignature)
    }
}

/// Splits and decodes a compact JWS without verifying it.
pub fn decode_jws(token: &str) -> Result<DecodedJws, CryptoError> {
    let mut parts = token.split('.');
    let (Some(h), Some(p), Some(s), None) = (parts.next(), parts.next(), parts.next(), parts.next()) else {
        return Err(CryptoError::MalformedJws("expected three dot-separated parts".into()));
    };

    let header_text =
        String::from_utf8(b64url_decode(h)?).map_err(|_| CryptoError::MalformedJws("header is not UTF-8".into()))?;
    let claims_text =
        String::from_utf8(b64url_decode(p)?).map_err(|_| CryptoError::MalformedJws("claims are not UTF-8".into()))?;
    let header: serde_json::Value =
        serde_json::from_str(&header_text).map_err(|e| CryptoError::MalformedJws(format!("header: {e}")))?;
    let claims = match serde_json::from_str::<serde_json::Value>(&claims_text) {
        Ok(serde_json::Value::Object(map)) => map,
        Ok(_) => return Err(CryptoError::MalformedJws("claims are not a JSON object".into())),
        Err(e) => return Err(CryptoError::MalformedJws(format!("claims: {e}"))),
    };

    Ok(DecodedJws {
        header,
        claims,
        header_text,
        claims_text,
        signing_input: format!("{h}.{p}"),
        signature: b64url_decode(s)?,
    })
}

/// The inbound JWT validation checks against one partner certificate.
///
/// `iss` = the certificate's CN, `aud` (when present) = `expected_audience`, lifetime with the 12 h
/// skew (`exp` required, `nbf` when present), ECDSA P-384 signature.
pub fn validate_jws_with_certificate(
    jws: &DecodedJws,
    cert: &ParsedCertificate,
    expected_audience: &str,
    now: SystemTime,
) -> Result<(), CryptoError> {
    let iss = jws.claim_str("iss").ok_or_else(|| CryptoError::Rejected("no iss".into()))?;
    let cn = cert.subject_cn.as_deref().ok_or_else(|| CryptoError::Rejected("partner certificate has no CN".into()))?;
    if !iss.eq_ignore_ascii_case(cn) {
        return Err(CryptoError::Rejected("iss is not the certificate CN".to_owned()));
    }

    if let Some(aud) = jws.claims.get("aud") {
        let ok = match aud {
            serde_json::Value::String(a) => a.eq_ignore_ascii_case(expected_audience),
            serde_json::Value::Array(items) => {
                items.iter().any(|a| a.as_str().is_some_and(|a| a.eq_ignore_ascii_case(expected_audience)))
            }
            _ => false,
        };
        if !ok {
            return Err(CryptoError::Rejected("aud is not the expected audience".to_owned()));
        }
    }

    let now_s = unix_seconds(now)?;
    let skew = JWT_CLOCK_SKEW.as_secs() as i64;
    let exp = jws.claim_i64("exp").ok_or_else(|| CryptoError::Rejected("no exp".into()))?;
    if now_s > exp + skew {
        return Err(CryptoError::Rejected("expired".into()));
    }

    if let Some(nbf) = jws.claims.get("nbf") {
        let nbf = nbf.as_i64().ok_or_else(|| CryptoError::Rejected("nbf is not a number".into()))?;
        if now_s < nbf - skew {
            return Err(CryptoError::Rejected("not yet valid".into()));
        }
    }

    jws.verify(&cert.p384_verifying_key()?)
}

#[cfg(test)]
mod tests;

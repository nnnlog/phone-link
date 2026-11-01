//! The WS-Trust `RST2.srf` requests and responses.
//!
//! It covers the device-ticket request (RSA-signed with the device key) and the
//! per-`(scope, client id)` token exchange (HMAC-signed with a key derived from the device proof
//! secret), their XML-DSig over exclusive canonicalization, response decryption, and the consent
//! fault.

use base64::Engine as _;
use base64::prelude::BASE64_STANDARD;
use hmac::{Hmac, KeyInit, Mac};
use rsa::Pkcs1v15Sign;
use rsa::RsaPrivateKey;
use sha2::{Digest, Sha256};

use super::c14n::Elem;
use super::license::{self, KDF_LABEL};

mod response;
#[cfg(feature = "test-support")]
pub use response::RstFault;
pub use response::{IssuedToken, RstResponse, parse_response};

type HmacSha256 = Hmac<Sha256>;

// Namespace URIs.
const NS_S: &str = "http://www.w3.org/2003/05/soap-envelope";
const NS_PS: &str = "http://schemas.microsoft.com/Passport/SoapServices/PPCRL";
const NS_WSSE: &str = "http://docs.oasis-open.org/wss/2004/01/oasis-200401-wss-wssecurity-secext-1.0.xsd";
const NS_WSU: &str = "http://docs.oasis-open.org/wss/2004/01/oasis-200401-wss-wssecurity-utility-1.0.xsd";
const NS_WSA: &str = "http://www.w3.org/2005/08/addressing";
const NS_WSSC: &str = "http://schemas.xmlsoap.org/ws/2005/02/sc";
const NS_WST: &str = "http://schemas.xmlsoap.org/ws/2005/02/trust";
const NS_WSP: &str = "http://schemas.xmlsoap.org/ws/2004/09/policy";
const NS_DS: &str = "http://www.w3.org/2000/09/xmldsig#";

const SIG_RSA: &str = "http://www.w3.org/2001/04/xmldsig-more#rsa-sha256";
const SIG_HMAC: &str = "http://www.w3.org/2001/04/xmldsig-more#hmac-sha256";
const C14N_EXC: &str = "http://www.w3.org/2001/10/xml-exc-c14n#";
const DIGEST_SHA256: &str = "http://www.w3.org/2001/04/xmlenc#sha256";
const KDF_ALG: &str = "urn:liveid:SP800108_CTR_HMAC_SHA256_DOUBLEDERIVED";

/// The ASN.1 `DigestInfo` prefix for a SHA-256 hash, prepended before RSASSA-PKCS1-v1.5 signing.
const SHA256_DIGESTINFO_PREFIX: [u8; 19] =
    [0x30, 0x31, 0x30, 0x0d, 0x06, 0x09, 0x60, 0x86, 0x48, 0x01, 0x65, 0x03, 0x04, 0x02, 0x01, 0x05, 0x00, 0x04, 0x20];

#[derive(Debug, thiserror::Error)]
pub enum RstError {
    #[error("RST2 response is not valid XML: {0}")]
    Xml(String),
    #[error("RST2 response is not valid XML: {0}")]
    XmlParse(#[source] Box<dyn std::error::Error + Send + Sync>),
    #[error("RST2 response is missing {0}")]
    Missing(&'static str),
    #[error("RST2 response could not be decrypted: {0}")]
    Decrypt(#[from] license::LicenseError),
    #[error("base64: {0}")]
    Base64(#[from] base64::DecodeError),
    #[error("RSA signing failed: {0}")]
    Rsa(#[from] rsa::Error),
    #[error("RST2 response has an unexpected {0}")]
    Unexpected(String),
}

/// A locally signed timestamp window: `created` now, `expires` five minutes on. RFC-3339, seconds,
/// trailing `Z`.
pub struct Window {
    pub created: String,
    pub expires: String,
}

/// The fixed `ps:AuthInfo` fields plus the per-request overrides.
pub struct AuthInfo<'a> {
    pub hosting_app: &'a str,
    pub sso_flags: Option<&'a str>,
    pub inline_ux: &'a str,
    pub inline_ft: Option<&'a str>,
    pub license_signature_key_version: Option<&'a str>,
}

impl AuthInfo<'_> {
    fn element(&self) -> Elem {
        // The fields are in the order the account service accepts. `SSOFlags` is always present
        // (empty when unset): the account service rejects the request's XML signature if it is
        // omitted, even though every element is self-digesting. `InlineFT`
        // follows `ClientCapabilities`.
        let sso_flags = match self.sso_flags {
            Some(v) => Elem::new("ps", "SSOFlags", NS_PS).text(v),
            None => Elem::new("ps", "SSOFlags", NS_PS),
        };

        let mut e = Elem::new("ps", "AuthInfo", NS_PS)
            .attr("", "Id", "", "PPAuthInfo")
            .child(sso_flags)
            .child(Elem::new("ps", "HostingApp", NS_PS).text(self.hosting_app))
            .child(Elem::new("ps", "BinaryVersion", NS_PS).text("55"))
            .child(Elem::new("ps", "UIVersion", NS_PS).text("1"))
            .child(Elem::new("ps", "InlineUX", NS_PS).text(self.inline_ux))
            .child(Elem::new("ps", "IsAdmin", NS_PS).text("1"))
            .child(Elem::new("ps", "Cookies", NS_PS))
            .child(Elem::new("ps", "RequestParams", NS_PS).text("AQAAAAIAAABsYwQAAAAxMDMz"))
            .child(Elem::new("ps", "WindowsClientString", NS_PS).text("b4d/QB7Zy5pjUAY9ByQ1echTyTITx6ZCErOEztuIVtw="));

        if let Some(v) = self.license_signature_key_version {
            e = e.child(Elem::new("ps", "LicenseSignatureKeyVersion", NS_PS).text(v));
        }
        e = e.child(Elem::new("ps", "ClientCapabilities", NS_PS).text("1"));
        if let Some(v) = self.inline_ft {
            e = e.child(Elem::new("ps", "InlineFT", NS_PS).text(v));
        }

        e
    }
}

/// The `wsu:Timestamp` element.
fn timestamp_element(window: &Window) -> Elem {
    Elem::new("wsu", "Timestamp", NS_WSU)
        .attr("wsu", "Id", NS_WSU, "Timestamp")
        .child(Elem::new("wsu", "Created", NS_WSU).text(&window.created))
        .child(Elem::new("wsu", "Expires", NS_WSU).text(&window.expires))
}

/// One `wst:RequestSecurityToken` (`Id="RST{i}"`) for a scope, with an optional policy reference.
fn rst_element(id: &str, address: &str, token_broker_policy: bool) -> Elem {
    let mut e = Elem::new("wst", "RequestSecurityToken", NS_WST)
        .attr("", "Id", "", id)
        .child(Elem::new("wst", "RequestType", NS_WST).text("http://schemas.xmlsoap.org/ws/2005/02/trust/Issue"))
        .child(Elem::new("wsp", "AppliesTo", NS_WSP).child(
            Elem::new("wsa", "EndpointReference", NS_WSA).child(Elem::new("wsa", "Address", NS_WSA).text(address)),
        ));

    if token_broker_policy {
        e = e.child(Elem::new("wsp", "PolicyReference", NS_WSP).attr("", "URI", "", "TOKEN_BROKER").text(""));
    }
    e
}

/// The SHA-256 digest of an element's exclusive-canonical form, base64 — a `DigestValue`.
fn digest_value(el: &Elem) -> String {
    let hash = Sha256::digest(el.exc_c14n().as_bytes());
    BASE64_STANDARD.encode(hash)
}

/// Builds the `SignedInfo` element over three references (`#RST0`/`#RSTS`, `#Timestamp`,
/// `#PPAuthInfo`), given each reference's already-computed digest and the signature method.
fn signed_info(method: &str, refs: &[(&str, String)]) -> Elem {
    let mut si = Elem::new("", "SignedInfo", NS_DS)
        .child(Elem::new("", "CanonicalizationMethod", NS_DS).attr("", "Algorithm", "", C14N_EXC))
        .child(Elem::new("", "SignatureMethod", NS_DS).attr("", "Algorithm", "", method));

    for (uri, digest) in refs {
        si = si.child(
            Elem::new("", "Reference", NS_DS)
                .attr("", "URI", "", uri)
                .child(Elem::new("", "Transforms", NS_DS).child(Elem::new("", "Transform", NS_DS).attr(
                    "",
                    "Algorithm",
                    "",
                    C14N_EXC,
                )))
                .child(Elem::new("", "DigestMethod", NS_DS).attr("", "Algorithm", "", DIGEST_SHA256))
                .child(Elem::new("", "DigestValue", NS_DS).text(digest)),
        );
    }
    si
}

/// RSASSA-PKCS1-v1.5 over SHA-256 of `canonical`, with the `DigestInfo` prepended by hand so no
/// cross-version SHA-256 binding is needed.
fn rsa_sign(key: &RsaPrivateKey, canonical: &[u8]) -> Result<String, RstError> {
    let hash = Sha256::digest(canonical);
    let mut digest_info = SHA256_DIGESTINFO_PREFIX.to_vec();
    digest_info.extend_from_slice(&hash);
    let sig = key.sign(Pkcs1v15Sign::new_unprefixed(), &digest_info)?;
    Ok(BASE64_STANDARD.encode(sig))
}

/// HMAC-SHA256 of `canonical` under the derived key, base64.
fn hmac_sign(derived_key: &[u8; 32], canonical: &[u8]) -> String {
    let mut mac = HmacSha256::new_from_slice(derived_key).expect("HMAC takes a key of any length");
    mac.update(canonical);
    BASE64_STANDARD.encode(mac.finalize().into_bytes())
}

/// The `Signature` element for the RSA path: no `KeyInfo` (the server resolves the device cert).
fn rsa_signature(si: &Elem, signature_value: &str) -> Elem {
    Elem::new("", "Signature", NS_DS)
        .child(si.clone())
        .child(Elem::new("", "SignatureValue", NS_DS).text(signature_value))
}

/// The `Signature` element for the HMAC path: `KeyInfo` points at the `#SignKey` derived-key token.
fn hmac_signature(si: &Elem, signature_value: &str) -> Elem {
    Elem::new("", "Signature", NS_DS)
        .child(si.clone())
        .child(Elem::new("", "SignatureValue", NS_DS).text(signature_value))
        .child(
            Elem::new("", "KeyInfo", NS_DS).child(
                Elem::new("wsse", "SecurityTokenReference", NS_WSSE)
                    .child(Elem::new("wsse", "Reference", NS_WSSE).attr("", "URI", "", "#SignKey")),
            ),
        )
}

/// The `wssc:DerivedKeyToken` that carries the per-request nonce the HMAC key is derived from.
fn derived_key_token(nonce_b64: &str) -> Elem {
    Elem::new("wssc", "DerivedKeyToken", NS_WSSC)
        .attr("wsu", "Id", NS_WSU, "SignKey")
        .attr("", "Algorithm", "", KDF_ALG)
        .child(
            Elem::new("wsse", "RequestedTokenReference", NS_WSSE)
                .child(Elem::new("wsse", "KeyIdentifier", NS_WSSE).attr(
                    "",
                    "ValueType",
                    "",
                    "http://docs.oasis-open.org/wss/2004/XX/oasis-2004XX-wss-saml-token-profile-1.0#SAMLAssertionID",
                ))
                .child(Elem::new("wsse", "Reference", NS_WSSE).attr("", "URI", "", "#DeviceDAToken")),
        )
        .child(Elem::new("wssc", "Nonce", NS_WSSC).text(nonce_b64))
}

/// Assembles the envelope from a built `ps:AuthInfo`, the `wsse:Security` header, and the body
/// `wst:RequestSecurityToken`(s), and serializes it.
fn envelope_document(auth_info: Elem, security: Elem, body: Elem) -> String {
    let header = Elem::new("s", "Header", NS_S)
        .child(
            Elem::new("wsa", "Action", NS_WSA)
                .attr("s", "mustUnderstand", NS_S, "1")
                .text("http://schemas.xmlsoap.org/ws/2005/02/trust/RST/Issue"),
        )
        .child(
            Elem::new("wsa", "To", NS_WSA)
                .attr("s", "mustUnderstand", NS_S, "1")
                .text("https://login.live.com:443/RST2.srf"),
        )
        .child(
            Elem::new("wsa", "MessageID", NS_WSA)
                .text(&phonelink_core::time::unix_secs(std::time::SystemTime::now()).max(0).to_string()),
        )
        .child(auth_info)
        .child(security);

    let envelope = Elem::new("s", "Envelope", NS_S).child(header).child(Elem::new("s", "Body", NS_S).child(body));
    envelope.serialize_document()
}

/// Builds the RSA-signed device-ticket request: a single `http://Passport.NET/tb` scope, the
/// membername in a `wsse:UsernameToken`, signed with the device RSA key.
pub fn device_ticket_request(membername: &str, window: &Window, key: &RsaPrivateKey) -> Result<String, RstError> {
    let auth_info = AuthInfo {
        hosting_app: "{DF60E2DF-88AD-4526-AE21-83D130EF0F68}",
        sso_flags: None,
        inline_ux: "TokenBroker",
        inline_ft: None,
        license_signature_key_version: Some("2"),
    };

    let username = Elem::new("wsse", "UsernameToken", NS_WSSE)
        .attr("wsu", "Id", NS_WSU, "devicesoftware")
        .child(Elem::new("wsse", "Username", NS_WSSE).text(membername));
    let timestamp = timestamp_element(window);
    let body = rst_element("RST0", "http://Passport.NET/tb", false);
    let auth_el = auth_info.element();

    let refs = [
        ("#RST0", digest_value(&body)),
        ("#Timestamp", digest_value(&timestamp)),
        ("#PPAuthInfo", digest_value(&auth_el)),
    ];
    let si = signed_info(SIG_RSA, &refs);
    let sig_value = rsa_sign(key, si.exc_c14n().as_bytes())?;
    let signature = rsa_signature(&si, &sig_value);

    let security = Elem::new("wsse", "Security", NS_WSSE).child(username).child(timestamp).child(signature);
    Ok(envelope_document(auth_el, security, body))
}

/// One requested token: the scope address and whether it carries the `TOKEN_BROKER` policy.
pub struct ScopeRequest {
    pub address: String,
    pub token_broker_policy: bool,
}

/// Builds the HMAC-signed app-token exchange.
///
/// The user token (an `EncryptedData` element — the sign-in page's DA token, or the master token a
/// previous exchange issued) and the device ticket (`BinarySecurityToken`), one or more scopes,
/// signed with a key derived from the device proof secret and `nonce`.
///
/// `inline_ux` is `TokenBroker` for the exchange right after an interactive sign-in or
/// consent (which carries that page's `inline_ft`) and `Silent` otherwise.
#[expect(clippy::too_many_arguments, reason = "independent inputs of one operation; not yet grouped")]
pub fn exchange_request(
    username_hint: &str,
    device_da_token: &str,
    user_token_xml: &str,
    hosting_app: &str,
    inline_ux: &str,
    inline_ft: Option<&str>,
    scopes: &[ScopeRequest],
    window: &Window,
    proof_secret: &[u8; 32],
    nonce: &[u8; 32],
) -> Result<String, RstError> {
    if scopes.is_empty() {
        return Err(RstError::Missing("a requested scope"));
    }

    // The user token is embedded byte-for-byte as the service issued it; it must stand alone.
    let user_doc = roxmltree::Document::parse(user_token_xml).map_err(|e| RstError::XmlParse(e.into()))?;
    if user_doc.root_element().tag_name().name() != "EncryptedData" {
        return Err(RstError::Unexpected(format!("user token <{}>", user_doc.root_element().tag_name().name())));
    }

    let auth_info = AuthInfo {
        hosting_app,
        sso_flags: Some("SsoRestr"),
        inline_ux,
        inline_ft,
        license_signature_key_version: None,
    };
    let username = Elem::new("wsse", "UsernameToken", NS_WSSE)
        .attr("wsu", "Id", NS_WSU, "user")
        .child(Elem::new("wsse", "UsernameHint", NS_WSSE).text(username_hint))
        .child(Elem::new("wsse", "LoginOption", NS_WSSE).text("1"));
    let device_bst = Elem::new("wsse", "BinarySecurityToken", NS_WSSE)
        .attr("", "id", "", "DeviceDAToken")
        .attr("", "ValueType", "", "urn:liveid:device")
        .text(device_da_token);

    let nonce_b64 = BASE64_STANDARD.encode(nonce);
    let dkt = derived_key_token(&nonce_b64);
    let timestamp = timestamp_element(window);

    let (body, first_ref) = if scopes.len() > 1 {
        let mut multi = Elem::new("ps", "RequestMultipleSecurityTokens", NS_PS).attr("", "Id", "", "RSTS");
        for (i, s) in scopes.iter().enumerate() {
            multi = multi.child(rst_element(&format!("RST{i}"), &s.address, s.token_broker_policy));
        }
        (multi, "#RSTS")
    } else {
        (rst_element("RST0", &scopes[0].address, scopes[0].token_broker_policy), "#RST0")
    };
    let auth_el = auth_info.element();

    let refs = [
        (first_ref, digest_value(&body)),
        ("#Timestamp", digest_value(&timestamp)),
        ("#PPAuthInfo", digest_value(&auth_el)),
    ];
    let si = signed_info(SIG_HMAC, &refs);

    let derived = license::sp800_108(proof_secret, KDF_LABEL, nonce, 32);
    let derived: [u8; 32] = derived.try_into().expect("sp800_108 returns the requested length");
    let sig_value = hmac_sign(&derived, si.exc_c14n().as_bytes());
    let signature = hmac_signature(&si, &sig_value);

    // The token broker's `wsse:Security` order: the user's UsernameToken, the user token, the device
    // token, the derived key, the timestamp, the signature.
    let security = Elem::new("wsse", "Security", NS_WSSE)
        .child(username)
        .raw(user_token_xml)
        .child(device_bst)
        .child(dkt)
        .child(timestamp)
        .child(signature);
    Ok(envelope_document(auth_el, security, body))
}

#[cfg(test)]
mod encrypted_response_tests {
    //! Parsing of signed-exchange responses in the account service's layouts: a header with
    //! two derived keys (`SignKey`, `EncKey`, each referencing the device token by a hash, not a
    //! `#id`) and a `psf:EncryptedPP`, and a body that is either a plaintext fault or an encrypted
    //! `EncryptedData` holding the issued tokens.
    use super::*;
    use aes::cipher::{BlockEncryptMut, KeyIvInit, block_padding::Pkcs7};

    const PROOF: [u8; 32] = [7u8; 32];
    const SIGN_NONCE: [u8; 32] = [1u8; 32];
    const ENC_NONCE: [u8; 32] = [2u8; 32];

    /// A token-broker `CipherValue` under the `EncKey` nonce: AES-256-CBC, PKCS7, IV prepended, base64.
    fn encrypt(plain: &str) -> String {
        let key: [u8; 32] = license::sp800_108(&PROOF, KDF_LABEL, &ENC_NONCE, 32).try_into().unwrap();
        let iv = [9u8; 16];
        type Enc = cbc::Encryptor<aes::Aes256>;
        let ct = Enc::new((&key).into(), (&iv).into()).encrypt_padded_vec_mut::<Pkcs7>(plain.as_bytes());
        let mut buf = iv.to_vec();
        buf.extend_from_slice(&ct);
        BASE64_STANDARD.encode(buf)
    }

    fn encrypted_data(id: &str, plain: &str) -> String {
        format!(
            r##"<EncryptedData xmlns="http://www.w3.org/2001/04/xmlenc#" Id="{id}" Type="http://www.w3.org/2001/04/xmlenc#Element"><EncryptionMethod Algorithm="http://www.w3.org/2001/04/xmlenc#aes256-cbc"></EncryptionMethod><ds:KeyInfo xmlns:ds="http://www.w3.org/2000/09/xmldsig#"><wsse:SecurityTokenReference><wsse:Reference URI="#EncKey"></wsse:Reference></wsse:SecurityTokenReference></ds:KeyInfo><CipherData><CipherValue>{}</CipherValue></CipherData></EncryptedData>"##,
            encrypt(plain)
        )
    }

    /// The envelope: the derived keys, an encrypted `psf:pp`, and `body` as the `S:Body` content.
    fn envelope(pp_plain: &str, body: &str) -> String {
        format!(
            r##"<?xml version="1.0" encoding="utf-8" ?><S:Envelope xmlns:S="http://www.w3.org/2003/05/soap-envelope" xmlns:wsse="http://docs.oasis-open.org/wss/2004/01/oasis-200401-wss-wssecurity-secext-1.0.xsd" xmlns:wsu="http://docs.oasis-open.org/wss/2004/01/oasis-200401-wss-wssecurity-utility-1.0.xsd" xmlns:wsa="http://www.w3.org/2005/08/addressing" xmlns:wssc="http://schemas.xmlsoap.org/ws/2005/02/sc" xmlns:wst="http://schemas.xmlsoap.org/ws/2005/02/trust" xmlns:wsp="http://schemas.xmlsoap.org/ws/2004/09/policy" xmlns:psf="http://schemas.microsoft.com/Passport/SoapServices/SOAPFault"><S:Header><wsse:Security S:mustUnderstand="1"><wsu:Timestamp wsu:Id="TS"><wsu:Created>2026-10-25T04:11:54Z</wsu:Created><wsu:Expires>2026-10-25T04:16:54Z</wsu:Expires></wsu:Timestamp><wssc:DerivedKeyToken wsu:Id="SignKey" Algorithm="urn:liveid:SP800108_CTR_HMAC_SHA256_DOUBLEDERIVED"><wsse:SecurityTokenReference><wsse:Reference URI="CtbewGWQuTBCylVaOqyuZq64Bl0="></wsse:Reference></wsse:SecurityTokenReference><wssc:Nonce>{}</wssc:Nonce></wssc:DerivedKeyToken><wssc:DerivedKeyToken wsu:Id="EncKey" Algorithm="urn:liveid:SP800108_CTR_HMAC_SHA256_DOUBLEDERIVED"><wsse:SecurityTokenReference><wsse:Reference URI="CtbewGWQuTBCylVaOqyuZq64Bl0="></wsse:Reference></wsse:SecurityTokenReference><wssc:Nonce>{}</wssc:Nonce></wssc:DerivedKeyToken></wsse:Security><psf:EncryptedPP>{}</psf:EncryptedPP></S:Header><S:Body>{body}</S:Body></S:Envelope>"##,
            BASE64_STANDARD.encode(SIGN_NONCE),
            BASE64_STANDARD.encode(ENC_NONCE),
            encrypted_data("EncPsf", pp_plain),
        )
    }

    /// The informational `psf:pp` of an accepted exchange (no consent asked for). Its prefix is
    /// declared only on the envelope, as the plaintext of an encrypted fragment may be.
    const PP_OK: &str = "<psf:pp><psf:PUID>0003BFFD00000000</psf:PUID><psf:reqstatus>0x0</psf:reqstatus></psf:pp>";

    #[test]
    fn issued_tokens_in_an_encrypted_body_are_read_past_the_encrypted_pp() {
        // The header's EncryptedPP is the first EncryptedData in document order, and the body's (the
        // issued tokens) must still be decrypted. The plaintext uses the
        // envelope's prefixes without declaring them, and the master token's EncryptedData relies on
        // an inherited `ds` prefix.
        let tokens = r#"<wst:RequestSecurityTokenResponseCollection xmlns:ds="http://www.w3.org/2000/09/xmldsig#"><wst:RequestSecurityTokenResponse><wst:TokenType>urn:passport:delegationcompact</wst:TokenType><wsp:AppliesTo><wsa:EndpointReference><wsa:Address>scope=service::msatoken.dcg.microsoft.com::MBI_SSL&amp;api-version=2.0&amp;clientid=APP</wsa:Address></wsa:EndpointReference></wsp:AppliesTo><wst:Lifetime><wsu:Created>2026-10-24T14:57:01Z</wsu:Created><wsu:Expires>2026-10-25T14:57:01Z</wsu:Expires></wst:Lifetime><wst:RequestedSecurityToken><wsse:BinarySecurityToken Id="Compact0">EwDCG</wsse:BinarySecurityToken></wst:RequestedSecurityToken></wst:RequestSecurityTokenResponse><wst:RequestSecurityTokenResponse><wst:TokenType>urn:passport:legacy</wst:TokenType><wsp:AppliesTo><wsa:EndpointReference><wsa:Address>http://Passport.NET/tb</wsa:Address></wsa:EndpointReference></wsp:AppliesTo><wst:Lifetime><wsu:Created>2026-10-24T14:57:01Z</wsu:Created><wsu:Expires>2027-01-22T14:57:01Z</wsu:Expires></wst:Lifetime><wst:RequestedSecurityToken><EncryptedData xmlns="http://www.w3.org/2001/04/xmlenc#" Id="BinaryDAToken1"><ds:KeyInfo><ds:KeyName>http://Passport.NET/STS</ds:KeyName></ds:KeyInfo><CipherData><CipherValue>TUFTVEVS</CipherValue></CipherData></EncryptedData></wst:RequestedSecurityToken></wst:RequestSecurityTokenResponse></wst:RequestSecurityTokenResponseCollection>"#;
        let response = envelope(PP_OK, &encrypted_data("BodyEnc", tokens));

        let RstResponse::Issued(issued) = parse_response(&response, Some(&PROOF)).unwrap() else {
            panic!("expected issued tokens");
        };

        assert_eq!(issued.len(), 2);

        let dcg = &issued[0];
        assert_eq!(dcg.token_type, "urn:passport:delegationcompact");
        assert_eq!(dcg.applies_to, "scope=service::msatoken.dcg.microsoft.com::MBI_SSL&api-version=2.0&clientid=APP");
        // The bearer is the BinarySecurityToken value, without a `d=` prefix.
        assert_eq!(dcg.token, "EwDCG");
        assert_eq!(dcg.expires.as_deref(), Some("2026-10-25T14:57:01Z"));

        let master = &issued[1];
        assert_eq!(master.token_type, "urn:passport:legacy");
        assert_eq!(master.applies_to, "http://Passport.NET/tb");

        // Standalone: the inherited `ds` is declared on it, and it parses on its own.
        let xml = master.legacy_xml.as_deref().unwrap();
        assert!(xml.starts_with(r#"<EncryptedData xmlns:ds="http://www.w3.org/2000/09/xmldsig#""#), "{xml}");
        let alone = roxmltree::Document::parse(xml).unwrap();
        assert_eq!(alone.root_element().tag_name().name(), "EncryptedData");
        assert!(xml.contains("TUFTVEVS"));
    }

    #[test]
    fn a_needed_consent_arrives_in_the_encrypted_pp_with_a_plaintext_fault() {
        let pp = "<psf:pp><psf:reqstatus>0x800488bd</psf:reqstatus><psf:inlineauthurl>https://login.live.com/ppsecure/InlineClientAuth.srf?id=1&amp;x=2</psf:inlineauthurl><psf:inlineendauthurl>https://login.live.com/ppsecure/InlineDesktop.srf</psf:inlineendauthurl></psf:pp>";
        let fault = r#"<S:Fault><S:Code><S:Value>S:Sender</S:Value><S:Subcode><S:Value>wst:FailedAuthentication</S:Value></S:Subcode></S:Code><S:Reason><S:Text xml:lang="en-US">Plaform SSO permission required</S:Text></S:Reason><S:Detail><psf:error><psf:value>0x800488bd</psf:value><psf:internalerror><psf:code>0x800478e7</psf:code><psf:text>Platform SSO permission interrupt</psf:text></psf:internalerror></psf:error></S:Detail></S:Fault>"#;

        assert_eq!(
            parse_response(&envelope(pp, fault), Some(&PROOF)).unwrap(),
            RstResponse::Consent {
                inline_auth_url: "https://login.live.com/ppsecure/InlineClientAuth.srf?id=1&x=2".into(),
                end_url: Some("https://login.live.com/ppsecure/InlineDesktop.srf".into()),
            }
        );
    }

    #[test]
    fn an_encrypted_refusal_is_a_fault() {
        let fault = r#"<S:Fault><S:Code><S:Value>S:Sender</S:Value><S:Subcode><S:Value>wst:FailedAuthentication</S:Value></S:Subcode></S:Code><S:Reason><S:Text>Authentication Failure</S:Text></S:Reason><S:Detail><psf:error><psf:value>0x80048821</psf:value><psf:internalerror><psf:code>0x80048823</psf:code></psf:internalerror></psf:error></S:Detail></S:Fault>"#;

        let RstResponse::Fault(f) =
            parse_response(&envelope(PP_OK, &encrypted_data("BodyEnc", fault)), Some(&PROOF)).unwrap()
        else {
            panic!("expected a fault");
        };

        assert_eq!(f.req_status.as_deref(), Some("0x80048821"));
        assert_eq!(f.internal_code.as_deref(), Some("0x80048823"));
        assert!(!f.is_request_error());
    }

    #[test]
    fn a_per_token_consent_in_a_collection_is_a_consent() {
        let tokens = r#"<wst:RequestSecurityTokenResponseCollection><wst:RequestSecurityTokenResponse><wst:TokenType>urn:passport:compact</wst:TokenType><wsp:AppliesTo><wsa:EndpointReference><wsa:Address>scope=a</wsa:Address></wsa:EndpointReference></wsp:AppliesTo><psf:pp><psf:reqstatus>0x800488bd</psf:reqstatus><psf:inlineauthurl>https://login.live.com/ppsecure/InlineClientAuth.srf?id=9</psf:inlineauthurl></psf:pp></wst:RequestSecurityTokenResponse></wst:RequestSecurityTokenResponseCollection>"#;

        assert_eq!(
            parse_response(&envelope(PP_OK, &encrypted_data("BodyEnc", tokens)), Some(&PROOF)).unwrap(),
            RstResponse::Consent {
                inline_auth_url: "https://login.live.com/ppsecure/InlineClientAuth.srf?id=9".into(),
                end_url: None,
            }
        );
    }

    #[test]
    fn decryption_uses_the_referenced_derived_key_not_the_first() {
        // Encrypted under EncKey; decrypting under SignKey (the first nonce) would fail the padding.
        let response = envelope(
            PP_OK,
            &encrypted_data("BodyEnc", r#"<S:Fault><S:Reason><S:Text>x</S:Text></S:Reason></S:Fault>"#),
        );
        assert!(matches!(parse_response(&response, Some(&PROOF)).unwrap(), RstResponse::Fault(_)));

        // Without the proof secret the encrypted parts cannot be read: fail loudly.
        assert!(matches!(parse_response(&response, None).unwrap_err(), RstError::Missing(_)));
    }
}

#[cfg(test)]
mod tests;

//! Reading an RST2 response: issued tokens, faults, and decrypting the encrypted parts.

use super::RstError;
use crate::auth::live::license;
use crate::auth::live::license::KDF_LABEL;
use base64::Engine;
use base64::prelude::BASE64_STANDARD;

/// What a token-broker response carries: the issued tokens, a consent the user must give first, or a
/// refusal.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RstResponse {
    Issued(Vec<IssuedToken>),
    /// The `(scope, client id)` needs a one-time consent: open `inline_auth_url` until it reaches
    /// `end_url` (the fault's `psf:inlineendauthurl`), then retry the exchange.
    Consent {
        inline_auth_url: String,
        end_url: Option<String>,
    },
    /// A SOAP fault (or a requested token the service refused) that is not a consent.
    Fault(RstFault),
}

/// A refusal's details: the `S:Reason` text, the fault subcode, and the `psf` status codes.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RstFault {
    pub reason: String,
    pub subcode: Option<String>,
    pub req_status: Option<String>,
    pub internal_code: Option<String>,
    pub internal_text: Option<String>,
}

impl RstFault {
    /// The service rejected the request's own construction (its signature, `0x80041317`, or the proof
    /// key derived from the device token, `0x80045c38`). Signing in again cannot fix that.
    pub fn is_request_error(&self) -> bool {
        matches!(self.internal_code.as_deref(), Some("0x80041317" | "0x80045c38"))
    }
}

impl std::fmt::Display for RstFault {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", if self.reason.is_empty() { "unspecified fault" } else { &self.reason })?;
        for (name, value) in [
            ("subcode", &self.subcode),
            ("reqstatus", &self.req_status),
            ("internal", &self.internal_code),
            ("internal text", &self.internal_text),
        ] {
            if let Some(v) = value {
                write!(f, "; {name} {v}")?;
            }
        }
        Ok(())
    }
}

/// One issued `RequestSecurityTokenResponse`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IssuedToken {
    pub token_type: String,
    /// The `AppliesTo` address: the request address the token answers.
    pub applies_to: String,
    /// The token as the service issued it. For `urn:passport:legacy`, the `EncryptedData` element's
    /// verbatim source span (echoed byte-for-byte as the device token); for `urn:passport:compact` and
    /// `urn:passport:delegationcompact`, the `BinarySecurityToken` value (the bearer, without a `d=`).
    pub token: String,
    /// For `urn:passport:legacy`, the same `EncryptedData` as a standalone element (the namespaces it
    /// inherited from the response declared on it when the span alone does not parse), for embedding
    /// as the user token.
    pub legacy_xml: Option<String>,
    /// The `RequestedProofToken/BinarySecret`, when present (the device ticket's proof key).
    pub proof_secret: Option<String>,
    /// `wst:Lifetime/wsu:Expires`.
    pub expires: Option<String>,
}

/// A `psf:pp` (plaintext, or decrypted from `psf:EncryptedPP`): the request status and the consent
/// URLs when the service asks for one.
#[derive(Debug, Clone, Default)]
pub(super) struct PpInfo {
    req_status: Option<String>,
    inline_auth_url: Option<String>,
    inline_end_auth_url: Option<String>,
}

impl PpInfo {
    pub(super) fn read(pp: roxmltree::Node) -> Self {
        Self {
            req_status: descendant_text(pp, "reqstatus"),
            inline_auth_url: descendant_text(pp, "inlineauthurl"),
            inline_end_auth_url: descendant_text(pp, "inlineendauthurl"),
        }
    }
}

/// Parses an RST2 response.
///
/// The header's `psf:pp` is read in plaintext or decrypted from
/// `psf:EncryptedPP`; the body is decrypted when it is an `EncryptedData` (a signed exchange's issued
/// tokens). Both are encrypted under keys derived from `proof_secret` and the nonce of the
/// `wssc:DerivedKeyToken` each `EncryptedData` references; pass `None` for the device-ticket response,
/// which is not encrypted. The body is then a `RequestSecurityTokenResponse`, a
/// `RequestSecurityTokenResponseCollection`, or an `S:Fault`.
pub fn parse_response(text: &str, proof_secret: Option<&[u8; 32]>) -> Result<RstResponse, RstError> {
    let doc = roxmltree::Document::parse(text).map_err(|e| RstError::XmlParse(e.into()))?;
    let envelope = doc.root_element();
    if envelope.tag_name().name() != "Envelope" {
        return Err(RstError::Unexpected(format!("root <{}>", envelope.tag_name().name())));
    }

    let pp = match child(envelope, "Header") {
        Some(header) => read_pp(&doc, header, proof_secret)?,
        None => None,
    };

    let body = child(envelope, "Body").ok_or(RstError::Missing("S:Body"))?;
    let content = body.children().find(|n| n.is_element()).ok_or(RstError::Missing("S:Body content"))?;

    if content.tag_name().name() == "EncryptedData" {
        let plain = decrypt_encrypted_data(&doc, content, proof_secret)?;
        let wrapped = wrap_fragment(&plain, body);
        let frag = roxmltree::Document::parse(&wrapped).map_err(|e| RstError::Xml(format!("decrypted body: {e}")))?;
        let inner = frag
            .root_element()
            .children()
            .find(|n| n.is_element())
            .ok_or(RstError::Missing("decrypted S:Body content"))?;
        return classify(&wrapped, inner, pp);
    }

    classify(text, content, pp)
}

/// The header's `psf:pp`, plaintext or decrypted from `psf:EncryptedPP`.
pub(super) fn read_pp(
    doc: &roxmltree::Document,
    header: roxmltree::Node,
    proof_secret: Option<&[u8; 32]>,
) -> Result<Option<PpInfo>, RstError> {
    if let Some(pp) = child(header, "pp") {
        return Ok(Some(PpInfo::read(pp)));
    }

    let Some(enc_pp) = child(header, "EncryptedPP") else {
        return Ok(None);
    };

    let enc = child(enc_pp, "EncryptedData").ok_or(RstError::Missing("psf:EncryptedPP/EncryptedData"))?;
    let plain = decrypt_encrypted_data(doc, enc, proof_secret)?;
    let wrapped = wrap_fragment(&plain, enc_pp);
    let frag = roxmltree::Document::parse(&wrapped).map_err(|e| RstError::Xml(format!("decrypted psf:pp: {e}")))?;
    let pp = frag
        .root_element()
        .descendants()
        .find(|n| n.is_element() && n.tag_name().name() == "pp")
        .ok_or(RstError::Missing("decrypted psf:pp"))?;
    Ok(Some(PpInfo::read(pp)))
}

/// Classifies the (decrypted) body content. `source` is the text `content` was parsed from.
pub(super) fn classify(source: &str, content: roxmltree::Node, pp: Option<PpInfo>) -> Result<RstResponse, RstError> {
    let pp = pp.unwrap_or_default();
    match content.tag_name().name() {
        "Fault" => {
            if let Some(url) = pp.inline_auth_url.clone().or_else(|| descendant_text(content, "inlineauthurl")) {
                let end_url = pp.inline_end_auth_url.clone().or_else(|| descendant_text(content, "inlineendauthurl"));
                return Ok(RstResponse::Consent { inline_auth_url: url, end_url });
            }
            Ok(RstResponse::Fault(read_fault(content, &pp)))
        }
        "RequestSecurityTokenResponse" => issued(source, vec![content]),
        "RequestSecurityTokenResponseCollection" => issued(
            source,
            content
                .children()
                .filter(|n| n.is_element() && n.tag_name().name() == "RequestSecurityTokenResponse")
                .collect(),
        ),
        other => Err(RstError::Unexpected(format!("body <{other}>"))),
    }
}

/// The tokens of the responses. A response without a `RequestedSecurityToken` is a per-token refusal:
/// one carrying an inline auth URL is a consent (the whole exchange is retried after it), any other
/// fails the exchange with its status.
pub(super) fn issued(source: &str, rstrs: Vec<roxmltree::Node>) -> Result<RstResponse, RstError> {
    if rstrs.is_empty() {
        return Err(RstError::Missing("RequestSecurityTokenResponse"));
    }

    let mut tokens = Vec::new();
    let mut refused = None;

    for rstr in rstrs {
        if child(rstr, "RequestedSecurityToken").is_some() {
            tokens.push(parse_issued(source, rstr)?);
            continue;
        }

        if let Some(url) = descendant_text(rstr, "inlineauthurl") {
            let end_url = descendant_text(rstr, "inlineendauthurl");
            return Ok(RstResponse::Consent { inline_auth_url: url, end_url });
        }

        refused.get_or_insert(RstFault {
            reason: format!(
                "no token issued for {}",
                descendant_text(rstr, "Address").unwrap_or_else(|| "a requested scope".into())
            ),
            req_status: descendant_text(rstr, "reqstatus"),
            internal_code: descendant_text(rstr, "code"),
            internal_text: descendant_text(rstr, "text"),
            ..RstFault::default()
        });
    }

    match refused {
        Some(fault) => Ok(RstResponse::Fault(fault)),
        None => Ok(RstResponse::Issued(tokens)),
    }
}

/// `S:Fault`: `Reason/Text`, `Code/Subcode/Value`, and `Detail/error` (`value`, `internalerror/code`,
/// `internalerror/text`), the request status falling back to the `psf:pp`'s.
pub(super) fn read_fault(fault: roxmltree::Node, pp: &PpInfo) -> RstFault {
    let path = |names: &[&str]| -> Option<String> {
        let mut node = fault;
        for name in names {
            node = child(node, name)?;
        }
        node.text().map(|t| t.trim().to_string()).filter(|t| !t.is_empty())
    };

    RstFault {
        reason: path(&["Reason", "Text"]).unwrap_or_default(),
        subcode: path(&["Code", "Subcode", "Value"]),
        req_status: path(&["Detail", "error", "value"]).or_else(|| pp.req_status.clone()),
        internal_code: path(&["Detail", "error", "internalerror", "code"]),
        internal_text: path(&["Detail", "error", "internalerror", "text"]),
    }
}

pub(super) fn parse_issued(source: &str, rstr: roxmltree::Node) -> Result<IssuedToken, RstError> {
    let token_type = child(rstr, "TokenType")
        .and_then(|n| n.text())
        .map(|t| t.trim().to_string())
        .ok_or(RstError::Missing("TokenType"))?;
    let applies_to = child(rstr, "AppliesTo").and_then(|n| descendant_text(n, "Address")).unwrap_or_default();
    let expires =
        child(rstr, "Lifetime").and_then(|n| child(n, "Expires")).and_then(|n| n.text()).map(|t| t.trim().to_string());
    let proof_secret = child(rstr, "RequestedProofToken")
        .and_then(|n| child(n, "BinarySecret"))
        .and_then(|n| n.text())
        .map(|t| t.trim().to_string());

    let requested = child(rstr, "RequestedSecurityToken").ok_or(RstError::Missing("RequestedSecurityToken"))?;
    let (token, legacy_xml) = match token_type.as_str() {
        "urn:passport:legacy" => {
            let enc = child(requested, "EncryptedData").ok_or(RstError::Missing("legacy token EncryptedData"))?;
            // The device token must be echoed byte-for-byte as the service issued it: re-serializing it
            // makes the service reject the derived proof key. Keep the verbatim span.
            (source[enc.range()].to_string(), Some(standalone_element(source, enc)?))
        }
        "urn:passport:compact" | "urn:passport:delegationcompact" => {
            let bst = child(requested, "BinarySecurityToken").ok_or(RstError::Missing("BinarySecurityToken"))?;
            let value = bst.text().map(|t| t.trim().to_string()).filter(|t| !t.is_empty());
            (value.ok_or(RstError::Missing("BinarySecurityToken value"))?, None)
        }
        other => return Err(RstError::Unexpected(format!("token type {other}"))),
    };

    Ok(IssuedToken { token_type, applies_to, token, legacy_xml, proof_secret, expires })
}

/// `node`'s source span as a standalone element: verbatim when it parses alone, else with the
/// namespaces it inherited (and does not itself declare) declared on its start tag.
pub(super) fn standalone_element(source: &str, node: roxmltree::Node) -> Result<String, RstError> {
    let span = &source[node.range()];
    if roxmltree::Document::parse(span).is_ok() {
        return Ok(span.to_string());
    }

    let start_tag = &span[..span.find('>').ok_or(RstError::Missing("start tag end"))?];
    let name_end = span[1..]
        .find(|c: char| c.is_whitespace() || c == '>' || c == '/')
        .map(|i| i + 1)
        .ok_or(RstError::Missing("start tag name"))?;

    let mut decls = String::new();
    for ns in node.namespaces() {
        let (attr, uri) = match ns.name() {
            Some("xml") => continue,
            Some(prefix) => (format!("xmlns:{prefix}"), ns.uri()),
            None => ("xmlns".to_string(), ns.uri()),
        };
        if !start_tag.contains(&format!("{attr}=")) {
            decls.push_str(&format!(" {attr}=\"{}\"", uri.replace('&', "&amp;").replace('"', "&quot;")));
        }
    }

    let fixed = format!("{}{}{}", &span[..name_end], decls, &span[name_end..]);
    roxmltree::Document::parse(&fixed).map_err(|e| RstError::Xml(format!("standalone token: {e}")))?;
    Ok(fixed)
}

/// Wraps a decrypted fragment so it parses with the namespaces in scope where it was encrypted (the
/// plaintext may use prefixes the envelope declared).
pub(super) fn wrap_fragment(plain: &str, context: roxmltree::Node) -> String {
    let plain = plain.trim_start_matches('\u{feff}').trim_start();
    let plain = match plain.strip_prefix("<?xml") {
        Some(rest) => rest.find("?>").map_or(plain, |i| &rest[i + 2..]),
        None => plain,
    };

    let mut out = String::from("<wrapper");
    for ns in context.namespaces() {
        let uri = ns.uri().replace('&', "&amp;").replace('"', "&quot;");
        match ns.name() {
            Some("xml") => {}
            Some(prefix) => out.push_str(&format!(" xmlns:{prefix}=\"{uri}\"")),
            None => out.push_str(&format!(" xmlns=\"{uri}\"")),
        }
    }

    out.push('>');
    out.push_str(plain);
    out.push_str("</wrapper>");
    out
}

/// Decrypts an `EncryptedData` (a response body or an `EncryptedPP`): AES-256-CBC under the key derived
/// from `proof_secret` and the nonce of the `wssc:DerivedKeyToken` its `KeyInfo` references by
/// `#<id>`. A response carries two derived keys (`SignKey`, `EncKey`) with different nonces, so the key
/// is selected by that reference.
pub(super) fn decrypt_encrypted_data(
    doc: &roxmltree::Document,
    enc: roxmltree::Node,
    proof_secret: Option<&[u8; 32]>,
) -> Result<String, RstError> {
    let secret = proof_secret.ok_or(RstError::Missing("a proof secret to decrypt an EncryptedData"))?;
    let key_id = child(enc, "KeyInfo")
        .and_then(|ki| {
            ki.descendants()
                .filter(|n| n.is_element() && n.tag_name().name() == "Reference")
                .find_map(|n| n.attribute("URI").and_then(|u| u.strip_prefix('#')))
        })
        .ok_or(RstError::Missing("EncryptedData KeyInfo reference"))?;

    let token = doc
        .descendants()
        .find(|n| {
            n.is_element()
                && n.tag_name().name() == "DerivedKeyToken"
                && n.attributes().any(|a| a.name() == "Id" && a.value() == key_id)
        })
        .ok_or(RstError::Missing("the referenced DerivedKeyToken"))?;
    let nonce_b64 = child(token, "Nonce").and_then(|n| n.text()).ok_or(RstError::Missing("DerivedKeyToken Nonce"))?;
    let nonce = BASE64_STANDARD.decode(nonce_b64.trim())?;

    let cipher_b64 = child(enc, "CipherData")
        .and_then(|n| child(n, "CipherValue"))
        .and_then(|n| n.text())
        .ok_or(RstError::Missing("CipherValue"))?;
    let cipher = BASE64_STANDARD.decode(cipher_b64.trim())?;

    let derived = license::sp800_108(secret, KDF_LABEL, &nonce, 32);
    let derived: [u8; 32] = derived.try_into().expect("sp800_108 returns the requested length");
    let plain = license::decrypt_response(&cipher, &derived)?;
    String::from_utf8(plain).map_err(|e| RstError::XmlParse(e.into()))
}

/// The first child element of `node` named `name` (local name).
pub(super) fn child<'a, 'input>(node: roxmltree::Node<'a, 'input>, name: &str) -> Option<roxmltree::Node<'a, 'input>> {
    node.children().find(|n| n.is_element() && n.tag_name().name() == name)
}

/// The trimmed, non-empty text of the first descendant element named `name` (local name).
pub(super) fn descendant_text(node: roxmltree::Node, name: &str) -> Option<String> {
    node.descendants()
        .find(|n| n.is_element() && n.tag_name().name() == name)
        .and_then(|n| n.text())
        .map(|t| t.trim().to_string())
        .filter(|s| !s.is_empty())
}

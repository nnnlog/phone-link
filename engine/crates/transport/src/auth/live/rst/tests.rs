use super::*;

fn window() -> Window {
    Window { created: "2026-01-01T00:00:00Z".into(), expires: "2026-01-01T00:05:00Z".into() }
}

#[test]
fn rsa_sign_verifies() {
    use rsa::traits::PublicKeyParts;
    let key = test_key();
    let canonical = b"<SignedInfo>example</SignedInfo>";

    let sig_b64 = rsa_sign(&key, canonical).unwrap();
    let sig = BASE64_STANDARD.decode(sig_b64).unwrap();

    let hash = Sha256::digest(canonical);
    let mut digest_info = SHA256_DIGESTINFO_PREFIX.to_vec();
    digest_info.extend_from_slice(&hash);
    key.to_public_key().verify(Pkcs1v15Sign::new_unprefixed(), &digest_info, &sig).unwrap();
    assert!(key.n().bits() >= 1024);
}

#[test]
fn device_ticket_request_is_well_formed_and_signed() {
    let key = test_key();
    let xml = device_ticket_request("02abcdefghijklmn", &window(), &key).unwrap();

    // Parses as XML and carries the signed parts.
    let doc = roxmltree::Document::parse(&xml).unwrap();
    assert!(doc.descendants().any(|n| n.tag_name().name() == "SignedInfo"));
    assert!(xml.contains("rsa-sha256"));
    assert!(xml.contains("<wsse:Username>02abcdefghijklmn</wsse:Username>"));
    assert!(xml.contains("http://Passport.NET/tb"));

    // `ps:SSOFlags` is always present, empty when unset: the account service rejects the
    // request's signature without it, even though the device ticket sets no SSO flags.
    assert!(xml.contains("<ps:SSOFlags></ps:SSOFlags>"));

    // Namespaces are declared at their point of use, not all on the root: the root declares only
    // its own prefix, `ps` is declared on `AuthInfo`, and no unused prefix (e.g. `saml`) appears.
    // The account service rejects an otherwise identical request that declares them on the root.
    assert!(xml.starts_with("<s:Envelope xmlns:s=\"http://www.w3.org/2003/05/soap-envelope\"><s:Header>"));
    assert!(xml.contains(
        "<ps:AuthInfo xmlns:ps=\"http://schemas.microsoft.com/Passport/SoapServices/PPCRL\" Id=\"PPAuthInfo\">"
    ));
    assert!(!xml.contains("saml"));

    // Three references, one signature value.
    assert_eq!(xml.matches("<Reference URI=").count(), 3);
    assert_eq!(xml.matches("<SignatureValue>").count(), 1);
}

const USER_TOKEN: &str = r#"<EncryptedData xmlns="http://www.w3.org/2001/04/xmlenc#" Id="BinaryDAToken0" Type="http://www.w3.org/2001/04/xmlenc#Element"><EncryptionMethod Algorithm="http://www.w3.org/2001/04/xmlenc#tripledes-cbc"></EncryptionMethod><ds:KeyInfo xmlns:ds="http://www.w3.org/2000/09/xmldsig#"><ds:KeyName>http://Passport.NET/STS</ds:KeyName></ds:KeyInfo><CipherData><CipherValue>QQ==</CipherValue></CipherData></EncryptedData>"#;

fn exchange(scopes: &[ScopeRequest], inline_ux: &str, inline_ft: Option<&str>) -> String {
    exchange_request(
        "user@example.com",
        "DEVICEDATOKEN",
        USER_TOKEN,
        "app-id",
        inline_ux,
        inline_ft,
        scopes,
        &window(),
        &[3u8; 32],
        &[5u8; 32],
    )
    .unwrap()
}

fn scope(address: &str, policy: bool) -> ScopeRequest {
    ScopeRequest { address: address.into(), token_broker_policy: policy }
}

#[test]
fn exchange_request_multi_scope_wraps_and_signs() {
    let xml = exchange(
        &[scope("scope=a&api-version=2.0&clientid=x", true), scope("http://Passport.NET/tb", false)],
        "TokenBroker",
        Some("inline-ft"),
    );

    let doc = roxmltree::Document::parse(&xml).unwrap();
    assert!(doc.descendants().any(|n| n.tag_name().name() == "RequestMultipleSecurityTokens"));
    assert!(xml.contains("hmac-sha256"));
    assert!(xml.contains("<Reference URI=\"#RSTS\">"));
    assert!(xml.contains("#SignKey"));
    assert!(xml.contains("<ps:InlineUX>TokenBroker</ps:InlineUX>"));
    assert!(xml.contains("<ps:InlineFT>inline-ft</ps:InlineFT>"));
    assert!(xml.contains("<ps:SSOFlags>SsoRestr</ps:SSOFlags>"));
    assert!(!xml.contains("LicenseSignatureKeyVersion"));

    // Only the scope request carries the TOKEN_BROKER policy.
    assert_eq!(xml.matches("TOKEN_BROKER").count(), 1);
}

#[test]
fn a_silent_exchange_carries_no_inline_flow_token() {
    let xml = exchange(&[scope("scope=a&api-version=2.0&clientid=x", true)], "Silent", None);

    assert!(xml.contains("<ps:InlineUX>Silent</ps:InlineUX>"));
    assert!(!xml.contains("InlineFT"));
    assert!(xml.contains("<Reference URI=\"#RST0\">"));
}

#[test]
fn the_user_token_is_embedded_verbatim_before_the_device_token() {
    let xml = exchange(&[scope("scope=a&api-version=2.0&clientid=x", true)], "Silent", None);

    // Byte-for-byte as issued (no namespace copied onto it, attributes in their own order).
    let at = xml.find(USER_TOKEN).expect("user token verbatim");

    // The token broker's `wsse:Security` order: UsernameToken, user token, device token, derived
    // key, timestamp, signature.
    let order = [
        "<wsse:UsernameToken",
        USER_TOKEN,
        "<wsse:BinarySecurityToken",
        "<wssc:DerivedKeyToken",
        "<wsu:Timestamp",
        "<Signature",
    ]
    .map(|needle| xml.find(needle).unwrap());

    assert!(order.windows(2).all(|w| w[0] < w[1]), "{order:?}");
    assert_eq!(order[1], at);
    assert!(xml.contains(r#"<wsse:BinarySecurityToken id="DeviceDAToken" ValueType="urn:liveid:device">DEVICEDATOKEN</wsse:BinarySecurityToken>"#));
}

#[test]
fn a_user_token_that_is_not_an_encrypted_data_element_is_refused() {
    let err = exchange_request(
        "u",
        "d",
        "<Foo/>",
        "app",
        "Silent",
        None,
        &[scope("s", true)],
        &window(),
        &[0u8; 32],
        &[0u8; 32],
    )
    .unwrap_err();

    assert!(matches!(err, RstError::Unexpected(_)), "{err:?}");
}

#[test]
fn parses_the_device_ticket_response() {
    // The plaintext device-ticket answer: an informational `psf:pp` in the header, the ticket in
    // the body (the layout of the live 2026-10-25 response).
    let body = r#"<?xml version="1.0" encoding="utf-8" ?><S:Envelope xmlns:S="http://www.w3.org/2003/05/soap-envelope" xmlns:wsse="http://docs.oasis-open.org/wss/2004/01/oasis-200401-wss-wssecurity-secext-1.0.xsd" xmlns:wsu="http://docs.oasis-open.org/wss/2004/01/oasis-200401-wss-wssecurity-utility-1.0.xsd" xmlns:wst="http://schemas.xmlsoap.org/ws/2005/02/trust" xmlns:psf="http://schemas.microsoft.com/Passport/SoapServices/SOAPFault"><S:Header><psf:pp><psf:PUID>0018400000000000</psf:PUID><psf:reqstatus>0x0</psf:reqstatus></psf:pp></S:Header><S:Body><wst:RequestSecurityTokenResponse xmlns:wsp="http://schemas.xmlsoap.org/ws/2004/09/policy" xmlns:wsa="http://www.w3.org/2005/08/addressing"><wst:TokenType>urn:passport:legacy</wst:TokenType><wsp:AppliesTo><wsa:EndpointReference><wsa:Address>http://Passport.NET/tb</wsa:Address></wsa:EndpointReference></wsp:AppliesTo><wst:Lifetime><wsu:Created>2026-01-01T00:00:00Z</wsu:Created><wsu:Expires>2026-04-01T00:00:00Z</wsu:Expires></wst:Lifetime><wst:RequestedSecurityToken><EncryptedData Id="devicesoftware" xmlns="http://www.w3.org/2001/04/xmlenc#"><CipherData><CipherValue>QUJD</CipherValue></CipherData></EncryptedData></wst:RequestedSecurityToken><wst:RequestedProofToken><wst:BinarySecret>UFJPT0Y=</wst:BinarySecret></wst:RequestedProofToken></wst:RequestSecurityTokenResponse></S:Body></S:Envelope>"#;

    let RstResponse::Issued(tokens) = parse_response(body, None).unwrap() else {
        panic!("expected issued");
    };

    assert_eq!(tokens.len(), 1);
    let t = &tokens[0];
    assert_eq!(t.token_type, "urn:passport:legacy");
    assert_eq!(t.applies_to, "http://Passport.NET/tb");
    assert_eq!(t.expires.as_deref(), Some("2026-04-01T00:00:00Z"));
    assert_eq!(t.proof_secret.as_deref(), Some("UFJPT0Y="));

    // Verbatim span (it stands alone, so the standalone form is identical).
    let span = r#"<EncryptedData Id="devicesoftware" xmlns="http://www.w3.org/2001/04/xmlenc#"><CipherData><CipherValue>QUJD</CipherValue></CipherData></EncryptedData>"#;
    assert_eq!(t.token, span);
    assert_eq!(t.legacy_xml.as_deref(), Some(span));
}

#[test]
fn parses_a_plaintext_consent_fault() {
    let body = r#"<S:Envelope xmlns:S="http://www.w3.org/2003/05/soap-envelope"><S:Header><psf:pp xmlns:psf="http://schemas.microsoft.com/Passport/SoapServices/SOAPFault"><psf:reqstatus>0x800488bd</psf:reqstatus><psf:inlineauthurl>https://login.live.com/ppsecure/InlineClientAuth.srf?a=1&amp;b=2</psf:inlineauthurl><psf:inlineendauthurl>https://login.live.com/ppsecure/InlineDesktop.srf</psf:inlineendauthurl></psf:pp></S:Header><S:Body><S:Fault><S:Reason><S:Text>Platform SSO permission required</S:Text></S:Reason></S:Fault></S:Body></S:Envelope>"#;

    assert_eq!(
        parse_response(body, None).unwrap(),
        RstResponse::Consent {
            inline_auth_url: "https://login.live.com/ppsecure/InlineClientAuth.srf?a=1&b=2".into(),
            end_url: Some("https://login.live.com/ppsecure/InlineDesktop.srf".into()),
        }
    );
}

#[test]
fn a_fault_without_a_consent_is_a_fault_with_its_codes() {
    let body = r#"<S:Envelope xmlns:S="http://www.w3.org/2003/05/soap-envelope" xmlns:wst="http://schemas.xmlsoap.org/ws/2005/02/trust" xmlns:psf="http://schemas.microsoft.com/Passport/SoapServices/SOAPFault"><S:Body><S:Fault><S:Code><S:Value>S:Sender</S:Value><S:Subcode><S:Value>wst:InvalidRequest</S:Value></S:Subcode></S:Code><S:Reason><S:Text xml:lang="en-US">Invalid Request</S:Text></S:Reason><S:Detail><psf:error><psf:value>0x80048820</psf:value><psf:internalerror><psf:code>0x80041317</psf:code><psf:text>Invalid XML signature.</psf:text></psf:internalerror></psf:error></S:Detail></S:Fault></S:Body></S:Envelope>"#;

    let RstResponse::Fault(fault) = parse_response(body, None).unwrap() else {
        panic!("expected a fault");
    };

    assert_eq!(fault.reason, "Invalid Request");
    assert_eq!(fault.subcode.as_deref(), Some("wst:InvalidRequest"));
    assert_eq!(fault.req_status.as_deref(), Some("0x80048820"));
    assert_eq!(fault.internal_code.as_deref(), Some("0x80041317"));
    assert_eq!(fault.internal_text.as_deref(), Some("Invalid XML signature."));
    assert!(fault.is_request_error());
    assert!(fault.to_string().contains("0x80041317"));
}

#[test]
fn an_unexpected_body_is_an_error() {
    let body =
        r#"<S:Envelope xmlns:S="http://www.w3.org/2003/05/soap-envelope"><S:Body><Other/></S:Body></S:Envelope>"#;
    assert!(matches!(parse_response(body, None).unwrap_err(), RstError::Unexpected(_)));
}

// A fixed 1024-bit key so the signing test needs no key generation.
fn test_key() -> RsaPrivateKey {
    use rsa::BigUint;
    let unhex = |s: &str| -> Vec<u8> {
        (0..s.len()).step_by(2).map(|i| u8::from_str_radix(&s[i..i + 2], 16).unwrap()).collect()
    };
    let p = "d804209783b23ed9ae5a440cb9771917937e92c0539e4c7d94fbeeb996486f4f2be5e9d6f184f6418a67a3378d6f61fc43874a22dd934493fb334ba5e3d6d3cf";
    let q = "e0f8c013d9f3b47039b397f77be147456008c86c780a1fc73ba2d0db5e182d5a41ac81f11d08f016e3b7846b3ad035bc24319293688872019b478d17ef623d2b";
    RsaPrivateKey::from_p_q(
        BigUint::from_bytes_be(&unhex(p)),
        BigUint::from_bytes_be(&unhex(q)),
        BigUint::from(65537u32),
    )
    .unwrap()
}

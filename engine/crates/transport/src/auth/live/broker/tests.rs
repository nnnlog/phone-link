use super::*;
use phonelink_core::edge::testing::MemoryStore;
use phonelink_core::edge::{SystemClock, WebSignInError};

/// Replays queued `rst2` answers and records every request body.
struct FakeTransport {
    rst2_replies: Mutex<Vec<String>>,
    sent: Mutex<Vec<String>>,
}
impl FakeTransport {
    fn new(replies: Vec<String>) -> Arc<Self> {
        Arc::new(Self { rst2_replies: Mutex::new(replies), sent: Mutex::new(Vec::new()) })
    }
    fn sent(&self) -> Vec<String> {
        self.sent.lock_or_panic().clone()
    }
}
#[async_trait::async_trait]
impl TokenTransport for FakeTransport {
    async fn device_add(&self, _body: String) -> Result<String, TransportError> {
        Err(TransportError::Other("device_add not expected".into()))
    }
    async fn rst2(&self, body: String) -> Result<String, TransportError> {
        self.sent.lock_or_panic().push(body);
        let mut r = self.rst2_replies.lock_or_panic();
        if r.is_empty() {
            return Err(TransportError::Other("no more rst2 replies".into()));
        }
        Ok(r.remove(0))
    }
}

/// Replays queued web-view outcomes and records each request's kind.
struct FakeWeb {
    replies: Mutex<Vec<Result<String, WebSignInError>>>,
    kinds: Mutex<Vec<WebSignInKind>>,
}
impl FakeWeb {
    fn new(replies: Vec<Result<String, WebSignInError>>) -> Arc<Self> {
        Arc::new(Self { replies: Mutex::new(replies), kinds: Mutex::new(Vec::new()) })
    }
    fn kinds(&self) -> Vec<WebSignInKind> {
        self.kinds.lock_or_panic().clone()
    }
}
#[async_trait::async_trait]
impl WebSignIn for FakeWeb {
    async fn present(&self, request: WebSignInRequest) -> Result<String, WebSignInError> {
        self.kinds.lock_or_panic().push(request.kind);
        self.replies.lock_or_panic().remove(0)
    }
}

const ADDRESS: &str = "scope=some-scope&api-version=2.0&clientid=client-x";
const MASTER: &str = r#"<EncryptedData xmlns="http://www.w3.org/2001/04/xmlenc#" Id="BinaryDAToken1"><CipherData><CipherValue>TUFTVEVS</CipherValue></CipherData></EncryptedData>"#;

fn rstr(token_type: &str, address: &str, token: &str) -> String {
    let requested = if token_type == "urn:passport:legacy" {
        token.to_string()
    } else {
        format!("<wsse:BinarySecurityToken>{token}</wsse:BinarySecurityToken>")
    };
    format!(
        "<wst:RequestSecurityTokenResponse><wst:TokenType>{token_type}</wst:TokenType><wsp:AppliesTo><wsa:EndpointReference><wsa:Address>{}</wsa:Address></wsa:EndpointReference></wsp:AppliesTo><wst:Lifetime><wsu:Expires>2099-01-01T00:00:00Z</wsu:Expires></wst:Lifetime><wst:RequestedSecurityToken>{requested}</wst:RequestedSecurityToken></wst:RequestSecurityTokenResponse>",
        address.replace('&', "&amp;")
    )
}

fn envelope(header: &str, body: &str) -> String {
    format!(
        r#"<S:Envelope xmlns:S="http://www.w3.org/2003/05/soap-envelope" xmlns:wst="http://schemas.xmlsoap.org/ws/2005/02/trust" xmlns:wsa="http://www.w3.org/2005/08/addressing" xmlns:wsp="http://schemas.xmlsoap.org/ws/2004/09/policy" xmlns:wsse="http://docs.oasis-open.org/wss/2004/01/oasis-200401-wss-wssecurity-secext-1.0.xsd" xmlns:wsu="http://docs.oasis-open.org/wss/2004/01/oasis-200401-wss-wssecurity-utility-1.0.xsd" xmlns:psf="http://schemas.microsoft.com/Passport/SoapServices/SOAPFault"><S:Header>{header}</S:Header><S:Body>{body}</S:Body></S:Envelope>"#
    )
}

fn issued(token: &str) -> String {
    envelope("", &rstr("urn:passport:delegationcompact", ADDRESS, token))
}

fn issued_with_master(token: &str) -> String {
    envelope(
        "",
        &format!(
            "<wst:RequestSecurityTokenResponseCollection>{}{}</wst:RequestSecurityTokenResponseCollection>",
            rstr("urn:passport:delegationcompact", ADDRESS, token),
            rstr("urn:passport:legacy", PASSPORT_TB, MASTER)
        ),
    )
}

fn consent() -> String {
    envelope(
        "<psf:pp><psf:reqstatus>0x800488bd</psf:reqstatus><psf:inlineauthurl>https://login.live.com/ppsecure/InlineClientAuth.srf?x=1</psf:inlineauthurl><psf:inlineendauthurl>https://login.live.com/ppsecure/InlineDesktop.srf</psf:inlineendauthurl></psf:pp>",
        "<S:Fault><S:Reason><S:Text>Platform SSO permission required</S:Text></S:Reason></S:Fault>",
    )
}

fn fault(internal: &str) -> String {
    envelope(
        "",
        &format!(
            "<S:Fault><S:Reason><S:Text>refused</S:Text></S:Reason><S:Detail><psf:error><psf:value>0x80048821</psf:value><psf:internalerror><psf:code>{internal}</psf:code></psf:internalerror></psf:error></S:Detail></S:Fault>"
        ),
    )
}

fn da_property_json(puid: &str, ft: &str) -> String {
    format!(
        r#"{{"sDAToken":"<EncryptedData xmlns=\"x\" Id=\"page\"></EncryptedData>","sSTSInlineFlowToken":"{ft}","sSigninName":"me@example.com","sDAExpires":"2099-01-01T00:00:00Z","K":"{puid}"}}"#
    )
}

/// A stored session (from an earlier run): no inline flow token, so exchanges are silent.
fn seed_session(store: &MemoryStore, puid: &str) {
    store.store(USER_KEY.into(), da_property_json(puid, "stale-ft").into_bytes()).unwrap();
}

fn seed(store: &MemoryStore) {
    // A device credential and a ticket, so a token() call only exercises the exchange.
    let cred = DeviceCredential {
        membername: "02abc".into(),
        password: "pw".into(),
        puid: "devpuid".into(),
        hw_device_id: "hw".into(),
        device_id: "dev".into(),
        sp_license_block: "QQ==".into(),
    };
    store.store(DEVICE_KEY.into(), serde_json::to_vec(&cred).unwrap()).unwrap();

    let ticket = DeviceTicket {
        da_token: "<EncryptedData xmlns=\"x\"></EncryptedData>".into(),
        proof_secret_b64: BASE64_STANDARD.encode([7u8; 32]),
        expires_secs: Some(4102444800),
    };
    store.store(TICKET_KEY.into(), serde_json::to_vec(&ticket).unwrap()).unwrap();
}

fn broker(transport: Arc<FakeTransport>, web: Arc<FakeWeb>, store: Arc<MemoryStore>) -> LiveBroker {
    LiveBroker::new(transport, store, Arc::new(SystemClock), web, "pl-app-id".into())
}

fn stored_session(store: &MemoryStore) -> UserSession {
    serde_json::from_slice(&store.load(USER_KEY.into()).unwrap().unwrap()).unwrap()
}

#[tokio::test]
async fn a_stored_session_exchanges_silently_and_caches() {
    let store = Arc::new(MemoryStore::new());
    seed(&store);
    seed_session(&store, "PUID1");
    let transport = FakeTransport::new(vec![issued("TOKENVALUE")]);
    let b = broker(transport.clone(), FakeWeb::new(vec![]), store);

    assert_eq!(b.token("some-scope", "client-x").await.unwrap(), "TOKENVALUE");

    // Served from cache the second time (no more replies queued).
    assert_eq!(b.token("some-scope", "client-x").await.unwrap(), "TOKENVALUE");

    let sent = transport.sent();
    assert_eq!(sent.len(), 1);

    // Silent: no inline flow token is replayed from storage.
    assert!(sent[0].contains("<ps:InlineUX>Silent</ps:InlineUX>"));
    assert!(!sent[0].contains("InlineFT"));
    assert!(sent[0].contains(r#"<EncryptedData xmlns="x" Id="page"></EncryptedData>"#));
}

#[tokio::test]
async fn the_exchange_after_a_web_sign_in_spends_its_inline_flow_token_once() {
    let store = Arc::new(MemoryStore::new());
    seed(&store);
    let transport = FakeTransport::new(vec![issued("T1"), issued("T2")]);
    let web = FakeWeb::new(vec![Ok(da_property_json("PUIDX", "flow-1"))]);
    let b = broker(transport.clone(), web.clone(), store);

    assert_eq!(b.sign_in().await.unwrap(), "PUIDX");
    assert_eq!(b.token("some-scope", "client-x").await.unwrap(), "T1");
    assert_eq!(b.token("other-scope", "client-x").await.unwrap(), "T2");

    let sent = transport.sent();
    assert!(sent[0].contains("<ps:InlineUX>TokenBroker</ps:InlineUX>"));
    assert!(sent[0].contains("<ps:InlineFT>flow-1</ps:InlineFT>"));
    assert!(sent[1].contains("<ps:InlineUX>Silent</ps:InlineUX>"));
    assert!(!sent[1].contains("InlineFT"));
    assert_eq!(web.kinds(), vec![WebSignInKind::Login]);
}

#[tokio::test]
async fn a_consent_retries_with_the_master_scope_and_keeps_the_master_token() {
    let store = Arc::new(MemoryStore::new());
    seed(&store);
    seed_session(&store, "PUID1");
    let transport = FakeTransport::new(vec![consent(), issued_with_master("TOKENVALUE"), issued("NEXT")]);
    let web = FakeWeb::new(vec![Ok(da_property_json("PUID1", "consent-ft"))]);
    let b = broker(transport.clone(), web.clone(), store.clone());

    assert_eq!(b.token("some-scope", "client-x").await.unwrap(), "TOKENVALUE");

    let sent = transport.sent();
    assert!(sent[0].contains("<ps:InlineUX>Silent</ps:InlineUX>"));

    // The retry: the consent page's flow token, the scope plus Passport.NET/tb.
    assert!(sent[1].contains("<ps:InlineUX>TokenBroker</ps:InlineUX>"));
    assert!(sent[1].contains("<ps:InlineFT>consent-ft</ps:InlineFT>"));
    assert!(sent[1].contains("<wsa:Address>http://Passport.NET/tb</wsa:Address>"));
    assert!(sent[1].contains("#RSTS"));
    assert_eq!(web.kinds(), vec![WebSignInKind::Consent]);

    // The master token is now the session's user token, and the next exchange is silent with it.
    let session = stored_session(&store);
    assert_eq!(session.user_token, MASTER);
    assert_eq!(session.expires, "2099-01-01T00:00:00Z");

    assert_eq!(b.token("other-scope", "client-x").await.unwrap(), "NEXT");
    let sent = transport.sent();
    assert!(sent[2].contains(MASTER));
    assert!(sent[2].contains("<ps:InlineUX>Silent</ps:InlineUX>"));
}

#[tokio::test]
async fn a_consent_page_that_ends_without_a_result_keeps_the_session() {
    let store = Arc::new(MemoryStore::new());
    seed(&store);
    seed_session(&store, "PUID1");
    let transport = FakeTransport::new(vec![consent(), issued_with_master("TOKENVALUE")]);
    let web = FakeWeb::new(vec![Ok("{}".into())]);
    let b = broker(transport.clone(), web, store);

    assert_eq!(b.token("some-scope", "client-x").await.unwrap(), "TOKENVALUE");

    let sent = transport.sent();
    assert!(sent[1].contains("<ps:InlineUX>Silent</ps:InlineUX>"));
    assert!(sent[1].contains("<wsa:Address>http://Passport.NET/tb</wsa:Address>"));
}

#[tokio::test]
async fn a_second_consent_fails() {
    let store = Arc::new(MemoryStore::new());
    seed(&store);
    seed_session(&store, "PUID1");
    let transport = FakeTransport::new(vec![consent(), consent()]);
    let web = FakeWeb::new(vec![Ok(da_property_json("PUID1", "ft"))]);
    let b = broker(transport, web, store);

    assert!(matches!(b.token("some-scope", "client-x").await.unwrap_err(), BrokerError::Exchange(_)));
}

#[tokio::test]
async fn a_refused_silent_exchange_signs_in_again_and_retries_interactively() {
    let store = Arc::new(MemoryStore::new());
    seed(&store);
    seed_session(&store, "PUID1");
    let transport = FakeTransport::new(vec![fault("0x80048823"), issued("TOKENVALUE")]);
    let web = FakeWeb::new(vec![Ok(da_property_json("PUID1", "fresh-ft"))]);
    let b = broker(transport.clone(), web.clone(), store);

    assert_eq!(b.token("some-scope", "client-x").await.unwrap(), "TOKENVALUE");
    assert_eq!(web.kinds(), vec![WebSignInKind::Login]);
    assert!(transport.sent()[1].contains("<ps:InlineFT>fresh-ft</ps:InlineFT>"));
}

#[tokio::test]
async fn a_request_error_fails_without_a_sign_in() {
    let store = Arc::new(MemoryStore::new());
    seed(&store);
    seed_session(&store, "PUID1");
    let transport = FakeTransport::new(vec![fault("0x80045c38")]);
    let web = FakeWeb::new(vec![]);
    let b = broker(transport, web.clone(), store);

    let err = b.token("some-scope", "client-x").await.unwrap_err();
    assert!(matches!(err, BrokerError::Exchange(ref m) if m.contains("0x80045c38")), "{err:?}");
    assert!(web.kinds().is_empty());
}

#[tokio::test]
async fn a_refused_interactive_exchange_fails() {
    let store = Arc::new(MemoryStore::new());
    seed(&store);
    let transport = FakeTransport::new(vec![fault("0x80048823")]);
    let web = FakeWeb::new(vec![Ok(da_property_json("PUID1", "ft"))]);
    let b = broker(transport, web.clone(), store);

    assert!(matches!(b.token("some-scope", "client-x").await.unwrap_err(), BrokerError::Exchange(_)));
    assert_eq!(web.kinds(), vec![WebSignInKind::Login]);
}

#[tokio::test]
async fn another_account_signing_in_mid_exchange_ends_the_session() {
    let store = Arc::new(MemoryStore::new());
    seed(&store);
    seed_session(&store, "PUID1");
    let transport = FakeTransport::new(vec![fault("0x80048823")]);
    let web = FakeWeb::new(vec![Ok(da_property_json("OTHER", "ft"))]);
    let b = broker(transport, web, store.clone());

    assert!(matches!(b.token("some-scope", "client-x").await.unwrap_err(), BrokerError::InteractionRequired(_)));
    assert!(store.load(USER_KEY.into()).unwrap().is_none());
}

#[tokio::test]
async fn sign_in_runs_web_when_no_session() {
    let store = Arc::new(MemoryStore::new());
    seed(&store);
    let web = FakeWeb::new(vec![Ok(da_property_json("PUIDX", "ft"))]);
    let b = broker(FakeTransport::new(vec![]), web, store);

    assert_eq!(b.sign_in().await.unwrap(), "PUIDX");
}

#[tokio::test]
async fn declined_web_sign_in_is_declined() {
    let store = Arc::new(MemoryStore::new());
    seed(&store);
    let web = FakeWeb::new(vec![Err(WebSignInError::Declined)]);
    let b = broker(FakeTransport::new(vec![]), web, store);

    assert!(matches!(b.sign_in().await.unwrap_err(), BrokerError::Declined));
}

#[test]
fn rfc3339_roundtrips() {
    assert_eq!(format_rfc3339(1_767_225_600), "2026-01-01T00:00:00Z");
    assert_eq!(parse_rfc3339_secs("2026-01-01T00:00:00Z"), Some(1_767_225_600));
    assert_eq!(parse_rfc3339_secs("2026-01-01T01:00:00+01:00"), Some(1_767_225_600));
    for unknown in ["nope", "", "2026-15-40T00:00:00Z", "2026-01-01T00:00:00", "1969-12-31T23:59:59Z"] {
        assert_eq!(parse_rfc3339_secs(unknown), None, "{unknown:?}");
    }
}

//! A fake Microsoft account service behind the device-bound broker's [`TokenTransport`]: it reads
//! every `RST2.srf` token exchange (the client id, the requested scope addresses, `InlineUX` and the
//! inline flow token), records it, and answers with a numbered token per exchange (`msa-at-1`,
//! `msa-at-2`, …) — the master `http://Passport.NET/tb` token too when the exchange asks for it — or
//! with the next scripted answer: a consent the user must give, or a refusal.
//!
//! The harness seeds the machine's device credential and ticket, so a device registration
//! (`deviceaddcredential.srf`) or a device-ticket request reaching the fake is a broken test setup
//! and fails loudly; the broker's own unit tests cover those steps.

use std::collections::VecDeque;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use phonelink_engine::edge::standins::FixedReachability;
use phonelink_engine::edge::testing::ManualClock;
use phonelink_engine::edge::{Clock, Reachability, TokenTransport, TransportError};
use phonelink_engine::time::{civil_from_days, unix_secs};

/// The scope the consent retry adds, which issues the master token.
pub const PASSPORT_TB: &str = "http://Passport.NET/tb";

/// The `HostingApp` of a device-ticket request (the device's own token broker), not an exchange's.
const DEVICE_HOSTING_APP: &str = "{DF60E2DF-88AD-4526-AE21-83D130EF0F68}";

/// The consent page a scripted consent sends the web view to.
pub const CONSENT_URL: &str = "https://login.live.com/ppsecure/InlineClientAuth.srf?fake=consent";

/// What the fake answers an exchange with instead of issuing tokens.
#[derive(Debug, Clone)]
pub enum Reply {
    /// The `(scope, client id)` needs a one-time consent (`psf:inlineauthurl` = [`CONSENT_URL`]).
    Consent,
    /// A refusal the broker answers by signing in again (`S:Fault`, internal code `0x80048821`).
    Refused,
    /// This body verbatim (a response the broker cannot read).
    Body(String),
}

/// One exchange the broker sent.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Exchange {
    /// `ps:HostingApp`: the identity's client id.
    pub client_id: String,
    /// The `wsa:Address` of each requested token, in request order.
    pub addresses: Vec<String>,
    /// `ps:InlineUX`: `TokenBroker` right after a sign-in or consent page, else `Silent`.
    pub inline_ux: String,
    /// `ps:InlineFT`: the inline flow token of that page.
    pub inline_ft: Option<String>,
}

impl Exchange {
    /// The `scope=` value of the first requested address.
    pub fn scope(&self) -> &str {
        let first = self.addresses.first().expect("an exchange requests a token");
        first.strip_prefix("scope=").and_then(|rest| rest.split('&').next()).unwrap_or_else(|| panic!("{first}"))
    }

    /// Whether the exchange spent a page's inline flow token.
    pub fn is_interactive(&self) -> bool {
        self.inline_ux == "TokenBroker"
    }
}

pub struct FakeMsaState {
    pub exchanges: Vec<Exchange>,
    /// Answered to the next exchanges, in order, before any is issued.
    pub script: VecDeque<Reply>,
    /// Every silent exchange is refused: the stored user token is no longer accepted (a password
    /// change, a revoked session).
    pub refuse_silent: bool,
    /// Every request fails as if the account service were unreachable.
    pub network_down: bool,
    /// The lifetime of an issued service token.
    pub token_lifetime: Duration,
    /// Service tokens issued so far.
    pub issued: u64,
}

pub struct FakeMsa {
    state: Mutex<FakeMsaState>,
    clock: Arc<ManualClock>,
    reachability: Arc<FixedReachability>,
}

impl FakeMsa {
    pub fn new(clock: Arc<ManualClock>, reachability: Arc<FixedReachability>) -> Arc<Self> {
        Arc::new(Self {
            state: Mutex::new(FakeMsaState {
                exchanges: Vec::new(),
                script: VecDeque::new(),
                refuse_silent: false,
                network_down: false,
                token_lifetime: Duration::from_secs(3600),
                issued: 0,
            }),
            clock,
            reachability,
        })
    }

    pub fn with<R>(&self, f: impl FnOnce(&mut FakeMsaState) -> R) -> R {
        f(&mut self.state.lock().unwrap())
    }

    pub fn exchanges(&self) -> Vec<Exchange> {
        self.with(|s| s.exchanges.clone())
    }

    /// The `scope=` value of every exchange so far.
    pub fn scopes(&self) -> Vec<String> {
        self.exchanges().iter().map(|e| e.scope().to_string()).collect()
    }

    /// Queues `reply` for the next exchange that has none queued before it.
    pub fn script(&self, reply: Reply) {
        self.with(|s| s.script.push_back(reply));
    }

    fn answer(&self, exchange: Exchange) -> String {
        let now = self.clock.now();
        let mut st = self.state.lock().unwrap();
        let interactive = exchange.is_interactive();
        st.exchanges.push(exchange.clone());

        if let Some(reply) = st.script.pop_front() {
            return match reply {
                Reply::Consent => consent(),
                Reply::Refused => refused(),
                Reply::Body(body) => body,
            };
        }

        if st.refuse_silent && !interactive {
            return refused();
        }

        st.issued += 1;
        let expires = rfc3339(unix_secs(now + st.token_lifetime));
        let rstrs: String = exchange
            .addresses
            .iter()
            .map(|address| {
                if address == PASSPORT_TB {
                    rstr("urn:passport:legacy", address, &expires, &master_token(st.issued))
                } else {
                    let token = format!("<wsse:BinarySecurityToken>msa-at-{}</wsse:BinarySecurityToken>", st.issued);
                    rstr("urn:passport:delegationcompact", address, &expires, &token)
                }
            })
            .collect();

        envelope(
            "",
            &format!(
                "<wst:RequestSecurityTokenResponseCollection>{rstrs}</wst:RequestSecurityTokenResponseCollection>"
            ),
        )
    }

    fn check_network(&self) -> Result<(), TransportError> {
        if self.with(|s| s.network_down) || !self.reachability.is_internet_available() {
            Err(TransportError::Network)
        } else {
            Ok(())
        }
    }
}

#[async_trait::async_trait]
impl TokenTransport for FakeMsa {
    async fn device_add(&self, _body: String) -> Result<String, TransportError> {
        panic!("the harness seeds the device credential: a device registration reached the fake account service");
    }

    async fn rst2(&self, body: String) -> Result<String, TransportError> {
        self.check_network()?;
        Ok(self.answer(read_exchange(&body)))
    }
}

/// Reads the fields the tests assert on out of an exchange request.
fn read_exchange(body: &str) -> Exchange {
    let doc = roxmltree::Document::parse(body).unwrap_or_else(|e| panic!("the exchange is not XML ({e}): {body}"));
    let text =
        |name: &str| doc.descendants().find(|n| n.has_tag_name(name)).map(|n| n.text().unwrap_or_default().to_string());

    let client_id = text("HostingApp").unwrap_or_else(|| panic!("no ps:HostingApp: {body}"));
    assert_ne!(client_id, DEVICE_HOSTING_APP, "the harness seeds the device ticket: a ticket request reached the fake");

    let addresses: Vec<String> = doc
        .descendants()
        .filter(|n| n.has_tag_name("RequestSecurityToken"))
        .map(|rst| {
            rst.descendants()
                .find(|n| n.has_tag_name("Address"))
                .and_then(|n| n.text())
                .unwrap_or_else(|| panic!("a wst:RequestSecurityToken without an address: {body}"))
                .to_string()
        })
        .collect();
    assert!(!addresses.is_empty(), "an exchange without a requested token: {body}");

    Exchange {
        client_id,
        addresses,
        inline_ux: text("InlineUX").unwrap_or_else(|| panic!("no ps:InlineUX: {body}")),
        inline_ft: text("InlineFT"),
    }
}

/// The master token issued with service token `n`, as the service wraps it (`EncryptedData`).
fn master_token(n: u64) -> String {
    format!(
        r#"<EncryptedData xmlns="http://www.w3.org/2001/04/xmlenc#" Id="BinaryDAToken1"><CipherData><CipherValue>master-{n}</CipherValue></CipherData></EncryptedData>"#
    )
}

fn rstr(token_type: &str, address: &str, expires: &str, requested: &str) -> String {
    format!(
        "<wst:RequestSecurityTokenResponse><wst:TokenType>{token_type}</wst:TokenType><wsp:AppliesTo><wsa:EndpointReference><wsa:Address>{}</wsa:Address></wsa:EndpointReference></wsp:AppliesTo><wst:Lifetime><wsu:Expires>{expires}</wsu:Expires></wst:Lifetime><wst:RequestedSecurityToken>{requested}</wst:RequestedSecurityToken></wst:RequestSecurityTokenResponse>",
        address.replace('&', "&amp;")
    )
}

fn envelope(header: &str, body: &str) -> String {
    format!(
        r#"<S:Envelope xmlns:S="http://www.w3.org/2003/05/soap-envelope" xmlns:wst="http://schemas.xmlsoap.org/ws/2005/02/trust" xmlns:wsa="http://www.w3.org/2005/08/addressing" xmlns:wsp="http://schemas.xmlsoap.org/ws/2004/09/policy" xmlns:wsse="http://docs.oasis-open.org/wss/2004/01/oasis-200401-wss-wssecurity-secext-1.0.xsd" xmlns:wsu="http://docs.oasis-open.org/wss/2004/01/oasis-200401-wss-wssecurity-utility-1.0.xsd" xmlns:psf="http://schemas.microsoft.com/Passport/SoapServices/SOAPFault"><S:Header>{header}</S:Header><S:Body>{body}</S:Body></S:Envelope>"#
    )
}

fn consent() -> String {
    envelope(
        &format!(
            "<psf:pp><psf:reqstatus>0x800488bd</psf:reqstatus><psf:inlineauthurl>{}</psf:inlineauthurl><psf:inlineendauthurl>https://login.live.com/ppsecure/InlineDesktop.srf</psf:inlineendauthurl></psf:pp>",
            CONSENT_URL.replace('&', "&amp;")
        ),
        "<S:Fault><S:Reason><S:Text>Platform SSO permission required</S:Text></S:Reason></S:Fault>",
    )
}

fn refused() -> String {
    envelope(
        "",
        "<S:Fault><S:Reason><S:Text>The user token is no longer valid</S:Text></S:Reason><S:Detail><psf:error><psf:value>0x80048821</psf:value><psf:internalerror><psf:code>0x80048821</psf:code></psf:internalerror></psf:error></S:Detail></S:Fault>",
    )
}

/// Unix seconds as `YYYY-MM-DDTHH:MM:SSZ`.
fn rfc3339(secs: i64) -> String {
    let (days, rem) = (secs.div_euclid(86_400), secs.rem_euclid(86_400));
    let (y, mo, d) = civil_from_days(days);
    format!("{y:04}-{mo:02}-{d:02}T{:02}:{:02}:{:02}Z", rem / 3600, rem % 3600 / 60, rem % 60)
}

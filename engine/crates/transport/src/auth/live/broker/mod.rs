//! The orchestrator of the device-bound sign-in.
//!
//! It covers it registers the machine as a device, keeps the device ticket and the signed-in user's
//! session, and mints a service token per `(scope, client id)` by exchanging the user and device
//! tokens, driving the web view for the interactive sign-in and for a per-client-id consent.
//!
//! Shared by both PC identities.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime};

use base64::Engine as _;
use base64::prelude::BASE64_STANDARD;
use rand::RngCore;
use serde::{Deserialize, Serialize};

use phonelink_core::edge::{Clock, SecureStore, WebSignIn, WebSignInKind, WebSignInRequest};
use phonelink_core::time::civil_from_days;

use super::consts::USER_AGENT;
use super::device::{self, DeviceCredential};
use super::license;
use super::login::{self, DaProperty};
use super::rst::{self, RstResponse, ScopeRequest, Window};
use phonelink_core::sync::MutexExt;

/// Store keys of the shared device-bound state (unprefixed: the machine's device and the account's
/// session are one for both identities).
const DEVICE_KEY: &str = "auth/live/device";
const TICKET_KEY: &str = "auth/live/device-ticket";
const USER_KEY: &str = "auth/live/user";

/// The `http://Passport.NET/tb` scope appended on a consent retry, which finalizes the grant and
/// issues the master token.
const PASSPORT_TB: &str = "http://Passport.NET/tb";

/// Where a consent page ends when its fault names no `psf:inlineendauthurl`.
const CONSENT_END_URL: &str = "https://login.live.com/ppsecure/InlineDesktop.srf";

/// How long a token whose response carries no `wst:Lifetime` is cached.
const UNDATED_TOKEN_LIFETIME: Duration = Duration::from_secs(10 * 60);

#[derive(Debug, thiserror::Error)]
pub enum BrokerError {
    #[error("no internet connectivity")]
    Internet,
    #[error("the Microsoft account sign-in needs the user: {0}")]
    InteractionRequired(String),
    #[error("the user declined the sign-in")]
    Declined,
    #[error("device registration failed: {0}")]
    Device(String),
    /// A step of the device registration failed in a library or a parser.
    #[error("device registration failed: {0}")]
    DeviceStep(#[source] Box<dyn std::error::Error + Send + Sync>),
    #[error("token exchange failed: {0}")]
    Exchange(String),
    #[error("secure store: {0}")]
    Store(String),
    /// A stored value that does not serialize or deserialize, or the store's own failure.
    #[error("secure store: {0}")]
    StoreStep(#[source] Box<dyn std::error::Error + Send + Sync>),
}

pub use phonelink_core::edge::{TokenTransport, TransportError};

/// The production transport: form-free SOAP/XML POSTs to the account service over HTTPS.
pub struct HttpTransport {
    client: reqwest::Client,
    reachability: Arc<dyn phonelink_core::edge::Reachability>,
}

impl HttpTransport {
    pub fn new(client: reqwest::Client, reachability: Arc<dyn phonelink_core::edge::Reachability>) -> Self {
        Self { client, reachability }
    }

    async fn post(&self, url: &str, body: String) -> Result<String, TransportError> {
        let result = self
            .client
            .post(url)
            .header("User-Agent", USER_AGENT)
            .header("Content-Type", "application/soap+xml")
            .header("Host", "login.live.com")
            .body(body)
            .send()
            .await;

        match result {
            Ok(resp) => {
                let status = resp.status();
                let text = resp.text().await.map_err(|e| TransportError::Other(e.to_string()))?;
                if status.is_success() {
                    Ok(text)
                } else {
                    // The account service returns its faults with a non-2xx status but a SOAP body;
                    // keep the body so the caller can read a psf:pp consent fault out of it.
                    Ok(text)
                }
            }
            Err(e) => {
                if !self.reachability.is_internet_available() || e.is_connect() || e.is_timeout() || e.is_request() {
                    Err(TransportError::Network)
                } else {
                    Err(TransportError::Other(e.to_string()))
                }
            }
        }
    }
}

#[async_trait::async_trait]
impl TokenTransport for HttpTransport {
    async fn device_add(&self, body: String) -> Result<String, TransportError> {
        self.post("https://login.live.com/ppsecure/deviceaddcredential.srf", body).await
    }
    async fn rst2(&self, body: String) -> Result<String, TransportError> {
        self.post("https://login.live.com/RST2.srf", body).await
    }
}

/// The persisted device STS ticket.
#[derive(Debug, Clone, Serialize, Deserialize)]
struct DeviceTicket {
    /// The `DeviceDAToken` (`EncryptedData` XML), embedded verbatim in every exchange.
    da_token: String,
    /// The 32-byte HMAC proof secret (base64) the exchange signature is derived from.
    proof_secret_b64: String,
    /// The ticket's `Expires`, Unix seconds; `None` when it could not be parsed (re-mint).
    expires_secs: Option<u64>,
}

/// The signed-in account's persisted session. The JSON keys are the sign-in page's `DAProperty`
/// names, since a session is first made from one.
#[derive(Debug, Clone, Serialize, Deserialize)]
struct UserSession {
    /// The account PUID (`K`).
    #[serde(rename = "K")]
    puid: String,
    #[serde(rename = "sSigninName", default)]
    sign_in_name: String,
    /// The user token every exchange carries (an `EncryptedData` element): the sign-in page's DA token,
    /// replaced by the master `http://Passport.NET/tb` token once an exchange issues one.
    #[serde(rename = "sDAToken")]
    user_token: String,
    /// The user token's expiry (RFC 3339); empty when unknown.
    #[serde(rename = "sDAExpires", default)]
    expires: String,
}

impl UserSession {
    fn from_page(prop: &DaProperty) -> Self {
        Self {
            puid: prop.puid.clone(),
            sign_in_name: prop.sign_in_name.clone(),
            user_token: prop.da_token.clone(),
            expires: prop.expires.clone(),
        }
    }
}

/// One cached service token.
#[derive(Clone)]
struct CachedToken {
    token: String,
    expires_at: SystemTime,
}

pub struct LiveBroker {
    transport: Arc<dyn TokenTransport>,
    store: Arc<dyn SecureStore>,
    clock: Arc<dyn Clock>,
    web_sign_in: Arc<dyn WebSignIn>,
    /// The client id the interactive `InlineLogin` runs under (the port's `PL` app id).
    login_client_id: String,
    /// Serializes device/ticket/session mutation.
    lock: tokio::sync::Mutex<()>,
    cache: Mutex<HashMap<String, CachedToken>>,
    /// The `sSTSInlineFlowToken` of the sign-in or consent page that produced the current session,
    /// until the next exchange spends it. It is one-shot, so it is kept in memory only.
    inline_ft: Mutex<Option<String>>,
    /// Re-mint the ticket / re-sign-in within this long of expiry.
    leeway: Duration,
}

impl LiveBroker {
    pub fn new(
        transport: Arc<dyn TokenTransport>,
        store: Arc<dyn SecureStore>,
        clock: Arc<dyn Clock>,
        web_sign_in: Arc<dyn WebSignIn>,
        login_client_id: String,
    ) -> Self {
        Self {
            transport,
            store,
            clock,
            web_sign_in,
            login_client_id,
            lock: tokio::sync::Mutex::new(()),
            cache: Mutex::new(HashMap::new()),
            inline_ft: Mutex::new(None),
            leeway: Duration::from_secs(5 * 60),
        }
    }

    /// The account PUID of the stored session, or `None` when not signed in.
    pub fn account_puid(&self) -> Result<Option<String>, BrokerError> {
        Ok(self.load_session()?.map(|s| s.puid))
    }

    /// The sign-in name (the account's e-mail address or phone number) of the stored session, or `None`
    /// when not signed in or the sign-in page reported none.
    pub fn account_sign_in_name(&self) -> Result<Option<String>, BrokerError> {
        Ok(self.load_session()?.map(|s| s.sign_in_name).filter(|n| !n.is_empty()))
    }

    /// The machine's device PUID from the stored device credential, or `None` when not registered.
    pub fn device_puid(&self) -> Result<Option<String>, BrokerError> {
        Ok(self.load::<DeviceCredential>(DEVICE_KEY)?.map(|c| c.puid))
    }

    /// A service token for `(scope, client_id)`: cached, else an exchange. The exchange right after an
    /// interactive sign-in or consent spends that page's inline flow token (`InlineUX` `TokenBroker`);
    /// any other is silent with the stored user token. A consent the service asks for runs the consent
    /// page and retries with `http://Passport.NET/tb` added, which also issues the master token; a
    /// silent exchange the service refuses runs the interactive sign-in once and retries.
    pub async fn token(&self, scope: &str, client_id: &str) -> Result<String, BrokerError> {
        let cache_key = format!("{client_id}|{scope}");

        if let Some(t) = self.cached(&cache_key) {
            return Ok(t);
        }

        let _guard = self.lock.lock().await;
        if let Some(t) = self.cached(&cache_key) {
            return Ok(t);
        }

        tracing::debug!(scope, client_id, "no cached device-bound token; ensuring the device ticket and the session");
        let ticket = self.ensure_ticket().await?;
        let session = self.ensure_session().await?;
        self.exchange_until_issued(scope, client_id, &cache_key, &ticket, session).await
    }

    /// The exchange loop of [`LiveBroker::token`], run under the broker lock: retries once after a
    /// consent and once after a silent exchange the service refused.
    async fn exchange_until_issued(
        &self,
        scope: &str,
        client_id: &str,
        cache_key: &str,
        ticket: &DeviceTicket,
        mut session: UserSession,
    ) -> Result<String, BrokerError> {
        let address = broker_scope(scope, client_id);
        let mut scopes = vec![ScopeRequest { address: address.clone(), token_broker_policy: true }];
        let mut consented = false;
        let mut signed_in_again = false;

        loop {
            let inline_ft = self.inline_ft.lock_or_panic().take();
            let interactive = inline_ft.is_some();

            match self.exchange(&session, ticket, client_id, &scopes, inline_ft.as_deref()).await? {
                RstResponse::Issued(tokens) => {
                    tracing::info!(scope, client_id, token_count = tokens.len(), "device-bound token issued");
                    self.keep_master_token(&mut session, &tokens)?;
                    return self.cache_and_pick(cache_key, &address, tokens);
                }
                RstResponse::Consent { inline_auth_url, end_url } => {
                    session = self.consent_once(&mut consented, client_id, &inline_auth_url, end_url, session).await?;
                    scopes.push(ScopeRequest { address: PASSPORT_TB.to_string(), token_broker_policy: false });
                }
                RstResponse::Fault(fault) if !interactive && !signed_in_again && !fault.is_request_error() => {
                    tracing::warn!(client_id, %fault, "silent token exchange refused; signing in again");
                    signed_in_again = true;
                    session = self.sign_in_again(&session).await?;
                }
                RstResponse::Fault(fault) => return Err(BrokerError::Exchange(fault.to_string())),
            }
        }
    }

    /// Runs the consent page the service asks for, refusing a second request for it.
    async fn consent_once(
        &self,
        consented: &mut bool,
        client_id: &str,
        inline_auth_url: &str,
        end_url: Option<String>,
        session: UserSession,
    ) -> Result<UserSession, BrokerError> {
        if *consented {
            return Err(BrokerError::Exchange("the service asked for a consent again after it was given".into()));
        }

        *consented = true;
        tracing::info!(client_id, "consent requested; opening the web view");
        self.consent(inline_auth_url, end_url, session).await
    }

    /// Ensures a signed-in user session, running the interactive sign-in when there is none, and
    /// returns the account PUID.
    pub async fn sign_in(&self) -> Result<String, BrokerError> {
        let _guard = self.lock.lock().await;
        Ok(self.ensure_session().await?.puid)
    }

    /// Forgets the user session and cached tokens (sign-out); keeps the device credential and ticket.
    pub fn forget_user(&self) -> Result<(), BrokerError> {
        self.store.delete(USER_KEY.into()).map_err(store_err)?;
        self.cache.lock_or_panic().clear();
        self.inline_ft.lock_or_panic().take();
        Ok(())
    }

    /// Forgets everything, including the machine's device credential and ticket (reset registration).
    pub fn reset(&self) -> Result<(), BrokerError> {
        for key in [USER_KEY, TICKET_KEY, DEVICE_KEY] {
            self.store.delete(key.into()).map_err(store_err)?;
        }

        self.cache.lock_or_panic().clear();
        self.inline_ft.lock_or_panic().take();
        tracing::info!("device registration reset; the device credential, ticket and user session are forgotten");
        Ok(())
    }

    /// Drops the cached token for `(scope, client_id)` (after the service rejected it).
    pub fn invalidate(&self, scope: &str, client_id: &str) {
        self.cache.lock_or_panic().remove(&format!("{client_id}|{scope}"));
    }

    fn cached(&self, key: &str) -> Option<String> {
        let now = self.clock.now();
        self.cache
            .lock_or_panic()
            .get(key)
            .filter(|t| t.expires_at.checked_sub(self.leeway).is_some_and(|e| e > now))
            .map(|t| t.token.clone())
    }

    /// Caches and returns the token issued for `address` (the request address it answers). A response
    /// that names no address (only seen from test doubles) yields its first token that is not the
    /// master token.
    fn cache_and_pick(
        &self,
        cache_key: &str,
        address: &str,
        tokens: Vec<rst::IssuedToken>,
    ) -> Result<String, BrokerError> {
        let chosen = tokens
            .iter()
            .find(|t| t.applies_to == address)
            .or_else(|| tokens.iter().find(|t| t.applies_to != PASSPORT_TB))
            .ok_or_else(|| BrokerError::Exchange(format!("no token was issued for {address}")))?;

        let now = self.clock.now();
        let expires_at = chosen
            .expires
            .as_deref()
            .and_then(parse_rfc3339_secs)
            .map(|s| SystemTime::UNIX_EPOCH + Duration::from_secs(s))
            .unwrap_or(now + UNDATED_TOKEN_LIFETIME);

        let value = chosen.token.clone();
        self.cache.lock_or_panic().insert(cache_key.to_string(), CachedToken { token: value.clone(), expires_at });
        Ok(value)
    }

    /// Makes an issued master token (`urn:passport:legacy` for `http://Passport.NET/tb`) the session's
    /// user token: later exchanges are silent with it.
    fn keep_master_token(&self, session: &mut UserSession, tokens: &[rst::IssuedToken]) -> Result<(), BrokerError> {
        let Some(master) = tokens.iter().find(|t| t.applies_to == PASSPORT_TB && t.token_type == "urn:passport:legacy")
        else {
            return Ok(());
        };

        let xml =
            master.legacy_xml.clone().ok_or_else(|| BrokerError::Exchange("the master token has no XML".into()))?;
        session.user_token = xml;
        session.expires = master.expires.clone().unwrap_or_default();

        self.save(USER_KEY, session)?;
        tracing::info!("master token stored; later exchanges are silent");
        Ok(())
    }

    async fn ensure_device(&self) -> Result<DeviceCredential, BrokerError> {
        if let Some(cred) = self.load::<DeviceCredential>(DEVICE_KEY)? {
            return Ok(cred);
        }

        tracing::debug!("registering this machine as a device (deviceaddcredential.srf)");
        let (membername, password) = device::random_credentials();
        let body = device::device_add_request(&membername, &password);

        let response = self.transport.device_add(body).await.map_err(self.transport_err())?;
        let cred = device::parse_device_add_response(&response, &membername, &password)
            .map_err(|e| BrokerError::DeviceStep(e.into()))?;

        tracing::info!(puid = %cred.puid, "device registered");
        self.save(DEVICE_KEY, &cred)?;
        Ok(cred)
    }

    async fn ensure_ticket(&self) -> Result<DeviceTicket, BrokerError> {
        if let Some(ticket) = self.load::<DeviceTicket>(TICKET_KEY)? {
            let now_secs = phonelink_core::time::unix_secs(self.clock.now()).max(0).unsigned_abs();
            let fresh = ticket.expires_secs.is_some_and(|e| e > now_secs + self.leeway.as_secs());
            if fresh {
                return Ok(ticket);
            }
        }

        let cred = self.ensure_device().await?;
        let sp_license = BASE64_STANDARD
            .decode(cred.sp_license_block.trim())
            .map_err(|e| BrokerError::Device(format!("SPLicenseBlock base64: {e}")))?;
        let sign_state = license::find_sign_state(&sp_license).map_err(|e| BrokerError::DeviceStep(e.into()))?;
        let key = license::device_rsa_key(&sign_state).map_err(|e| BrokerError::DeviceStep(e.into()))?;

        let window = self.window();
        tracing::debug!("requesting the device STS ticket (RST2.srf)");
        let body = rst::device_ticket_request(&cred.membername, &window, &key)
            .map_err(|e| BrokerError::Device(format!("building the device ticket request: {e}")))?;
        let response = self.transport.rst2(body).await.map_err(self.transport_err())?;
        let parsed = rst::parse_response(&response, None).map_err(|e| BrokerError::DeviceStep(e.into()))?;

        let tokens = match parsed {
            RstResponse::Issued(tokens) => tokens,
            RstResponse::Consent { .. } => {
                return Err(BrokerError::Device("the device ticket request was answered with a consent".into()));
            }
            RstResponse::Fault(fault) => {
                return Err(BrokerError::Device(format!("the device ticket was refused: {fault}")));
            }
        };

        let token = tokens.into_iter().next().ok_or_else(|| BrokerError::Device("no device ticket issued".into()))?;
        let proof_b64 =
            token.proof_secret.ok_or_else(|| BrokerError::Device("the ticket carried no proof token".into()))?;

        // Validate that the proof token is a well-formed CLEP block now, so a bad ticket fails here.
        let proof_block = BASE64_STANDARD
            .decode(proof_b64.trim())
            .map_err(|e| BrokerError::Device(format!("proof token base64: {e}")))?;
        let block: [u8; 4096] =
            proof_block.try_into().map_err(|_| BrokerError::Device("proof token is not 4096 bytes".into()))?;
        let secret = license::hmac_proof_secret(&block).map_err(|e| BrokerError::DeviceStep(e.into()))?;

        let ticket = DeviceTicket {
            da_token: token.token,
            proof_secret_b64: BASE64_STANDARD.encode(secret),
            expires_secs: token.expires.as_deref().and_then(parse_rfc3339_secs),
        };
        self.save(TICKET_KEY, &ticket)?;
        tracing::info!("device ticket issued");
        Ok(ticket)
    }

    /// The stored session while its user token is unexpired, else an interactive sign-in.
    async fn ensure_session(&self) -> Result<UserSession, BrokerError> {
        if let Some(session) = self.load_session()? {
            let now_secs = phonelink_core::time::unix_secs(self.clock.now()).max(0).unsigned_abs();
            // An unknown expiry is tried: a refused silent exchange falls back to the sign-in.
            let valid = parse_rfc3339_secs(&session.expires).is_none_or(|e| e > now_secs + self.leeway.as_secs());
            if valid {
                tracing::debug!("stored user session still valid");
                return Ok(session);
            }
        }

        tracing::info!("no valid user session; opening the web sign-in");
        self.interactive_sign_in().await
    }

    /// Runs the `InlineLogin` page and stores the session it yields, keeping its inline flow token for
    /// the next exchange.
    async fn interactive_sign_in(&self) -> Result<UserSession, BrokerError> {
        let json = self.web_sign_in.present(self.login_request()).await.map_err(web_err)?;
        let prop = login::parse_da_property(&json).map_err(|e| BrokerError::Exchange(e.to_string()))?;
        let session = self.adopt_page_result(&prop)?;
        tracing::debug!("web sign-in completed; user session stored");
        Ok(session)
    }

    /// After a refused silent exchange: sign in interactively. A different account signing in here
    /// would change the account under the running identities, so it ends the session instead and the
    /// next start signs in (and switches) properly.
    async fn sign_in_again(&self, current: &UserSession) -> Result<UserSession, BrokerError> {
        let session = self.interactive_sign_in().await?;

        if session.puid != current.puid {
            self.forget_user()?;
            return Err(BrokerError::InteractionRequired("a different Microsoft account signed in".into()));
        }
        Ok(session)
    }

    /// Runs a consent page until it posts a sign-in result or reaches `end_url`. A posted result
    /// becomes the session (with its inline flow token); reaching the end page without one keeps the
    /// current session.
    async fn consent(
        &self,
        url: &str,
        end_url: Option<String>,
        current: UserSession,
    ) -> Result<UserSession, BrokerError> {
        let request = WebSignInRequest {
            kind: WebSignInKind::Consent,
            url: url.to_string(),
            headers: HashMap::new(),
            user_agent: web_view_user_agent(),
            bridge_script: bridge_script(),
            end_url_prefix: Some(end_url.unwrap_or_else(|| CONSENT_END_URL.into())),
        };

        let json = self.web_sign_in.present(request).await.map_err(web_err)?;
        match login::parse_da_property(&json) {
            Ok(prop) if prop.puid != current.puid => {
                self.forget_user()?;
                Err(BrokerError::InteractionRequired("a different Microsoft account signed in".into()))
            }
            Ok(prop) => self.adopt_page_result(&prop),
            Err(login::LoginError::NoDaProperty) => {
                tracing::debug!("consent page finished without a new sign-in result; the session is kept");
                Ok(current)
            }
            Err(e) => Err(BrokerError::Exchange(e.to_string())),
        }
    }

    /// Stores the session a sign-in or consent page posted and keeps its inline flow token.
    fn adopt_page_result(&self, prop: &DaProperty) -> Result<UserSession, BrokerError> {
        let session = UserSession::from_page(prop);
        self.save(USER_KEY, &session)?;

        let ft = Some(prop.inline_flow_token.clone()).filter(|t| !t.is_empty());
        *self.inline_ft.lock_or_panic() = ft;
        Ok(session)
    }

    async fn exchange(
        &self,
        session: &UserSession,
        ticket: &DeviceTicket,
        client_id: &str,
        scopes: &[ScopeRequest],
        inline_ft: Option<&str>,
    ) -> Result<RstResponse, BrokerError> {
        let proof = BASE64_STANDARD
            .decode(ticket.proof_secret_b64.trim())
            .ok()
            .and_then(|b| <[u8; 32]>::try_from(b).ok())
            .ok_or_else(|| BrokerError::Exchange("stored proof secret is not 32 bytes".into()))?;

        let mut nonce = [0u8; 32];
        rand::rng().fill_bytes(&mut nonce);

        let inline_ux = if inline_ft.is_some() { "TokenBroker" } else { "Silent" };
        tracing::debug!(
            client_id,
            inline_ux,
            scope_count = scopes.len(),
            "exchanging for a device-bound token (RST2.srf)"
        );
        let body = rst::exchange_request(
            &session.sign_in_name,
            &ticket.da_token,
            &session.user_token,
            client_id,
            inline_ux,
            inline_ft,
            scopes,
            &self.window(),
            &proof,
            &nonce,
        )
        .map_err(|e| BrokerError::Exchange(format!("building the exchange request: {e}")))?;

        let response = self.transport.rst2(body).await.map_err(self.transport_err())?;
        rst::parse_response(&response, Some(&proof)).map_err(|e| BrokerError::Exchange(e.to_string()))
    }

    fn login_request(&self) -> WebSignInRequest {
        let url = format!(
            "https://login.live.com/ppsecure/InlineLogin.srf?id=80604&scid=3&mkt=en-US&Platform=Windows10&clientid={}&hosted=1",
            self.login_client_id
        );

        WebSignInRequest {
            kind: WebSignInKind::Login,
            url,
            headers: cxh_headers(),
            user_agent: web_view_user_agent(),
            bridge_script: bridge_script(),
            end_url_prefix: None,
        }
    }

    fn window(&self) -> Window {
        let now = phonelink_core::time::unix_secs(self.clock.now()).max(0).unsigned_abs();
        Window { created: format_rfc3339(now), expires: format_rfc3339(now + 300) }
    }

    fn transport_err(&self) -> impl Fn(TransportError) -> BrokerError + '_ {
        move |e| match e {
            TransportError::Network => BrokerError::Internet,
            other => BrokerError::Exchange(other.to_string()),
        }
    }

    fn load_session(&self) -> Result<Option<UserSession>, BrokerError> {
        self.load(USER_KEY)
    }

    fn load<T: for<'de> Deserialize<'de>>(&self, key: &str) -> Result<Option<T>, BrokerError> {
        match self.store.load(key.into()).map_err(store_err)? {
            Some(bytes) => Ok(Some(serde_json::from_slice(&bytes).map_err(|e| BrokerError::StoreStep(e.into()))?)),
            None => Ok(None),
        }
    }

    fn save<T: Serialize>(&self, key: &str, value: &T) -> Result<(), BrokerError> {
        let bytes = serde_json::to_vec(value).map_err(|e| BrokerError::StoreStep(e.into()))?;
        self.store.store(key.into(), bytes).map_err(store_err)
    }
}

fn broker_scope(scope: &str, client_id: &str) -> String {
    format!("scope={scope}&api-version=2.0&clientid={client_id}")
}

fn store_err(e: phonelink_core::edge::EdgeError) -> BrokerError {
    BrokerError::StoreStep(e.into())
}

fn web_err(e: phonelink_core::edge::WebSignInError) -> BrokerError {
    match e {
        phonelink_core::edge::WebSignInError::Declined => BrokerError::Declined,
        phonelink_core::edge::WebSignInError::Failed { reason } => BrokerError::InteractionRequired(reason),
    }
}

/// The web view's user agent (a Windows Edge/MSAppHost string), and the token-broker user agent for
/// the REST calls, are both wire constants.
fn web_view_user_agent() -> String {
    "Mozilla/5.0 (Windows NT 10.0; Win64; x64; MSAppHost/3.0) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/70.0.3538.102 Safari/537.36 Edge/18.26100"
        .to_string()
}

/// The `cxh-*` headers that identify the host as `CloudExperienceHost`.
fn cxh_headers() -> HashMap<String, String> {
    [
        (
            "cxh-capabilities",
            r#"{"PrivatePropertyBag":1,"PasswordlessConnect":1,"PreferAssociate":1,"ChromelessUI":0}"#,
        ),
        ("cxh-msaBinaryVersion", "55"),
        ("cxh-identityClientBinaryVersion", "3"),
        ("cxh-osVersionInfo", r#"{"platformId":2,"majorVersion":10,"minorVersion":0,"buildNumber":26100}"#),
        ("cxh-platform", "CloudExperienceHost.Platform.DESKTOP"),
        ("cxh-protocol", "TokenBroker"),
        ("cxh-source", "TokenBroker"),
        ("hostApp", "CloudExperienceHost"),
    ]
    .into_iter()
    .map(|(k, v)| (k.to_string(), v.to_string()))
    .chain(std::iter::once(("cxh-correlationId".to_string(), uuid::Uuid::new_v4().to_string())))
    .collect()
}

/// The document-start bridge script: it aliases `window.external.notify` onto the native message
/// handler, as `CloudExperienceHost` exposes it to the sign-in page. The host side (answering
/// `CloudExperienceHost.getContext` and pulling `ServerData` once `post.srf` loads) is driven by the
/// native web-view host, which sees every `notify` message and the page's navigations.
fn bridge_script() -> String {
    "window.external = { notify: function(m){ try { window.webkit.messageHandlers.phonelink.postMessage(m); } catch(e) {} } };"
        .to_string()
}

/// Formats Unix seconds as `YYYY-MM-DDTHH:MM:SSZ` (UTC).
fn format_rfc3339(secs: u64) -> String {
    let (days, rem) = ((secs / 86400) as i64, secs % 86400);
    let (h, m, s) = (rem / 3600, (rem % 3600) / 60, rem % 60);
    let (y, mo, d) = civil_from_days(days);
    format!("{y:04}-{mo:02}-{d:02}T{h:02}:{m:02}:{s:02}Z")
}

/// An expiry date-time (`YYYY-MM-DDTHH:MM:SS[.fraction](Z|±HH:MM)`) as Unix seconds, rounded down.
/// `None` for anything else (an invalid date or time, a missing offset, trailing text, a time before
/// the epoch); each caller says what an unknown expiry means for its token.
fn parse_rfc3339_secs(s: &str) -> Option<u64> {
    phonelink_core::time::parse_date_time(s).and_then(|ms| u64::try_from(ms.div_euclid(1000)).ok())
}

#[cfg(test)]
mod tests;

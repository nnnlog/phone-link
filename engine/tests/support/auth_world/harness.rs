//! Wires an `AuthService` to the fakes: every DCG endpoint at `127.0.0.1`, the device-bound broker
//! over a fake account service and web view (`fake_msa`, `fake_web`), a `ManualClock` at
//! 2026-10-20T00:00:00Z, a `MemoryStore` that survives "restarts" (a new `AuthService` over the
//! same store), collected events and tap events.

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use phonelink_engine::api::events::EventBridge;
use phonelink_engine::api::events::testing::CollectingEvents;
use phonelink_engine::auth::crypto::KeyPair;
use phonelink_engine::auth::live::broker::LiveBroker;
use phonelink_engine::auth::{AuthService, FetchMode, PeerDevice, Provisioned};
use phonelink_engine::config::{DcgRestConfig, EngineConfig};
use phonelink_engine::constants::PROFILE_PL;
use phonelink_engine::edge::standins::FixedReachability;
use phonelink_engine::edge::testing::{ManualClock, MemoryStore, memory_edges};
use phonelink_engine::edge::{Clock, Edges, SecureStore};
use phonelink_engine::engine::CoreEvents;
use phonelink_engine::identification::ClientIdentification;
use phonelink_engine::tap::{MemoryTap, Tap};
use phonelink_engine::types::{DeviceCertificates, DeviceInfoItem, Environment, new_guid_lower};

use super::fake_dcg::FakeDcg;
use super::fake_msa::FakeMsa;
use super::fake_web::FakeWeb;
pub use super::fake_web::{SEED_PUID, da_property_json};
use super::jwt;

/// The DCG REST policy without retries or resends, so a call's outcome is its first reply: the
/// tests of the rules above the policy script one failure per failed call. The policy itself runs in
/// `auth_rest_policy`.
pub fn single_attempt_policy() -> DcgRestConfig {
    DcgRestConfig { retries: 0, resends: 0, ..DcgRestConfig::default() }
}

/// The production policy with every wait and bound shortened for tests.
pub fn fast_policy() -> DcgRestConfig {
    DcgRestConfig {
        total_timeout: Duration::from_secs(3),
        retry_base_delay: Duration::from_millis(5),
        breaker_sampling: Duration::from_secs(30),
        breaker_break: Duration::from_millis(500),
        attempt_timeout: Some(Duration::from_secs(1)),
        resend_delay: Duration::from_millis(5),
        ..DcgRestConfig::default()
    }
}

/// Store keys of the device-bound sign-in state the harness seeds.
const DEVICE_KEY: &str = "auth/live/device";
const TICKET_KEY: &str = "auth/live/device-ticket";
/// Store key of the signed-in user session.
pub const USER_KEY: &str = "auth/live/user";

/// Seeds the machine's device credential and device ticket into `store` (a one-time registration).
fn seed_device_and_ticket(store: &MemoryStore) {
    let device = r#"{"membername":"02seed","password":"pw","puid":"devpuid","hw_device_id":"hw","device_id":"dev","sp_license_block":"QQ=="}"#;
    let ticket = format!(
        r#"{{"da_token":"<EncryptedData xmlns=\"x\"></EncryptedData>","proof_secret_b64":"{}","expires_secs":4102444800}}"#,
        jwt::b64(&[7u8; 32])
    );
    store.store(DEVICE_KEY.into(), device.as_bytes().to_vec()).unwrap();
    store.store(TICKET_KEY.into(), ticket.into_bytes()).unwrap();
}

pub struct Harness {
    pub clock: Arc<ManualClock>,
    pub store: Arc<MemoryStore>,
    pub events: Arc<CollectingEvents>,
    pub reachability: Arc<FixedReachability>,
    pub tap: Arc<MemoryTap>,
    pub dcg: FakeDcg,
    /// The Microsoft account service the broker exchanges tokens with.
    pub msa: Arc<FakeMsa>,
    /// The web view the broker shows the sign-in and consent pages in.
    pub web: Arc<FakeWeb>,
    pub config: EngineConfig,
    ident: Mutex<Option<Arc<ClientIdentification>>>,
}

/// A linked Android phone: its id, trust key pair (`CN=trust_<id>`) and device-list entry.
pub struct Phone {
    pub id: String,
    pub pair: KeyPair,
    pub item: DeviceInfoItem,
}

impl Harness {
    pub async fn new() -> Self {
        let clock = Arc::new(ManualClock::fixed());
        let dcg = FakeDcg::start(clock.clone()).await;

        let mut config = EngineConfig::default();
        config.endpoints.prod_base = dcg.base.clone();
        config.endpoints.beta_base = format!("{}beta/", dcg.base);
        config.endpoints.dogfood_base = format!("{}df/", dcg.base);
        config.endpoints.discovery_base = dcg.base.clone();
        config.auth.rest = single_attempt_policy();

        let store = Arc::new(MemoryStore::new());
        // The machine is already registered: a device credential and ticket are present, as after a
        // one-time device registration. Only the user session is interactive.
        seed_device_and_ticket(&store);

        let reachability = Arc::new(FixedReachability::online());
        let msa = FakeMsa::new(clock.clone(), reachability.clone());

        Self {
            clock,
            store,
            events: Arc::new(CollectingEvents::default()),
            reachability,
            tap: MemoryTap::new(),
            dcg,
            msa,
            web: FakeWeb::new(),
            config,
            ident: Mutex::new(None),
        }
    }

    /// A fresh broker over the shared store (a new "process": in-memory token cache empty, the
    /// device credential, ticket and session reloaded from the store).
    fn broker(&self) -> Arc<LiveBroker> {
        Arc::new(LiveBroker::new(
            self.msa.clone(),
            self.store.clone(),
            self.clock.clone(),
            self.web.clone(),
            PROFILE_PL.msa_client_id.into(),
        ))
    }

    /// The core's events, delivered to [`Harness::events`] as the UI would see them.
    pub fn core_events(&self) -> Arc<dyn CoreEvents> {
        EventBridge::new(self.events.clone())
    }

    pub fn edges(&self) -> Edges {
        Edges {
            store: self.store.clone(),
            reachability: self.reachability.clone(),
            clock: self.clock.clone(),
            web_sign_in: self.web.clone(),
            token_transport: Some(self.msa.clone()),
            ..memory_edges()
        }
    }

    /// A new `AuthService` over the shared store — one "process" (fresh `DCG-SessionId`).
    pub fn service(&self) -> Arc<AuthService> {
        let ident = Arc::new(ClientIdentification::load_or_create(&self.config, &*self.store).unwrap());
        *self.ident.lock().unwrap() = Some(ident.clone());
        let tap: Tap = self.tap.clone();
        AuthService::new(Arc::new(self.config.clone()), self.edges(), ident, tap, self.broker()).unwrap()
    }

    /// The identification headers of the latest `service()`.
    pub fn ident(&self) -> Arc<ClientIdentification> {
        self.ident.lock().unwrap().clone().expect("service() was called")
    }

    /// The user selected the phone `id`: the engine's start serves it.
    pub fn select_phone(&self, id: &str) {
        self.store.store(phonelink_engine::engine::SELECTED_PHONE_KEY.into(), id.as_bytes().to_vec()).unwrap();
    }

    /// Seeds the user session of an earlier sign-in as [`SEED_PUID`] (the device credential and
    /// ticket are seeded in `new`), so a `sign_in()` shows no web view and every exchange is silent.
    pub fn seed_session(&self) {
        self.store.store(USER_KEY.into(), da_property_json(SEED_PUID).into_bytes()).unwrap();
    }

    /// The PUID of the stored user session, if any.
    pub fn session_puid(&self) -> Option<String> {
        self.store_bytes(USER_KEY).map(|_| self.store_json(USER_KEY)["K"].as_str().unwrap().to_string())
    }

    /// Signed in silently over a seeded session as [`SEED_PUID`].
    pub async fn signed_in(&self) -> Arc<AuthService> {
        self.seed_session();
        let service = self.service();
        service.sign_in().await.unwrap();
        service
    }

    /// Signed in through the web view as the fake web view's current account. The page's inline
    /// flow token is spent by the next exchange (`InlineUX` `TokenBroker`).
    pub async fn signed_in_interactively(&self) -> Arc<AuthService> {
        let service = self.service();
        service.sign_in().await.unwrap();
        service
    }

    pub async fn provisioned(&self) -> (Arc<AuthService>, Provisioned) {
        let service = self.signed_in().await;
        let p = service.ensure_provisioned().await.unwrap();
        (service, p)
    }

    pub fn phone(&self, name: &str) -> Phone {
        let id = new_guid_lower();
        let pair = KeyPair::generate(&format!("trust_{id}"), self.clock.now()).unwrap();
        let item = DeviceInfoItem {
            id: id.clone(),
            is_linked: Some(true),
            certificates: Some(DeviceCertificates {
                ct_self_signed: None,
                self_signed: Some(vec![jwt::b64(&pair.cert.der)]),
                pki: None,
            }),
            capabilities: Some(vec!["DEVICE_STATUS".into(), "CLIPBOARD".into()]),
            client_type: Some("LTW".into()),
            client_version: Some("1.24012.100.0".into()),
            consent_version: Some(0),
            custom_data: Some(r#"{"yppCapabilities":127}"#.into()),
            name: Some(name.into()),
            os_name: Some("Android".into()),
            os_version: Some("14".into()),
            ..Default::default()
        };
        Phone { id, pair, item }
    }

    /// Provisioned, one linked phone in the device list, and a `ForceCloud` trust sync.
    pub async fn with_phone(&self) -> (Arc<AuthService>, Provisioned, Phone, Vec<PeerDevice>) {
        let (service, p) = self.provisioned().await;
        let phone = self.phone("Pixel");
        self.dcg.add_peer(phone.item.clone());
        let peers = service.sync_trust(FetchMode::ForceCloud).await.unwrap();
        (service, p, phone, peers)
    }

    pub fn store_bytes(&self, key: &str) -> Option<Vec<u8>> {
        self.store.load(key.into()).unwrap()
    }

    pub fn store_json(&self, key: &str) -> serde_json::Value {
        serde_json::from_slice(&self.store_bytes(key).unwrap_or_else(|| panic!("{key} is stored"))).unwrap()
    }

    pub fn put_store_json(&self, key: &str, value: &serde_json::Value) {
        self.store.store(key.into(), serde_json::to_vec(value).unwrap()).unwrap();
    }

    /// A key + certificate entry `{ "key": <base64 PKCS#8>, "certificate": <base64 DER> }`:
    /// `(key, certificate)`; `None` when absent.
    pub fn pair_entry(&self, key: &str) -> Option<(Vec<u8>, Vec<u8>)> {
        pair_entry_in(&self.store, key)
    }

    /// Writes a key + certificate entry in the store's format.
    pub fn put_pair_entry(&self, key: &str, private_key: &[u8], certificate: &[u8]) {
        let v = serde_json::json!({ "key": jwt::b64(private_key), "certificate": jwt::b64(certificate) });
        self.put_store_json(key, &v);
    }

    /// The persisted trust certificate of `env`.
    pub fn trust_cert(&self, env: Environment) -> Vec<u8> {
        self.pair_entry(&format!("auth/{}/trust-pair", env_key(env))).expect("trust certificate stored").1
    }

    /// The persisted identity certificate of `env`.
    pub fn identity_cert(&self, env: Environment) -> Vec<u8> {
        self.pair_entry(&format!("auth/{}/identity-pair", env_key(env))).expect("identity certificate stored").1
    }

    /// The persisted incoming identity certificate of a key rotation of `env`, if any.
    pub fn pending_identity_cert(&self, env: Environment) -> Option<Vec<u8>> {
        self.pair_entry(&format!("auth/{}/identity-pair-pending", env_key(env))).map(|(_, cert)| cert)
    }

    /// Replaces the whole store by `snapshot`.
    pub fn restore_store(&self, snapshot: &BTreeMap<String, Vec<u8>>) {
        for key in self.store.keys() {
            self.store.delete(key).unwrap();
        }
        for (key, value) in snapshot {
            self.store.store(key.clone(), value.clone()).unwrap();
        }
    }

    pub fn advance(&self, by: Duration) {
        self.clock.advance(by);
    }
}

/// The environment segment of the store keys.
pub fn env_key(env: Environment) -> &'static str {
    match env {
        Environment::Prod => "prod",
        Environment::Beta => "beta",
        Environment::Dogfood => "dogfood",
    }
}

/// A key + certificate entry of `store`: `(PKCS#8 key, certificate DER)`; `None` when absent.
pub fn pair_entry_in(store: &MemoryStore, key: &str) -> Option<(Vec<u8>, Vec<u8>)> {
    let bytes = store.load(key.into()).unwrap()?;
    let v: serde_json::Value = serde_json::from_slice(&bytes).expect("a key pair entry is JSON");
    assert_eq!(v.as_object().unwrap().len(), 2, "{key}: exactly key and certificate");
    Some((jwt::unb64(v["key"].as_str().unwrap()), jwt::unb64(v["certificate"].as_str().unwrap())))
}

/// Every entry of `store` (what a crash at this instant would leave on disk).
pub fn snapshot(store: &MemoryStore) -> BTreeMap<String, Vec<u8>> {
    store.keys().into_iter().map(|k| (k.clone(), store.load(k).unwrap().unwrap())).collect()
}

/// Polls `f` until it is true or `timeout` passes.
pub async fn eventually(timeout: Duration, mut f: impl FnMut() -> bool) -> bool {
    let deadline = tokio::time::Instant::now() + timeout;
    loop {
        if f() {
            return true;
        }
        if tokio::time::Instant::now() >= deadline {
            return false;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
}

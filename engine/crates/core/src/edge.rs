//! OS-edge interfaces: the engine declares them as `uniffi` foreign traits; `gui/` implements
//! them in Swift and the CLI with file-backed stand-ins, so the engine links no macOS-only framework.
//!
//! The traits are synchronous, and most callers, async ones included, call them directly: a Keychain
//! item, the session-lock query and the reachability flag are short local calls. The few edge calls
//! that may take long run on the blocking pool (`spawn_blocking`): the `/DeviceResourceManager`
//! wrapper's lock test and the features' PC status, pasteboard and image work.
//!
//! [`standins`] holds the stand-ins the headless CLI runs with, [`testing`] the in-memory
//! implementations the tests use.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::SystemTime;

/// Failure reported by an OS-edge implementation.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error, uniffi::Error)]
pub enum EdgeError {
    #[error("store failure: {message}")]
    Store { message: String },
    #[error("unexpected failure in the foreign implementation: {message}")]
    Unexpected { message: String },
}

impl From<uniffi::UnexpectedUniFFICallbackError> for EdgeError {
    fn from(e: uniffi::UnexpectedUniFFICallbackError) -> Self {
        EdgeError::Unexpected { message: e.reason }
    }
}

/// Persistent private storage for the identity and enrollment state that must survive a restart, and
/// for the App layer's per-install and per-phone records.
///
/// macOS: Keychain; CLI: one `0600` file per key. Keys are `/`-separated names owned by the layer
/// that writes them (`auth/…`, `app/…`).
#[uniffi::export(with_foreign)]
pub trait SecureStore: Send + Sync {
    /// The value stored under `key`; `None` when there is none.
    fn load(&self, key: String) -> Result<Option<Vec<u8>>, EdgeError>;
    /// Stores `value` under `key`, replacing what was there.
    fn store(&self, key: String, value: Vec<u8>) -> Result<(), EdgeError>;
    /// Deletes the value under `key`; deleting a missing key succeeds.
    fn delete(&self, key: String) -> Result<(), EdgeError>;
    /// Every stored key that starts with `prefix`, in any order (the sign-out deletes the App
    /// layer's per-phone records, whose phones it cannot otherwise name).
    fn keys_with_prefix(&self, prefix: String) -> Result<Vec<String>, EdgeError>;
}

/// The PC session lock state (macOS: `CGSessionCopyCurrentDictionary()`; *unknown* counts as locked
/// wherever a rule gates on the lock).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, uniffi::Enum)]
pub enum LockState {
    Unlocked,
    Locked,
    Unknown,
}

/// Whether the Mac's user session is locked.
#[uniffi::export(with_foreign)]
pub trait SessionLock: Send + Sync {
    /// The session's lock state now.
    fn lock_state(&self) -> LockState;
}

/// Host facts read from the operating system.
#[uniffi::export(with_foreign)]
pub trait HostInfo: Send + Sync {
    /// The host name; `metadata.displayName` is the part before its first `.`.
    fn host_name(&self) -> String;
    /// The PC's region as an ISO 3166-1 alpha-2 code (macOS: `Locale.current.region`), when known:
    /// the last fallback of the phone's country (local contact lookup by address; message address
    /// identity).
    fn region_code(&self) -> Option<String>;
}

/// Internet reachability (macOS: `NWPathMonitor`), used where a failure maps to an *internet error*
/// only while the device has no connectivity.
#[uniffi::export(with_foreign)]
pub trait Reachability: Send + Sync {
    /// Whether the Mac has an internet connection now.
    fn is_internet_available(&self) -> bool;
}

/// Why a request to the Microsoft account service got no usable answer.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum TransportError {
    #[error("no HTTP response (network)")]
    Network,
    #[error("HTTP {0}")]
    Http(u16),
    #[error("{0}")]
    Other(String),
}

/// The HTTP seam to the Microsoft account service (`login.live.com`) the device-bound sign-in uses,
/// so the broker's orchestration is testable without a network.
///
/// The production implementation is in `auth::live::broker`; tests inject a fake through
/// [`Edges::token_transport`].
#[async_trait::async_trait]
pub trait TokenTransport: Send + Sync {
    /// Posts the device registration `body` and returns the response body.
    async fn device_add(&self, body: String) -> Result<String, TransportError>;
    /// Posts the security-token request `body` and returns the response body.
    async fn rst2(&self, body: String) -> Result<String, TransportError>;
}

/// Which page a web sign-in shows: the interactive Microsoft sign-in, or a per-client-id consent
/// page the token exchange asked for.
#[derive(Debug, Clone, Copy, PartialEq, Eq, uniffi::Enum)]
pub enum WebSignInKind {
    Login,
    Consent,
}

/// A page the engine asks the UI to host in a web view: the Microsoft-account sign-in.
///
/// The UI loads `url` with `headers` and `user_agent`, injects `bridge_script` at document start,
/// and returns the JSON the page posts (the sign-in result). For a `Consent` page it instead
/// resolves once the page reaches `end_url_prefix`.
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct WebSignInRequest {
    pub kind: WebSignInKind,
    pub url: String,
    pub headers: HashMap<String, String>,
    pub user_agent: String,
    pub bridge_script: String,
    pub end_url_prefix: Option<String>,
}

/// Why the web sign-in produced no result.
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Error, thiserror::Error)]
pub enum WebSignInError {
    /// The user closed the web view without completing the sign-in.
    #[error("the sign-in was declined")]
    Declined,
    /// The web view could not run the sign-in (no web view available, or a load failure).
    #[error("the sign-in could not run: {reason}")]
    Failed { reason: String },
}

/// Hosts the Microsoft-account sign-in / consent pages in a web view (macOS: `WKWebView`).
///
/// This is an engine-internal seam: the production implementation raises
/// `EngineEvent::WebSignInRequested` and waits for the UI to answer through
/// `Engine::complete_web_sign_in` / `cancel_web_sign_in`, so the UI boundary stays a plain event
/// plus a callback rather than a foreign async trait. The CLI's stand-in refuses, because an
/// interactive sign-in needs the app.
#[async_trait::async_trait]
pub trait WebSignIn: Send + Sync {
    /// Shows the sign-in page of `request` and returns the URL it ended on.
    async fn present(&self, request: WebSignInRequest) -> Result<String, WebSignInError>;
}

/// Wall-clock time for values stated in UTC (certificate validity, JWT `nbf`/`exp`, token expiry,
/// trust expiry). Durations and timers use `tokio::time`, which tests can pause.
pub trait Clock: Send + Sync {
    /// The current wall-clock time.
    fn now(&self) -> SystemTime;
}

/// The [`Clock`] of the operating system.
#[derive(Debug, Default, Clone, Copy)]
pub struct SystemClock;

impl Clock for SystemClock {
    fn now(&self) -> SystemTime {
        SystemTime::now()
    }
}

/// The handles every layer receives.
#[derive(Clone)]
pub struct Edges {
    pub store: Arc<dyn SecureStore>,
    pub lock: Arc<dyn SessionLock>,
    pub host: Arc<dyn HostInfo>,
    pub reachability: Arc<dyn Reachability>,
    pub clock: Arc<dyn Clock>,
    /// Hosts the Microsoft-account sign-in / consent pages (macOS: `WKWebView`; CLI: refuses).
    pub web_sign_in: Arc<dyn WebSignIn>,
    /// Overrides the account-service transport (tests inject a fake); `None` uses the production
    /// HTTPS transport to `login.live.com`.
    pub token_transport: Option<Arc<dyn TokenTransport>>,
}

/// The keys of one identity's stack inside the shared store: every key is stored as
/// `<prefix><key>`, so two identities' `auth/…` keys do not collide.
pub struct PrefixedStore {
    inner: Arc<dyn SecureStore>,
    prefix: String,
}

impl PrefixedStore {
    /// `prefix` must end in `/`.
    pub fn new(inner: Arc<dyn SecureStore>, prefix: &str) -> Self {
        assert!(prefix.ends_with('/') && prefix.len() > 1, "a store prefix is a non-empty path ending in '/'");
        Self { inner, prefix: prefix.to_string() }
    }
}

impl SecureStore for PrefixedStore {
    fn load(&self, key: String) -> Result<Option<Vec<u8>>, EdgeError> {
        self.inner.load(format!("{}{key}", self.prefix))
    }
    fn store(&self, key: String, value: Vec<u8>) -> Result<(), EdgeError> {
        self.inner.store(format!("{}{key}", self.prefix), value)
    }
    fn delete(&self, key: String) -> Result<(), EdgeError> {
        self.inner.delete(format!("{}{key}", self.prefix))
    }
    fn keys_with_prefix(&self, prefix: String) -> Result<Vec<String>, EdgeError> {
        let keys = self.inner.keys_with_prefix(format!("{}{prefix}", self.prefix))?;
        Ok(keys.into_iter().map(|k| k[self.prefix.len()..].to_string()).collect())
    }
}

/// Stand-ins for this module's OS edges that the headless CLI runs with.
pub mod standins {
    use super::{LockState, Reachability, SessionLock};
    use crate::sync::MutexExt;
    use std::sync::Mutex;

    /// A [`SessionLock`] whose state the test sets.
    #[derive(Debug)]
    pub struct FixedLock(pub Mutex<LockState>);

    impl FixedLock {
        /// A lock edge that reports `state` until [`FixedLock::set`] changes it.
        pub fn new(state: LockState) -> Self {
            Self(Mutex::new(state))
        }
        /// Makes the edge report `state` from now on.
        pub fn set(&self, state: LockState) {
            *self.0.lock_or_panic() = state;
        }
    }

    impl SessionLock for FixedLock {
        fn lock_state(&self) -> LockState {
            *self.0.lock_or_panic()
        }
    }

    /// A reachability edge that reports what the test sets.
    #[derive(Debug, Default)]
    pub struct FixedReachability(pub std::sync::atomic::AtomicBool);

    impl FixedReachability {
        /// A reachability edge that reports the internet as available.
        pub fn online() -> Self {
            Self(std::sync::atomic::AtomicBool::new(true))
        }
        /// Makes the edge report the internet as available or not.
        pub fn set(&self, online: bool) {
            self.0.store(online, std::sync::atomic::Ordering::SeqCst);
        }
    }

    impl Reachability for FixedReachability {
        fn is_internet_available(&self) -> bool {
            self.0.load(std::sync::atomic::Ordering::SeqCst)
        }
    }

    /// A web sign-in stand-in that always refuses: the interactive sign-in needs the app's web view.
    pub struct RefusingWebSignIn;

    #[async_trait::async_trait]
    impl super::WebSignIn for RefusingWebSignIn {
        async fn present(&self, _request: super::WebSignInRequest) -> Result<String, super::WebSignInError> {
            Err(super::WebSignInError::Failed { reason: "interactive sign-in needs the app".into() })
        }
    }
}

/// In-memory implementations for tests.
#[cfg(any(test, feature = "test-support"))]
pub mod testing {
    use std::time::Duration;

    use super::standins::{FixedLock, FixedReachability, RefusingWebSignIn};
    use super::{Clock, EdgeError, Edges, HostInfo, LockState, SecureStore, SystemClock};
    use crate::sync::MutexExt;
    use std::collections::BTreeMap;
    use std::sync::Arc;
    use std::sync::Mutex;
    use std::time::SystemTime;

    /// A [`SecureStore`] backed by a map.
    #[derive(Debug, Default)]
    pub struct MemoryStore(Mutex<BTreeMap<String, Vec<u8>>>);

    impl MemoryStore {
        pub fn new() -> Self {
            Self::default()
        }
        pub fn keys(&self) -> Vec<String> {
            self.0.lock_or_panic().keys().cloned().collect()
        }
    }

    impl SecureStore for MemoryStore {
        fn load(&self, key: String) -> Result<Option<Vec<u8>>, EdgeError> {
            Ok(self.0.lock_or_panic().get(&key).cloned())
        }
        fn store(&self, key: String, value: Vec<u8>) -> Result<(), EdgeError> {
            self.0.lock_or_panic().insert(key, value);
            Ok(())
        }
        fn delete(&self, key: String) -> Result<(), EdgeError> {
            self.0.lock_or_panic().remove(&key);
            Ok(())
        }
        fn keys_with_prefix(&self, prefix: String) -> Result<Vec<String>, EdgeError> {
            Ok(self.0.lock_or_panic().keys().filter(|k| k.starts_with(&prefix)).cloned().collect())
        }
    }

    /// A host with this name and no known region.
    #[derive(Debug, Clone)]
    pub struct FixedHost(pub String);

    impl HostInfo for FixedHost {
        fn host_name(&self) -> String {
            self.0.clone()
        }
        fn region_code(&self) -> Option<String> {
            None
        }
    }

    /// A host with a name and a region.
    #[derive(Debug, Clone)]
    pub struct RegionHost {
        pub name: String,
        pub region: String,
    }

    impl HostInfo for RegionHost {
        fn host_name(&self) -> String {
            self.name.clone()
        }
        fn region_code(&self) -> Option<String> {
            Some(self.region.clone())
        }
    }

    /// A [`Clock`] the test moves by hand.
    #[derive(Debug)]
    pub struct ManualClock(pub Mutex<SystemTime>);

    impl ManualClock {
        pub fn new(start: SystemTime) -> Self {
            Self(Mutex::new(start))
        }
        /// 2026-10-20T00:00:00Z, a fixed start for golden tests.
        pub fn fixed() -> Self {
            Self::new(SystemTime::UNIX_EPOCH + Duration::from_secs(1_792_454_400))
        }
        pub fn advance(&self, by: Duration) {
            *self.0.lock_or_panic() += by;
        }
        pub fn set(&self, to: SystemTime) {
            *self.0.lock_or_panic() = to;
        }
    }

    impl Clock for ManualClock {
        fn now(&self) -> SystemTime {
            *self.0.lock_or_panic()
        }
    }

    /// Edges wired to the in-memory implementations above (online, unlocked, host `test-mac.local`).
    pub fn memory_edges() -> Edges {
        Edges {
            store: Arc::new(MemoryStore::new()),
            lock: Arc::new(FixedLock::new(LockState::Unlocked)),
            host: Arc::new(FixedHost("test-mac.local".into())),
            reachability: Arc::new(FixedReachability::online()),
            clock: Arc::new(SystemClock),
            web_sign_in: Arc::new(RefusingWebSignIn),
            token_transport: None,
        }
    }
}

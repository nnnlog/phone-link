//! Instant hotspot's Wi-Fi OS edge: seeing, joining and leaving the hotspot.
//!
//! Bluetooth, which the side channel rides, is the shared [`phonelink_bluetooth`] edge. The
//! edge is synchronous and may block, so the engine calls it on the blocking pool. The CLI uses
//! [`NoWifi`]; the tests use [`testing`].

/// Why a Wi-Fi scan or join failed.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error, uniffi::Error)]
pub enum WifiError {
    /// This Mac has no Wi-Fi interface, or it is off.
    #[error("Wi-Fi is unavailable")]
    Unavailable,
    /// The scan or association failed.
    #[error("{reason}")]
    Failed { reason: String },
    #[error("unexpected failure in the foreign implementation: {message}")]
    Unexpected { message: String },
}

impl From<uniffi::UnexpectedUniFFICallbackError> for WifiError {
    fn from(e: uniffi::UnexpectedUniFFICallbackError) -> Self {
        WifiError::Unexpected { message: e.reason }
    }
}

/// Wi-Fi on this Mac.
#[uniffi::export(with_foreign)]
pub trait HotspotWifi: Send + Sync {
    /// This Mac has a Wi-Fi interface.
    fn has_interface(&self) -> bool;
    /// A scan finds a network named `ssid`.
    fn is_visible(&self, ssid: String) -> Result<bool, WifiError>;
    /// Joins the network named `ssid` with WPA personal `passphrase`.
    fn join(&self, ssid: String, passphrase: String) -> Result<(), WifiError>;
    /// The name of the network the Wi-Fi interface is associated with; `None` while it is associated
    /// with none, or when macOS withholds the name.
    fn current_ssid(&self) -> Option<String>;
    /// Leaves the network the Wi-Fi interface is associated with.
    fn leave(&self) -> Result<(), WifiError>;
}

/// The CLI's Wi-Fi: none.
#[derive(Debug, Default)]
pub struct NoWifi;

impl HotspotWifi for NoWifi {
    fn has_interface(&self) -> bool {
        false
    }
    fn is_visible(&self, _ssid: String) -> Result<bool, WifiError> {
        Err(WifiError::Unavailable)
    }
    fn join(&self, _ssid: String, _passphrase: String) -> Result<(), WifiError> {
        Err(WifiError::Unavailable)
    }
    fn current_ssid(&self) -> Option<String> {
        None
    }
    fn leave(&self) -> Result<(), WifiError> {
        Err(WifiError::Unavailable)
    }
}

/// In-memory stand-ins for tests.
#[cfg(any(test, feature = "test-support"))]
pub mod testing {
    use phonelink_core::sync::MutexExt;
    use std::collections::VecDeque;
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
    use std::sync::{Arc, Mutex};

    use super::{HotspotWifi, WifiError};

    /// A scripted Wi-Fi: scans, joins and the network associated with.
    ///
    /// `scans` is what each scan finds, in order (the last entry repeats); a successful join
    /// associates with its network, a successful leave clears it.
    pub struct ScriptedWifi {
        pub interface: AtomicBool,
        pub scans: Mutex<VecDeque<bool>>,
        pub join_result: Mutex<Result<(), WifiError>>,
        pub joins: Mutex<Vec<(String, String)>>,
        pub scanned: Mutex<Vec<String>>,
        pub current: Mutex<Option<String>>,
        pub leave_result: Mutex<Result<(), WifiError>>,
        pub leaves: AtomicUsize,
    }

    impl ScriptedWifi {
        pub fn new() -> Arc<Self> {
            Arc::new(Self {
                interface: AtomicBool::new(true),
                scans: Mutex::new(VecDeque::from([true])),
                join_result: Mutex::new(Ok(())),
                joins: Mutex::new(Vec::new()),
                scanned: Mutex::new(Vec::new()),
                current: Mutex::new(None),
                leave_result: Mutex::new(Ok(())),
                leaves: AtomicUsize::new(0),
            })
        }
    }

    impl HotspotWifi for ScriptedWifi {
        fn has_interface(&self) -> bool {
            self.interface.load(Ordering::SeqCst)
        }
        fn is_visible(&self, ssid: String) -> Result<bool, WifiError> {
            self.scanned.lock_or_panic().push(ssid);

            let mut scans = self.scans.lock_or_panic();
            let visible = *scans.front().expect("a scripted scan");
            if scans.len() > 1 {
                scans.pop_front();
            }
            Ok(visible)
        }
        fn join(&self, ssid: String, passphrase: String) -> Result<(), WifiError> {
            self.joins.lock_or_panic().push((ssid.clone(), passphrase));

            let result = self.join_result.lock_or_panic().clone();
            if result.is_ok() {
                *self.current.lock_or_panic() = Some(ssid);
            }
            result
        }
        fn current_ssid(&self) -> Option<String> {
            self.current.lock_or_panic().clone()
        }
        fn leave(&self) -> Result<(), WifiError> {
            self.leaves.fetch_add(1, Ordering::SeqCst);

            let result = self.leave_result.lock_or_panic().clone();
            if result.is_ok() {
                *self.current.lock_or_panic() = None;
            }
            result
        }
    }
}

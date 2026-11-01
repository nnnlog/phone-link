//! The phone's Bluetooth Classic OS edge: the radio, the phone among the devices paired with this
//! Mac, discovery and pairing, and RFCOMM channels to a service on the phone.
//!
//! The engine decides what to do and frames every byte; the edge only reaches the radio. It is
//! synchronous and may block, so the engine calls it on the blocking pool. The CLI uses
//! [`NoBluetooth`]; the tests use [`testing`].

/// Why a Bluetooth discovery and pairing ended without a paired phone.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error, uniffi::Error)]
pub enum PairingError {
    /// No device whose name matches the phone answered the inquiry in time.
    #[error("the phone was not found")]
    NotFound,
    /// A matching device was found; pairing with it failed or was refused.
    #[error("pairing failed: {reason}")]
    Failed { reason: String },
    #[error("unexpected failure in the foreign implementation: {message}")]
    Unexpected { message: String },
}

impl From<uniffi::UnexpectedUniFFICallbackError> for PairingError {
    fn from(e: uniffi::UnexpectedUniFFICallbackError) -> Self {
        PairingError::Unexpected { message: e.reason }
    }
}

/// Why an RFCOMM operation failed.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error, uniffi::Error)]
pub enum RfcommError {
    /// The device did not answer (out of range, Bluetooth off on it, no baseband connection).
    #[error("the device is unreachable")]
    Unreachable,
    /// The device answered, but it offers no RFCOMM service with the UUID.
    #[error("the service is not offered")]
    ServiceNotFound,
    /// The channel did not open.
    #[error("connect failed: {reason}")]
    ConnectFailed { reason: String },
    /// Writing to or reading from an open channel failed.
    #[error("send or receive failed: {reason}")]
    Io { reason: String },
    /// The operation did not finish within its timeout.
    #[error("timed out")]
    TimedOut,
    #[error("unexpected failure in the foreign implementation: {message}")]
    Unexpected { message: String },
}

impl From<uniffi::UnexpectedUniFFICallbackError> for RfcommError {
    fn from(e: uniffi::UnexpectedUniFFICallbackError) -> Self {
        RfcommError::Unexpected { message: e.reason }
    }
}

/// Bluetooth Classic on this Mac. `names` are the phone's names; a device is the phone when its name
/// equals one of them ignoring case and surrounding white space (an empty name matches nothing).
#[uniffi::export(with_foreign)]
pub trait PhoneBluetooth: Send + Sync {
    /// The Bluetooth radio is on.
    fn radio_on(&self) -> bool;
    /// The address (`xx-xx-xx-xx-xx-xx`) of the first device paired with this Mac that is the phone.
    fn paired_phone(&self, names: Vec<String>) -> Option<String>;
    /// Runs an inquiry for at most `timeout_ms` and pairs with the first device found that is the
    /// phone (the user confirms the pairing code); returns its address.
    fn discover_and_pair(&self, names: Vec<String>, timeout_ms: u64) -> Result<String, PairingError>;
    /// Looks up the device's RFCOMM service `service_uuid` in the service records the Mac holds for the
    /// paired device and opens an encrypted, authenticated channel to it (at most `timeout_ms`);
    /// returns the channel's handle.
    fn rfcomm_open(&self, address: String, service_uuid: String, timeout_ms: u64) -> Result<u64, RfcommError>;
    /// Makes the Mac read the paired device's service records again — by connecting to the device
    /// when it is not connected, and closing that connection afterwards — and waits at most
    /// `timeout_ms` for the records the Mac holds to include `service_uuid`; returns whether they do.
    fn refresh_services(&self, address: String, service_uuid: String, timeout_ms: u64) -> Result<bool, RfcommError>;
    /// Writes `bytes` on the channel.
    fn rfcomm_write(&self, channel: u64, bytes: Vec<u8>) -> Result<(), RfcommError>;
    /// The bytes received on the channel since the last read, waiting at most `timeout_ms` for some;
    /// empty when the channel has closed. A wait that ends without data is `TimedOut`.
    fn rfcomm_read(&self, channel: u64, timeout_ms: u64) -> Result<Vec<u8>, RfcommError>;
    /// Closes the channel; closing a closed or unknown channel does nothing.
    fn rfcomm_close(&self, channel: u64);
}

/// The CLI's Bluetooth: none.
#[derive(Debug, Default)]
pub struct NoBluetooth;

impl PhoneBluetooth for NoBluetooth {
    fn radio_on(&self) -> bool {
        false
    }
    fn paired_phone(&self, _names: Vec<String>) -> Option<String> {
        None
    }
    fn discover_and_pair(&self, _names: Vec<String>, _timeout_ms: u64) -> Result<String, PairingError> {
        Err(PairingError::Unexpected { message: "this host has no Bluetooth".into() })
    }
    fn rfcomm_open(&self, _address: String, _service_uuid: String, _timeout_ms: u64) -> Result<u64, RfcommError> {
        Err(RfcommError::Unexpected { message: "this host has no Bluetooth".into() })
    }
    fn refresh_services(&self, _address: String, _service_uuid: String, _timeout_ms: u64) -> Result<bool, RfcommError> {
        Err(RfcommError::Unexpected { message: "this host has no Bluetooth".into() })
    }
    fn rfcomm_write(&self, _channel: u64, _bytes: Vec<u8>) -> Result<(), RfcommError> {
        Err(RfcommError::Unexpected { message: "this host has no Bluetooth".into() })
    }
    fn rfcomm_read(&self, _channel: u64, _timeout_ms: u64) -> Result<Vec<u8>, RfcommError> {
        Err(RfcommError::Unexpected { message: "this host has no Bluetooth".into() })
    }
    fn rfcomm_close(&self, _channel: u64) {}
}

/// In-memory stand-ins for tests.
#[cfg(any(test, feature = "test-support"))]
pub mod testing {
    use phonelink_core::sync::MutexExt;
    use std::collections::VecDeque;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::{Arc, Mutex};

    use super::{PairingError, PhoneBluetooth, RfcommError};

    /// What a scripted phone answers on one RFCOMM channel: the request bytes it received go to
    /// `answer`, whose result is the bytes it sends back (`None`: it closes without answering).
    pub type Answer = Box<dyn Fn(&[u8]) -> Option<Vec<u8>> + Send + Sync>;

    /// A scripted Bluetooth: a radio switch, the paired phone's address, a discovery result, and
    /// RFCOMM attempts that fail in order (`open_failures`) before channels answer with `answer`.
    ///
    /// A service-record refresh answers `refresh`.
    pub struct ScriptedBluetooth {
        pub radio: AtomicBool,
        pub paired: Mutex<Option<String>>,
        pub discovery: Mutex<Result<String, PairingError>>,
        pub open_failures: Mutex<VecDeque<RfcommError>>,
        pub refresh: Mutex<Result<bool, RfcommError>>,
        /// The (address, service UUID) of every service-record refresh.
        pub refreshed: Mutex<Vec<(String, String)>>,
        pub answer: Mutex<Option<Answer>>,
        /// The names each `paired_phone` and `discover_and_pair` call asked for.
        pub asked_names: Mutex<Vec<Vec<String>>>,
        /// The bytes written on every channel, one entry per channel.
        pub written: Mutex<Vec<Vec<u8>>>,
        pub opened: Mutex<Vec<(String, String)>>,
        pub closed: Mutex<Vec<u64>>,
        pending: Mutex<VecDeque<Option<Vec<u8>>>>,
    }

    impl ScriptedBluetooth {
        pub fn new() -> Arc<Self> {
            Arc::new(Self {
                radio: AtomicBool::new(true),
                paired: Mutex::new(None),
                discovery: Mutex::new(Err(PairingError::NotFound)),
                open_failures: Mutex::new(VecDeque::new()),
                refresh: Mutex::new(Ok(true)),
                refreshed: Mutex::new(Vec::new()),
                answer: Mutex::new(None),
                asked_names: Mutex::new(Vec::new()),
                written: Mutex::new(Vec::new()),
                opened: Mutex::new(Vec::new()),
                closed: Mutex::new(Vec::new()),
                pending: Mutex::new(VecDeque::new()),
            })
        }

        pub fn set_paired(&self, address: Option<&str>) {
            *self.paired.lock_or_panic() = address.map(str::to_string);
        }

        pub fn set_answer(&self, answer: Answer) {
            *self.answer.lock_or_panic() = Some(answer);
        }
    }

    impl PhoneBluetooth for ScriptedBluetooth {
        fn radio_on(&self) -> bool {
            self.radio.load(Ordering::SeqCst)
        }
        fn paired_phone(&self, names: Vec<String>) -> Option<String> {
            self.asked_names.lock_or_panic().push(names);
            self.paired.lock_or_panic().clone()
        }
        fn discover_and_pair(&self, names: Vec<String>, _timeout_ms: u64) -> Result<String, PairingError> {
            self.asked_names.lock_or_panic().push(names);

            let result = self.discovery.lock_or_panic().clone();
            if let Ok(address) = &result {
                *self.paired.lock_or_panic() = Some(address.clone());
            }
            result
        }
        fn rfcomm_open(&self, address: String, service_uuid: String, _timeout_ms: u64) -> Result<u64, RfcommError> {
            self.opened.lock_or_panic().push((address, service_uuid));

            if let Some(e) = self.open_failures.lock_or_panic().pop_front() {
                return Err(e);
            }

            let mut written = self.written.lock_or_panic();
            written.push(Vec::new());
            Ok(written.len() as u64)
        }
        fn refresh_services(
            &self,
            address: String,
            service_uuid: String,
            _timeout_ms: u64,
        ) -> Result<bool, RfcommError> {
            self.refreshed.lock_or_panic().push((address, service_uuid));
            self.refresh.lock_or_panic().clone()
        }
        fn rfcomm_write(&self, channel: u64, bytes: Vec<u8>) -> Result<(), RfcommError> {
            let mut written = self.written.lock_or_panic();
            let slot = written.get_mut(channel as usize - 1).expect("written on an open channel");
            slot.extend_from_slice(&bytes);

            let reply = self.answer.lock_or_panic().as_ref().map(|a| a(slot));
            if let Some(reply) = reply {
                let mut pending = self.pending.lock_or_panic();
                match reply {
                    Some(bytes) => {
                        // Two pieces, so the reader reassembles a frame.
                        let mid = bytes.len() / 2;
                        pending.push_back(Some(bytes[..mid].to_vec()));
                        pending.push_back(Some(bytes[mid..].to_vec()));
                    }
                    None => pending.push_back(None),
                }
            }

            Ok(())
        }
        fn rfcomm_read(&self, _channel: u64, _timeout_ms: u64) -> Result<Vec<u8>, RfcommError> {
            match self.pending.lock_or_panic().pop_front() {
                Some(Some(bytes)) => Ok(bytes),
                Some(None) => Ok(Vec::new()),
                None => Err(RfcommError::TimedOut),
            }
        }
        fn rfcomm_close(&self, channel: u64) {
            self.closed.lock_or_panic().push(channel);
            self.pending.lock_or_panic().clear();
        }
    }
}

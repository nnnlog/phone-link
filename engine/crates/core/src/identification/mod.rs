//! The client-identification headers sent on every DCG REST request and on the `SignalR` connection,
//! with the engine's own values where Phone Link on Windows derives one from Windows.

use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};

use base64::Engine as _;
use rand::RngCore;

use crate::config::EngineConfig;
use crate::constants::http_header as h;
use crate::edge::{EdgeError, SecureStore};
use crate::sync::MutexExt;
use crate::types::new_guid_lower;

/// Store key of the persisted `DCG-LogicalDeviceId`.
pub const LOGICAL_DEVICE_ID_KEY: &str = "auth/logical-device-id";

/// `DCG-SessionId`: one random lower-case GUID per process and PC identity, minted on first use:
/// every engine core of the process sends the same one for `client_type`.
pub fn process_session_id(client_type: &str) -> String {
    static SESSION_IDS: OnceLock<Mutex<HashMap<String, String>>> = OnceLock::new();
    let mut ids = SESSION_IDS.get_or_init(Mutex::default).lock_or_recover();
    ids.entry(client_type.to_string()).or_insert_with(new_guid_lower).clone()
}

/// The seven identification headers of one process.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClientIdentification {
    /// 32 random bytes, standard base64 (44 characters), minted once and persisted.
    pub logical_device_id: String,
    pub app_version: String,
    pub app_id: String,
    /// One random lower-case GUID per process ([`process_session_id`]).
    pub session_id: String,
    pub ring_name: String,
    pub os: String,
    pub os_version: String,
}

impl ClientIdentification {
    /// Loads the persisted logical device id (minting and storing it on first use) and takes the
    /// rest from `config`; `session_id` is this process's ([`process_session_id`]), whichever load
    /// asks.
    pub fn load_or_create(config: &EngineConfig, store: &dyn SecureStore) -> Result<Self, EdgeError> {
        let logical_device_id = match store.load(LOGICAL_DEVICE_ID_KEY.into())? {
            Some(bytes) => String::from_utf8(bytes)
                .map_err(|e| EdgeError::Store { message: format!("{LOGICAL_DEVICE_ID_KEY} is not UTF-8: {e}") })?,
            None => {
                let mut raw = [0u8; 32];
                rand::rng().fill_bytes(&mut raw);
                let id = base64::prelude::BASE64_STANDARD.encode(raw);
                store.store(LOGICAL_DEVICE_ID_KEY.into(), id.clone().into_bytes())?;
                id
            }
        };

        Ok(Self {
            logical_device_id,
            app_version: config.identity.profile.package_version.to_string(),
            app_id: config.identity.profile.package_family.to_string(),
            session_id: process_session_id(config.identity.profile.client_type),
            ring_name: config.identity.ring_name.clone(),
            os: config.identity.os_name.clone(),
            os_version: config.identity.os_version.clone(),
        })
    }

    /// The headers, in a fixed order.
    pub fn headers(&self) -> [(&'static str, &str); 7] {
        [
            (h::DCG_LOGICAL_DEVICE_ID, &self.logical_device_id),
            (h::DCG_APP_VERSION, &self.app_version),
            (h::DCG_APP_ID, &self.app_id),
            (h::DCG_SESSION_ID, &self.session_id),
            (h::DCG_RING_NAME, &self.ring_name),
            (h::DCG_OS, &self.os),
            (h::DCG_OS_VERSION, &self.os_version),
        ]
    }
}

#[cfg(test)]
mod tests;

//! The selected phone as the UI names it: the phone record the App layer keeps from the device list
//! (Android) and the connection state; and the account's PCs, which the user can unlink.

use std::sync::Arc;

use crate::api::engine::Engine;
use crate::engine::EngineError;

use super::{FeatureApiError, core_of, selected_phone};

/// The name shown when the phone reports none (local UI text, as the low-battery notification uses).
pub const FALLBACK_PHONE_NAME: &str = "Phone";

/// The selected phone.
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct PhoneInfo {
    /// The phone's `dcgClientId`.
    pub id: String,
    /// The name the phone reports, else its device-list name, else "Phone".
    pub name: String,
    pub manufacturer: Option<String>,
    pub model: Option<String>,
    /// The phone's OS version: the last value the device list or a connect reply wrote.
    pub os_version: Option<String>,
    /// The platform session with the phone is connected.
    pub connected: bool,
    /// The phone turned every feature off for this PC.
    pub all_features_disabled: bool,
}

/// A phone the user can select.
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct PhoneChoice {
    /// The phone's `dcgClientId`.
    pub id: String,
    /// Its device-list name, else its model, else "Phone".
    pub name: String,
    pub model: Option<String>,
    /// When the account's device list last saw it, Unix seconds (`0` unknown).
    pub last_seen: i64,
    pub selected: bool,
}

/// A PC of the account's device list.
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct AccountPc {
    /// Its `dcgClientId`.
    pub id: String,
    /// Its device-list name ("" when it has none).
    pub name: String,
    /// The app it was enrolled for (`PL`, `WEA`, …; "" when unknown).
    pub client_type: String,
    /// When it was enrolled and when the device list last saw it, Unix seconds (`0` unknown).
    pub registered: i64,
    pub last_seen: i64,
    /// One of this Mac's own identities (never unlinked from here).
    pub this_mac: bool,
}

/// The selected phone and the choice of it.
#[derive(uniffi::Object)]
pub struct PhoneApi {
    engine: Arc<Engine>,
}

impl PhoneApi {
    pub(crate) fn new(engine: Arc<Engine>) -> Arc<Self> {
        Arc::new(Self { engine })
    }
}

#[uniffi::export(async_runtime = "tokio")]
impl PhoneApi {
    /// The selected phone's name, make, model, OS version and connection state.
    ///
    /// Fails with `NoPhone` when no phone is selected. A phone that reports no name is "Phone".
    pub fn info(&self) -> Result<PhoneInfo, FeatureApiError> {
        let core = core_of(&self.engine)?;
        let phone = selected_phone(&core)?;
        let record = core.app.phone_record();

        let nonblank = |s: Option<String>| s.filter(|n| !n.trim().is_empty());
        let name = record
            .as_ref()
            .and_then(|r| nonblank(r.display_name.clone()).or_else(|| nonblank(r.name.clone())))
            .unwrap_or_else(|| FALLBACK_PHONE_NAME.to_owned());

        Ok(PhoneInfo {
            id: phone.id.clone(),
            name,
            manufacturer: record.as_ref().and_then(|r| nonblank(r.manufacturer.clone())),
            model: record.as_ref().and_then(|r| nonblank(r.device_model.clone())),
            os_version: record.as_ref().and_then(|r| nonblank(r.current_os_version().map(str::to_owned))),
            connected: core.phone_connected(),
            all_features_disabled: record.as_ref().is_some_and(|r| r.all_features_disabled),
        })
    }

    /// The account's linked phones, the selected one marked, most recently seen first.
    pub fn phones(&self) -> Result<Vec<PhoneChoice>, FeatureApiError> {
        let core = core_of(&self.engine)?;
        let selected = core.phone();
        let nonblank = |s: &Option<String>| s.clone().filter(|n| !n.trim().is_empty());

        let mut phones: Vec<PhoneChoice> = core
            .phones()
            .into_iter()
            .map(|p| PhoneChoice {
                selected: selected
                    .as_ref()
                    .is_some_and(|s| s.env == p.client.env && s.id.eq_ignore_ascii_case(&p.client.id)),
                name: nonblank(&p.info.name)
                    .or_else(|| nonblank(&p.info.model_name))
                    .unwrap_or_else(|| FALLBACK_PHONE_NAME.to_owned()),
                model: nonblank(&p.info.model_name),
                last_seen: p.info.last_seen_time.unwrap_or(0),
                id: p.client.id,
            })
            .collect();

        phones.sort_by_key(|p| std::cmp::Reverse(p.last_seen));
        Ok(phones)
    }

    /// The account's PCs, this Mac's identities marked, most recently seen first (no network).
    pub fn account_pcs(&self) -> Result<Vec<AccountPc>, FeatureApiError> {
        let core = core_of(&self.engine)?;
        let own = core.own_clients();

        let mut pcs: Vec<AccountPc> = core
            .account_pcs()
            .into_iter()
            .map(|p| AccountPc {
                this_mac: own.iter().any(|c| c.env == p.client.env && c.id.eq_ignore_ascii_case(&p.client.id)),
                name: p.info.name.clone().unwrap_or_default(),
                client_type: p.info.client_type.clone().unwrap_or_default(),
                registered: p.info.registration_time.unwrap_or(0),
                last_seen: p.info.last_seen_time.unwrap_or(0),
                id: p.client.id,
            })
            .collect();

        pcs.sort_by_key(|p| std::cmp::Reverse(p.last_seen));
        Ok(pcs)
    }

    /// Unlinks the PC `id`, one of [`PhoneApi::account_pcs`] that is not this Mac, from the account.
    pub async fn unlink_pc(&self, id: String) -> Result<(), FeatureApiError> {
        let core = core_of(&self.engine)?;
        core.unlink_pc(&id).await.map_err(|e| match e {
            EngineError::NotStarted => FeatureApiError::NotStarted,
            other => FeatureApiError::NotAvailable { reason: other.to_string() },
        })
    }

    /// Selects the phone `id`, one of [`PhoneApi::phones`]: it becomes the selected phone, is
    /// remembered across restarts, and gets a background connect.
    pub async fn select(&self, id: String) -> Result<(), FeatureApiError> {
        let core = core_of(&self.engine)?;
        core.select_phone(&id).await.map(|_| ()).map_err(|e| match e {
            EngineError::NoPhone => FeatureApiError::UnknownItem { what: "phone".into() },
            EngineError::NotStarted => FeatureApiError::NotStarted,
            other => FeatureApiError::NotAvailable { reason: other.to_string() },
        })
    }
}

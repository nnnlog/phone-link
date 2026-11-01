//! The request and response bodies.

use phonelink_core::codec::json::OrderedStringMap;
use phonelink_core::types::DeviceCertificates;
use serde::ser::SerializeMap;
use serde::{Deserialize, Serialize};

/// `metadata` of `EnrollDevice` / `SetDeviceInfo`, fields in the order Phone Link on Windows sends;
/// `distinguishedDeviceId` / `globalDeviceId` are never assigned (their purpose is unknown) and
/// omitted.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DeviceMetadata {
    pub capabilities: Vec<String>,
    pub client_type: String,
    pub client_version: String,
    pub consent_version: i32,
    pub custom_data: String,
    pub display_name: String,
    pub is_enabled: bool,
    pub manufacture: String,
    pub model_name: String,
    pub model_version: String,
    pub os_name: String,
    pub os_version: String,
}

/// `{ "metadata": { … }, "certificates": { "SelfSigned": [ <trust certificate> ] } }`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DeviceInfoBody {
    pub metadata: DeviceMetadata,
    pub certificates: DeviceCertificates,
}

/// `WakeBody` of `/Dispatcher/Wake`: `deviceId`, `ttl`, `priority`, `collapseKey`, `data` —
/// written in that order, `data` in insertion order.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WakeBody {
    pub device_id: String,
    pub ttl: u64,
    pub priority: String,
    pub collapse_key: String,
    pub data: OrderedStringMap,
}

impl Serialize for WakeBody {
    fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        let mut m = s.serialize_map(Some(5))?;
        m.serialize_entry("deviceId", &self.device_id)?;
        m.serialize_entry("ttl", &self.ttl)?;
        m.serialize_entry("priority", &self.priority)?;
        m.serialize_entry("collapseKey", &self.collapse_key)?;
        m.serialize_entry("data", &self.data)?;
        m.end()
    }
}

/// `NonceResponse`: `nonce` is required; `epochTimeStamp` is not consumed.
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct NonceResponse {
    #[serde(default)]
    pub(super) nonce: Option<String>,
}

/// `TokenResponse` as received.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct RawTokenResponse {
    #[serde(default)]
    pub(super) access_token: Option<String>,
    #[serde(default)]
    pub(super) device_id: Option<String>,
    #[serde(default)]
    pub(super) epoch_expiration_time: Option<i64>,
    #[serde(default)]
    pub(super) key_valid_remaining_days: Option<i64>,
}

/// A `TokenResponse` whose required fields are present: `accessToken` non-empty, `deviceId`,
/// `epochExpirationTime`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TokenResponse {
    pub access_token: String,
    pub device_id: String,
    pub epoch_expiration_time: i64,
    pub key_valid_remaining_days: Option<i64>,
}

/// `EnrollDevice` response: only `accountCert` is consumed; `accountInfo`, `devices` and
/// `rootCertificateChain` are tolerated and ignored.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EnrollDeviceResponse {
    /// `None` when absent or not a string — the caller reports `AccountTrustSyncFailure`.
    pub account_cert: Option<String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct RawEnrollResponse {
    #[serde(default)]
    pub(super) account_cert: Option<serde_json::Value>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct AccountCertResponse {
    #[serde(default)]
    pub(super) account_cert: Option<serde_json::Value>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct EnvironmentResponse {
    #[serde(default)]
    pub(super) service_environment: Option<serde_json::Value>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct DeviceIdBody<'a> {
    pub(super) device_id: &'a str,
}

#[derive(Serialize)]
pub(super) struct CertificateJwtBody<'a> {
    #[serde(rename = "certificateJWT")]
    pub(super) certificate_jwt: &'a str,
}

#[derive(Serialize)]
pub(super) struct RotateKeysBody<'a> {
    #[serde(rename = "oldCertificateJWT")]
    pub(super) old_certificate_jwt: &'a str,
    #[serde(rename = "newCertificateJWT")]
    pub(super) new_certificate_jwt: &'a str,
}

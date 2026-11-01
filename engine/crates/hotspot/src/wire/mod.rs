//! The side channel's wire: varint32 length-prefixed protobuf frames, the credentials request and
//! its authorization data, the checks on the phone's reply, and the RSA-OAEP credentials.

use base64::Engine as _;
use base64::prelude::BASE64_STANDARD as B64;
use prost::Message;
use rand::RngCore;
use rsa::pkcs8::EncodePublicKey;
use rsa::{Oaep, RsaPrivateKey};

use phonelink_core::codec::json::OrderedStringMap;
use phonelink_core::types::{DcgClient, TraceContext};
use phonelink_proto::side_channel::v1::{
    SideChannelAuthorization, SideChannelClientRequest, SideChannelErrorType, SideChannelHotspotCapableStatus,
    SideChannelHotspotRequest, SideChannelHotspotRequestType, SideChannelHotspotResponseStatus,
    SideChannelServerMessage, SideChannelServerResponse, SideChannelTraceContext, side_channel_client_request,
    side_channel_server_message, side_channel_server_response,
};

/// The phone's side-channel RFCOMM service.
pub const SERVICE_UUID: &str = "111FAB8F-00A2-4935-B6B4-ACA388CDD473";
/// `Scope` of the request's authorization JWT.
pub const REQUEST_SCOPE: &str = "hotspot.request";
/// `Scope` the phone's reply JWT must carry.
pub const RESPONSE_SCOPE: &str = "hotspot.response";
/// `ypTriggerId` of the credentials request's trace.
pub const TRIGGER: &str = "RequestCredentials";
/// `ypScenarioId` of every hotspot trace.
pub const SCENARIO: &str = "InstantHotspot";
/// The largest inbound frame the port accepts (differs from Phone Link on Windows, which sets none).
pub const MAX_FRAME: usize = 64 * 1024;
/// Random bytes in a nonce.
const NONCE_BYTES: usize = 128;
const RSA_BITS: usize = 2048;
/// The SSID's and the passphrase's limits, in UTF-16 code units.
const SSID_MAX: usize = 32;
const PASSPHRASE_MIN: usize = 8;
const PASSPHRASE_MAX: usize = 63;

// ------------------------------------------------------------------------------------------------
// Framing
// ------------------------------------------------------------------------------------------------

/// `message` behind its varint32 byte length.
pub fn encode_frame(message: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(message.len() + 5);
    let mut n = u32::try_from(message.len()).expect("a side-channel message fits in u32");

    while n >= 0x80 {
        out.push((n as u8 & 0x7F) | 0x80);
        n >>= 7;
    }
    out.push(n as u8);
    out.extend_from_slice(message);
    out
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum FrameError {
    #[error("malformed varint32 length")]
    MalformedLength,
    #[error("frame of {0} bytes is larger than {MAX_FRAME}")]
    TooLarge(usize),
}

/// Reassembles frames from the bytes a channel delivers in arbitrary pieces.
#[derive(Debug, Default)]
pub struct FrameDecoder {
    buf: Vec<u8>,
}

impl FrameDecoder {
    pub fn push(&mut self, bytes: &[u8]) {
        self.buf.extend_from_slice(bytes);
    }

    /// The next complete frame's body, `None` while one is incomplete.
    pub fn next_frame(&mut self) -> Result<Option<Vec<u8>>, FrameError> {
        let mut len: usize = 0;
        let mut header = None;
        for (i, b) in self.buf.iter().take(5).enumerate() {
            len |= usize::from(b & 0x7F) << (7 * i);
            if b & 0x80 == 0 {
                header = Some(i + 1);
                break;
            }
        }

        let Some(header) = header else {
            return if self.buf.len() >= 5 { Err(FrameError::MalformedLength) } else { Ok(None) };
        };
        if len > MAX_FRAME {
            return Err(FrameError::TooLarge(len));
        }
        if self.buf.len() < header + len {
            return Ok(None);
        }

        let frame = self.buf[header..header + len].to_vec();
        self.buf.drain(..header + len);
        Ok(Some(frame))
    }
}

// ------------------------------------------------------------------------------------------------
// The request
// ------------------------------------------------------------------------------------------------

/// A nonce: standard base64 of 128 random bytes.
pub fn new_nonce() -> String {
    let mut bytes = [0u8; NONCE_BYTES];
    rand::rng().fill_bytes(&mut bytes);
    B64.encode(bytes)
}

/// The authorization JWT's `Data`, in order: `publicKey`, `nonce`, `DCG-Environment`.
pub fn authorization_data(public_key: &str, nonce: &str, phone: &DcgClient) -> OrderedStringMap {
    let mut data = OrderedStringMap::default();
    data.push("publicKey", public_key);
    data.push("nonce", nonce);
    data.push("DCG-Environment", phone.env.name());
    data
}

/// The credentials request's trace: a new root with the hotspot names, not user-initiated.
pub fn request_trace() -> TraceContext {
    TraceContext::new_root(vec![
        (phonelink_core::types::trace_key::TRIGGER.into(), TRIGGER.into()),
        (phonelink_core::types::trace_key::SCENARIO.into(), SCENARIO.into()),
        ("isuserinitiated".into(), "False".into()),
    ])
}

/// `SideChannelClientRequest { context, hotspot_request { CREDENTIALS }, authorization }`.
pub fn credentials_request(trace: &TraceContext, signed_jwt: String) -> SideChannelClientRequest {
    SideChannelClientRequest {
        context: Some(SideChannelTraceContext {
            trace_id: trace.trace_id.clone(),
            parent_id: trace.parent_id.clone().unwrap_or_default(),
            correlation_id: trace.correlation_id.clone(),
            trace_state: trace.trace_state.iter().cloned().collect(),
        }),
        authorization: Some(SideChannelAuthorization { signed_jwt_payload: signed_jwt }),
        request: Some(side_channel_client_request::Request::HotspotRequest(SideChannelHotspotRequest {
            r#type: SideChannelHotspotRequestType::Credentials as i32,
        })),
    }
}

// ------------------------------------------------------------------------------------------------
// The reply
// ------------------------------------------------------------------------------------------------

/// Why the phone cannot share its connection (`reservation.status` of a not-capable reply).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Unavailable {
    AirplaneMode,
    BlockedByPolicy,
    DataPlan,
    DataSaver,
    PasswordNotSet,
    SimNotReady,
    SmartViewInUse,
    WifiDirectInUse,
    WirelessDexInUse,
    LinkToWindowsOff,
    BluetoothPermission,
    Other,
}

impl Unavailable {
    fn from_status(status: i32) -> Self {
        use SideChannelHotspotCapableStatus as S;
        match S::try_from(status) {
            Ok(S::AirplaneMode) => Self::AirplaneMode,
            Ok(S::BlockedByPolicy) => Self::BlockedByPolicy,
            Ok(S::DataNotAllowed | S::UnsupportedDataPlan) => Self::DataPlan,
            Ok(S::DataSaverIsEnabled) => Self::DataSaver,
            Ok(S::HotspotPasswordNotSet) => Self::PasswordNotSet,
            Ok(S::SimNotReady) => Self::SimNotReady,
            Ok(S::SmartViewInUse) => Self::SmartViewInUse,
            Ok(S::WifiDirectInUse) => Self::WifiDirectInUse,
            Ok(S::WirelessDexInUse) => Self::WirelessDexInUse,
            Ok(S::LinkToWindowsNotTurnedOn) => Self::LinkToWindowsOff,
            Ok(S::BluetoothPermissionNotGranted) => Self::BluetoothPermission,
            _ => Self::Other,
        }
    }
}

/// What a reply that does not carry credentials says.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ReplyError {
    /// `ok` false with an error other than `SCENARIO_SERVICE_ERROR`.
    #[error("the phone answered error {0:?}")]
    Remote(SideChannelErrorType),
    #[error("the reply carries no hotspot response")]
    NoHotspotResponse,
    #[error("the phone does not trust this PC")]
    Untrusted,
    #[error("the phone answered {0:?}")]
    PhoneError(SideChannelHotspotResponseStatus),
    #[error("the phone cannot share its connection: {0:?}")]
    Unavailable(Unavailable),
    #[error("the reply carries no authorization")]
    AuthorizationMissing,
    #[error("the reply's nonce is not the request's")]
    InvalidNonce,
}

impl ReplyError {
    /// The phone asked to try again: its trust check timed out.
    pub fn is_retryable(&self) -> bool {
        matches!(self, ReplyError::Remote(SideChannelErrorType::CryptoTrustTimeout))
    }
}

/// The server response a frame carries, `None` for a frame without `request_response`.
pub fn server_response(frame: &[u8]) -> Result<Option<SideChannelServerResponse>, prost::DecodeError> {
    let message = SideChannelServerMessage::decode(frame)?;
    Ok(message.body.map(|side_channel_server_message::Body::RequestResponse(r)| r))
}

/// Checks a successful credentials reply up to its JWT: the response is read, its status is
/// `SUCCESS`, it carries an authorization and the request's `nonce`. Returns the JWT to validate.
pub fn check_reply(response: &SideChannelServerResponse, nonce: &str) -> Result<String, ReplyError> {
    if !response.ok {
        let error_type = response.error.as_ref().map_or(0, |e| e.r#type);
        let error_type = SideChannelErrorType::try_from(error_type).unwrap_or(SideChannelErrorType::Unspecified);
        if error_type != SideChannelErrorType::ScenarioServiceError {
            return Err(ReplyError::Remote(error_type));
        }
    }

    let Some(side_channel_server_response::Response::HotspotResponse(hotspot)) = &response.response else {
        return Err(ReplyError::NoHotspotResponse);
    };

    use SideChannelHotspotResponseStatus as S;
    match S::try_from(hotspot.status) {
        Ok(S::Success) => {}
        Ok(S::Untrusted) => return Err(ReplyError::Untrusted),
        Ok(S::NotHotspotCapable) => {
            let status = hotspot.reservation.as_ref().map_or(0, |r| r.status);
            return Err(ReplyError::Unavailable(Unavailable::from_status(status)));
        }
        Ok(other) => return Err(ReplyError::PhoneError(other)),
        Err(_) => return Err(ReplyError::PhoneError(S::Unspecified)),
    }

    let Some(authorization) = &response.authorization else {
        return Err(ReplyError::AuthorizationMissing);
    };
    if hotspot.reservation.as_ref().map(|r| r.nonce.as_str()) != Some(nonce) {
        return Err(ReplyError::InvalidNonce);
    }
    Ok(authorization.signed_jwt_payload.clone())
}

// ------------------------------------------------------------------------------------------------
// The credentials
// ------------------------------------------------------------------------------------------------

/// The hotspot's network.
#[derive(Clone, PartialEq, Eq)]
pub struct Credentials {
    pub ssid: String,
    pub passphrase: String,
}

impl std::fmt::Debug for Credentials {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Credentials").field("ssid_len", &self.ssid.len()).field("passphrase", &"<hidden>").finish()
    }
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum CredentialsError {
    #[error("the reply's data is not a JSON object")]
    Json,
    #[error("the reply's data carries no credentials")]
    Missing,
    #[error("the credentials do not decrypt")]
    Decrypt,
    #[error("the credentials are invalid: {0}")]
    Invalid(&'static str),
}

/// `encryptedCredentials` and `encryptedSsidLength` from the reply JWT's `Data`, each as its text.
pub fn encrypted_credentials(data: Option<&str>) -> Result<(String, String), CredentialsError> {
    let data = data.ok_or(CredentialsError::Missing)?;
    let value: serde_json::Value = serde_json::from_str(data).map_err(|_| CredentialsError::Json)?;
    let object = value.as_object().ok_or(CredentialsError::Json)?;

    let text = |key: &str| {
        object.get(key).map(|v| match v {
            serde_json::Value::String(s) => s.clone(),
            other => other.to_string(),
        })
    };
    match (text("encryptedCredentials"), text("encryptedSsidLength")) {
        (Some(c), Some(n)) => Ok((c, n)),
        _ => Err(CredentialsError::Missing),
    }
}

/// The key pair of one connect: the phone encrypts the credentials with its public half.
pub struct HotspotKeyPair {
    key: RsaPrivateKey,
}

impl HotspotKeyPair {
    /// A fresh RSA-2048 key pair.
    pub fn generate() -> Result<Self, rsa::Error> {
        let mut rng = rand_core06::OsRng;
        Ok(Self { key: RsaPrivateKey::new(&mut rng, RSA_BITS)? })
    }

    /// Standard base64 of the public key's DER `SubjectPublicKeyInfo`.
    pub fn public_key(&self) -> String {
        let der = self.key.to_public_key().to_public_key_der().expect("an RSA public key encodes as SPKI");
        B64.encode(der.as_bytes())
    }

    /// Decrypts `encrypted` (base64, RSA-OAEP with SHA-256) and splits the text after the first
    /// `ssid_length` UTF-16 code units into SSID and passphrase, then checks both.
    pub fn credentials(&self, encrypted: &str, ssid_length: &str) -> Result<Credentials, CredentialsError> {
        let ciphertext = B64.decode(encrypted.trim()).map_err(|_| CredentialsError::Decrypt)?;
        let plaintext =
            self.key.decrypt(Oaep::new::<sha2_010::Sha256>(), &ciphertext).map_err(|_| CredentialsError::Decrypt)?;
        let text = String::from_utf8(plaintext).map_err(|_| CredentialsError::Decrypt)?;
        split_credentials(&text, ssid_length)
    }
}

/// Splits `text` into the SSID and the passphrase at `ssid_length` UTF-16 code units.
///
/// The phone counts the SSID in Java `char`s. `ssid_length` must be a decimal integer in `1..=` the
/// unit count and must not split a surrogate pair; the SSID is 1–32 units, the passphrase 8–63.
pub fn split_credentials(text: &str, ssid_length: &str) -> Result<Credentials, CredentialsError> {
    let n: usize = ssid_length.trim().parse().map_err(|_| CredentialsError::Invalid("SSID length not a number"))?;
    let units: Vec<u16> = text.encode_utf16().collect();
    if n == 0 || n > units.len() {
        return Err(CredentialsError::Invalid("SSID length out of range"));
    }

    let (ssid, passphrase) = units.split_at(n);
    if ssid.len() > SSID_MAX {
        return Err(CredentialsError::Invalid("SSID longer than 32 units"));
    }
    if !(PASSPHRASE_MIN..=PASSPHRASE_MAX).contains(&passphrase.len()) {
        return Err(CredentialsError::Invalid("passphrase not 8 to 63 units"));
    }

    let decode =
        |u: &[u16]| String::from_utf16(u).map_err(|_| CredentialsError::Invalid("SSID length splits a character"));
    Ok(Credentials { ssid: decode(ssid)?, passphrase: decode(passphrase)? })
}

#[cfg(test)]
mod tests;

//! Device registration at the account service's `ppsecure/deviceaddcredential.srf`: build the
//! `<DeviceAddRequest>` document and read the device PUID and `SPLicenseBlock` out of the response.
//!
//! This machine submits no hardware fingerprint — the SMBIOS components are marked unavailable, the
//! disk serial is a single zero byte, and the two CLEP challenge blobs are the fixed all-zero-input
//! constants — because a Mac exposes none of the Windows fields.

use rand::Rng;
use rand::distr::Alphanumeric;
use serde::{Deserialize, Serialize};

use super::consts::{CLEP_CHALLENGE_V2, CLEP_CHALLENGE_V4, COMPONENT_UNAVAILABLE};

#[derive(Debug, thiserror::Error)]
pub enum DeviceError {
    #[error("deviceaddcredential.srf response is not valid XML: {0}")]
    Xml(#[from] roxmltree::Error),
    #[error("deviceaddcredential.srf response is missing <{0}>")]
    Missing(&'static str),
    #[error("deviceaddcredential.srf reported Success=false")]
    NotSuccessful,
}

/// The randomly generated account credentials of a device registration. Both are sent in the clear
/// in the request; only the membername is needed again (it signs the device-ticket request).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DeviceCredential {
    pub membername: String,
    pub password: String,
    pub puid: String,
    pub hw_device_id: String,
    pub device_id: String,
    /// The base64 `SPLicenseBlock`, from which the device RSA key is derived.
    pub sp_license_block: String,
}

/// Fresh random credentials: the membername is the two characters `02` and 14 random alphanumerics
/// (16 total); the password is 20 random alphanumerics.
pub fn random_credentials() -> (String, String) {
    let mut rng = rand::rng();
    let alnum = |rng: &mut rand::rngs::ThreadRng, n: usize| -> String {
        (0..n).map(|_| rng.sample(Alphanumeric) as char).collect()
    };

    let membername = format!("02{}", alnum(&mut rng, 14));
    let password = alnum(&mut rng, 20);
    (membername, password)
}

/// The `<DeviceAddRequest>` document for `membername`/`password`. No XML declaration and no SOAP
/// envelope, matching what the account service expects from the token broker.
pub fn device_add_request(membername: &str, password: &str) -> String {
    // One component sent as unavailable.
    let err = |id: &str| format!("<Component name=\"{id}\" error=\"{COMPONENT_UNAVAILABLE}\"/>");
    // One component with a base64 value.
    let val = |id: &str, b64: &str| format!("<Component name=\"{id}\">{b64}</Component>");

    let components = [
        err("4100"),             // SMBIOS board version — unavailable on a Mac
        err("4101"),             // SMBIOS serial — unavailable
        err("4102"),             // SMBIOS UUID — unavailable
        val("4113", "AA=="),     // disk serial subset — a single zero byte
        val("4145", "AQAAAA=="), // chassis type
        err("4160"),
        err("4161"),
        val("8196", CLEP_CHALLENGE_V2), // CLEP challenge blob V2
        val("8197", CLEP_CHALLENGE_V4), // CLEP challenge blob V4
    ]
    .concat();

    format!(
        "<DeviceAddRequest>\
<ClientInfo name=\"IDCRL\" version=\"1.0\"><BinaryVersion>55</BinaryVersion></ClientInfo>\
<Authentication><Membername>{membername}</Membername><Password>{password}</Password></Authentication>\
<DeviceInfo Id=\"DeviceInfo\">{components}</DeviceInfo>\
</DeviceAddRequest>"
    )
}

/// Parses a `<DeviceAddResponse>` into the parts the credential keeps.
///
/// Element names are matched case-insensitively against the local names, so a `DeviceID`/`DeviceId`
/// spelling difference does not break it.
pub fn parse_device_add_response(
    body: &str,
    membername: &str,
    password: &str,
) -> Result<DeviceCredential, DeviceError> {
    let doc = roxmltree::Document::parse(body)?;
    let root = doc.root_element();

    // Success is an attribute on the root; absent is treated as success only if the fields are there.
    if let Some(success) = attr_ci(root, "Success") {
        if success.eq_ignore_ascii_case("false") || success == "0" {
            return Err(DeviceError::NotSuccessful);
        }
    }

    let text_of = |name: &'static str| -> Option<String> {
        doc.descendants()
            .find(|n| n.is_element() && n.tag_name().name().eq_ignore_ascii_case(name))
            .and_then(|n| n.text())
            .map(|t| t.trim().to_string())
            .filter(|s| !s.is_empty())
    };

    let puid = text_of("puid").ok_or(DeviceError::Missing("puid"))?;
    let sp_license_block = text_of("SPLicenseBlock").ok_or(DeviceError::Missing("SPLicenseBlock"))?;
    let hw_device_id = text_of("HWDeviceID").unwrap_or_default();
    // device_id lives inside a Binding element; take the first DeviceID/DeviceId text found.
    let device_id = doc
        .descendants()
        .find(|n| n.is_element() && n.tag_name().name().eq_ignore_ascii_case("deviceid"))
        .and_then(|n| n.text())
        .map(|t| t.trim().to_string())
        .unwrap_or_default();

    Ok(DeviceCredential {
        membername: membername.to_string(),
        password: password.to_string(),
        puid,
        hw_device_id,
        device_id,
        sp_license_block,
    })
}

fn attr_ci<'a>(node: roxmltree::Node<'a, 'a>, name: &str) -> Option<&'a str> {
    node.attributes().find(|a| a.name().eq_ignore_ascii_case(name)).map(|a| a.value())
}

#[cfg(test)]
mod tests;

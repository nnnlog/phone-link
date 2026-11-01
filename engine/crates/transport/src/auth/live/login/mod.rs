//! The interactive user sign-in result. The GUI hosts the `InlineLogin.srf` page in a web view and
//! hands back the JSON the page posts; this module reads the fields the exchange needs out of it.

use serde::{Deserialize, Serialize};

/// The signed-in user's tokens and identity, as the login page reports them (`DAProperty`).
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
pub struct DaProperty {
    #[serde(rename = "sDAToken")]
    pub da_token: String,
    #[serde(rename = "sDAStartTime", default)]
    pub start_time: String,
    #[serde(rename = "sDAExpires", default)]
    pub expires: String,
    #[serde(rename = "sSTSInlineFlowToken", default)]
    pub inline_flow_token: String,
    #[serde(rename = "sSigninName", default)]
    pub sign_in_name: String,
    /// The account PUID (`K`): the stable identifier the port keys the account on.
    #[serde(rename = "K")]
    pub puid: String,
}

#[derive(Debug, thiserror::Error)]
pub enum LoginError {
    #[error("the sign-in result is not valid JSON: {0}")]
    Json(#[from] serde_json::Error),
    #[error("the sign-in result carries no DAProperty (no sDAToken/K)")]
    NoDaProperty,
}

/// Reads a [`DaProperty`] out of the JSON the web view posted.
///
/// The page may post the property at the top level or nested inside a `ServerData` wrapper, so the
/// first object that carries both an `sDAToken` and a `K` is used.
pub fn parse_da_property(json: &str) -> Result<DaProperty, LoginError> {
    let value: serde_json::Value = serde_json::from_str(json)?;
    find_da_property(&value).ok_or(LoginError::NoDaProperty)
}

fn find_da_property(value: &serde_json::Value) -> Option<DaProperty> {
    if let serde_json::Value::Object(map) = value {
        if map.contains_key("sDAToken") && map.contains_key("K") {
            if let Ok(prop) = serde_json::from_value::<DaProperty>(value.clone()) {
                return Some(prop);
            }
        }

        for v in map.values() {
            if let Some(found) = find_da_property(v) {
                return Some(found);
            }
        }
    }
    None
}

#[cfg(test)]
mod tests;

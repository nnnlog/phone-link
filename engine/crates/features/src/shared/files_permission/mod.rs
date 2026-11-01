//! The phone's `FILES` permission, which file transfer and phone files both request over
//! `/DeviceResourceManager` and read from the same `PermissionsResponseMessage`.

use prost::Message;

use phonelink_proto::permissions::v1::{
    FeaturePermissions, PermissionsRequestMessage, PermissionsRequestStatus, PermissionsResponseMessage,
};
use phonelink_transport::platform::DrmSendOutcome;

/// `PermissionsRequestMessage { feature_permissions: [FILES] }`.
pub fn request() -> Vec<u8> {
    PermissionsRequestMessage { feature_permissions: vec![FeaturePermissions::Files as i32] }.encode_to_vec()
}

/// Whether a publication or reply grants `FILES`.
pub fn grants_files(reply: &PermissionsResponseMessage) -> bool {
    reply.feature_permissions.contains(&(FeaturePermissions::Files as i32))
}

/// The result of a permission request.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PermissionResult {
    /// Accepted, with `FILES` among the granted permissions.
    Granted,
    /// Accepted without `FILES`: the phone is prompting.
    Required,
    /// The reply carried another `request_status`.
    RequestStatus(i32),
    /// The reply's body does not parse.
    ParseError,
    /// The request did not get a successful reply.
    ResponseStatus(DrmSendOutcome),
}

impl PermissionResult {
    /// Reads the outcome of a permission request.
    pub fn from_outcome(outcome: DrmSendOutcome) -> Self {
        let DrmSendOutcome::Success { payload } = outcome else {
            return Self::ResponseStatus(outcome);
        };
        let Ok(reply) = PermissionsResponseMessage::decode(payload.as_slice()) else {
            return Self::ParseError;
        };
        if reply.request_status != PermissionsRequestStatus::Accepted as i32 {
            return Self::RequestStatus(reply.request_status);
        }

        if grants_files(&reply) { Self::Granted } else { Self::Required }
    }
}

#[cfg(test)]
mod tests;

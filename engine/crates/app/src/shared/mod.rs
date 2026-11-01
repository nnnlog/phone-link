//! The App messages that belong to no single feature: the `heartbeat`, `permissions`, `deviceQuery`
//! and `phoneCommand` processors, and the `/permission` request builder.

pub mod remoting_uri;

pub use remoting_uri::RemotingUri;

use async_trait::async_trait;
use tracing::{debug, warn};

use phonelink_core::codec::valueset::{Value, ValueSet};
use phonelink_core::types::TraceContext;

use super::consts::{content_type, key, literal, permission, result_code, route};
use super::processor::{
    ContentProcessor, ContentProcessorFactory, InboundContext, Processed, Rejection, simple_answer,
};
use super::request::{AppRequest, RequestKind, RequestOptions};
use super::{AppEvent, AppLayer};

/// The built-in factories, by catalog `contentType`.
pub(crate) fn builtin_factories() -> Vec<(&'static str, std::sync::Arc<dyn ContentProcessorFactory>)> {
    vec![
        (content_type::HEARTBEAT, std::sync::Arc::new(HeartbeatFactory)),
        (content_type::PERMISSIONS, std::sync::Arc::new(PermissionsFactory)),
        (content_type::DEVICE_QUERY, std::sync::Arc::new(DeviceQueryFactory)),
        (content_type::PHONE_COMMAND, std::sync::Arc::new(PhoneCommandFactory)),
    ]
}

// ------------------------------------------------------------------------------------------------
// heartbeat
// ------------------------------------------------------------------------------------------------

/// `heartbeat` (phone → PC): `{ result: Int32 0 }`; a metadata message is rejected.
struct HeartbeatFactory;

impl ContentProcessorFactory for HeartbeatFactory {
    fn create(
        &self,
        _ctx: &InboundContext<'_>,
        _payload: &ValueSet,
    ) -> Result<Option<Box<dyn ContentProcessor>>, Rejection> {
        Ok(Some(Box::new(Heartbeat)))
    }
}

struct Heartbeat;

#[async_trait]
impl ContentProcessor for Heartbeat {
    async fn content(&mut self, _ctx: &InboundContext<'_>, _payload: &ValueSet) -> Processed {
        Processed::completed(simple_answer(result_code::SUCCESS))
    }
}

// ------------------------------------------------------------------------------------------------
// permissions
// ------------------------------------------------------------------------------------------------

/// `permissions` (phone → PC). `itemCount` (`Int32`, default `0`) is read from the message that
/// creates the processor; the transaction completes when it equals the pair count of the last
/// valid content message.
struct PermissionsFactory;

impl ContentProcessorFactory for PermissionsFactory {
    fn create(
        &self,
        ctx: &InboundContext<'_>,
        payload: &ValueSet,
    ) -> Result<Option<Box<dyn ContentProcessor>>, Rejection> {
        let expected = match payload.get(key::ITEM_COUNT) {
            None => 0,
            Some(Value::Int32(n)) => *n,
            Some(_) => return Err(Rejection::new("permissions itemCount is not an Int32")),
        };

        if ctx.correlation_vector(payload).is_none() {
            warn!("permissions without correlationVector; answered without one");
        }
        Ok(Some(Box::new(Permissions { expected, count: 0 })))
    }
}

struct Permissions {
    expected: i32,
    count: i64,
}

impl Permissions {
    fn is_complete(&self) -> bool {
        i64::from(self.expected) == self.count
    }
}

#[async_trait]
impl ContentProcessor for Permissions {
    async fn metadata(&mut self, ctx: &InboundContext<'_>, payload: &ValueSet) -> Processed {
        let mut answer = ValueSet::new();
        if let Some(cv) = ctx.correlation_vector(payload) {
            answer.insert(key::CORRELATION_VECTOR, cv.to_string());
        }
        answer.insert(key::CONTENT_TYPE, literal::PERMISSIONS_ANSWER);
        answer.insert(key::CONTRACT_VERSION, "1.0");
        answer.insert(key::ITEM_COUNT, 0i32);
        // Sent as `Int32`.
        answer.insert(key::RESULT, if self.expected == 0 { result_code::SUCCESS } else { result_code::FAILURE });
        Processed::Answer { answer, completed: self.is_complete() }
    }

    async fn content(&mut self, ctx: &InboundContext<'_>, payload: &ValueSet) -> Processed {
        let types = payload.i32_array(key::PERMISSION_TYPES);
        let states = payload.i32_array(key::PERMISSION_STATES);

        let code = match (types, states) {
            (Some(types), Some(states)) if types.len() == states.len() => {
                ctx.app.update_record_for(ctx.phone, |r| {
                    for (t, s) in types.iter().zip(states) {
                        r.set_permission(*t as u32, *s == 0);
                    }
                });
                self.count = types.len() as i64;
                result_code::SUCCESS
            }
            _ => {
                warn!("permissions content without two Int32Arrays of equal length; answered 14");
                result_code::INVALID_REQUEST
            }
        };

        Processed::Answer { answer: simple_answer(code), completed: self.is_complete() }
    }
}

// ------------------------------------------------------------------------------------------------
// deviceQuery
// ------------------------------------------------------------------------------------------------

/// `deviceQuery` (phone → PC): unexpected; both phases answered with an empty `ValueSet`.
struct DeviceQueryFactory;

impl ContentProcessorFactory for DeviceQueryFactory {
    fn create(
        &self,
        _ctx: &InboundContext<'_>,
        _payload: &ValueSet,
    ) -> Result<Option<Box<dyn ContentProcessor>>, Rejection> {
        Ok(Some(Box::new(DeviceQuery)))
    }
}

struct DeviceQuery;

#[async_trait]
impl ContentProcessor for DeviceQuery {
    async fn metadata(&mut self, _ctx: &InboundContext<'_>, _payload: &ValueSet) -> Processed {
        unexpected_device_query(true)
    }

    async fn content(&mut self, _ctx: &InboundContext<'_>, _payload: &ValueSet) -> Processed {
        unexpected_device_query(false)
    }
}

/// A `deviceQuery` in either phase: logged, answered with an empty `ValueSet`.
fn unexpected_device_query(is_metadata: bool) -> Processed {
    warn!(is_metadata, "unexpected deviceQuery; answered empty");
    Processed::completed(ValueSet::new())
}

// ------------------------------------------------------------------------------------------------
// phoneCommand
// ------------------------------------------------------------------------------------------------

/// `phoneCommandType` values.
pub mod phone_command {
    pub const SYNC_OVER_METERED_CONNECTION_ENABLED: i32 = 0;
    pub const LAUNCH_URI: i32 = 1;
    pub const SYNC_DISABLED: i32 = 2;
    pub const HOTSPOT_PERMISSIONS_SETTING_UPDATE: i32 = 3;
    pub const POST_NOTIFICATION_PERMISSION_USAGE: i32 = 4;
    pub const TFL_CONTACT_SYNC_CONSENT_GRANTED: i32 = 5;
    pub const CAMERA_PERMISSION_USAGE: i32 = 6;
    pub const NOTIFICATIONS_SENSITIVE_PERMISSION_USAGE: i32 = 7;
}

/// An absolute URI with scheme `ms-phone` (case-insensitive): `ms-phone:` followed by characters
/// a URI may hold (no whitespace or control characters).
pub fn is_ms_phone_uri(uri: &str) -> bool {
    match uri.split_once(':') {
        Some((scheme, _)) => {
            scheme.eq_ignore_ascii_case("ms-phone") && !uri.chars().any(|c| c.is_whitespace() || c.is_control())
        }
        None => false,
    }
}

/// `phoneCommand` (phone → PC). `correlationVector` is required and every key it reads must have
/// its stated type; type 0 is applied when the processor is created, so in either phase.
struct PhoneCommandFactory;

impl ContentProcessorFactory for PhoneCommandFactory {
    fn create(
        &self,
        ctx: &InboundContext<'_>,
        payload: &ValueSet,
    ) -> Result<Option<Box<dyn ContentProcessor>>, Rejection> {
        let correlation_vector = match payload.get(key::CORRELATION_VECTOR) {
            Some(Value::String(cv)) => cv.clone(),
            Some(_) => {
                return Err(Rejection::new("phoneCommand correlationVector is not a String"));
            }
            None => return Err(Rejection::new("phoneCommand without correlationVector")),
        };

        let command_type = match payload.get(key::PHONE_COMMAND_TYPE) {
            None => None,
            Some(Value::Int32(t)) => Some(*t),
            Some(_) => return Err(Rejection::new("phoneCommandType is not an Int32")),
        };

        if command_type == Some(phone_command::SYNC_OVER_METERED_CONNECTION_ENABLED) {
            let enabled = match payload.get(key::SYNC_OVER_METERED_CONNECTION) {
                None => true,
                Some(Value::Bool(b)) => *b,
                Some(_) => return Err(Rejection::new("syncOverMeteredConnection is not a Bool")),
            };
            ctx.app.update_record_for(ctx.phone, |r| {
                r.status_keys.sync_over_metered_connection = Some(enabled);
            });
        }

        Ok(Some(Box::new(PhoneCommand { correlation_vector, command_type })))
    }
}

struct PhoneCommand {
    correlation_vector: String,
    command_type: Option<i32>,
}

fn optional_bool(payload: &ValueSet, k: &str) -> Result<Option<bool>, Rejection> {
    payload.read_checked(k).map_err(|_| Rejection::new("phoneCommand Bool key of another type"))
}

/// `Ok(Some(granted))` for the two known scenario strings, `Ok(None)` for an absent key or another
/// string; `Err` for a key that is not a `String`.
fn scenario(payload: &ValueSet, k: &str, granted: &str, denied: &str) -> Result<Option<bool>, Rejection> {
    match payload.get(k) {
        None => Ok(None),
        Some(Value::String(s)) if s == granted => Ok(Some(true)),
        Some(Value::String(s)) if s == denied => Ok(Some(false)),
        Some(Value::String(_)) => Ok(None),
        Some(_) => Err(Rejection::new("phoneCommand scenario key is not a String")),
    }
}

impl PhoneCommand {
    /// Validates, then applies the command; the content answer's `result`.
    fn run(&self, ctx: &InboundContext<'_>, payload: &ValueSet) -> Result<i32, Rejection> {
        let Some(t) = self.command_type else {
            debug!("phoneCommand without phoneCommandType; answered 0, nothing applied");
            return Ok(result_code::SUCCESS);
        };
        run_command(ctx, payload, t)
    }
}

/// Validates, then applies a command of type `t`; the content answer's `result`.
fn run_command(ctx: &InboundContext<'_>, payload: &ValueSet, t: i32) -> Result<i32, Rejection> {
    use phone_command as pc;
    match t {
        pc::SYNC_OVER_METERED_CONNECTION_ENABLED => {} // applied at creation
        pc::LAUNCH_URI => return launch_uri(ctx, payload),
        pc::SYNC_DISABLED => {
            ctx.app.update_record_for(ctx.phone, |r| r.all_features_disabled = true);
        }
        pc::HOTSPOT_PERMISSIONS_SETTING_UPDATE => apply_hotspot_permissions(ctx, payload)?,
        pc::POST_NOTIFICATION_PERMISSION_USAGE => apply_post_notification_usage(ctx, payload)?,
        pc::TFL_CONTACT_SYNC_CONSENT_GRANTED => {
            debug!("phoneCommand TflContactSyncConsentGranted")
        }
        pc::CAMERA_PERMISSION_USAGE => debug!("phoneCommand CameraPermissionUsage"),
        pc::NOTIFICATIONS_SENSITIVE_PERMISSION_USAGE => apply_notifications_sensitive_usage(ctx, payload)?,
        other => warn!(phone_command_type = other, "phoneCommand of an unknown type; answered 0, nothing applied"),
    }
    Ok(result_code::SUCCESS)
}

/// `LaunchUri`: an absolute `ms-phone` URI launches what a remoting URI names (through screen
/// mirroring) or brings the window forward; anything else answers `1`.
fn launch_uri(ctx: &InboundContext<'_>, payload: &ValueSet) -> Result<i32, Rejection> {
    match payload.get(key::LAUNCH_URI) {
        Some(Value::String(uri)) if is_ms_phone_uri(uri) => match RemotingUri::parse(uri) {
            // Inbound App messages come only from the selected phone, so a launch on another one
            // would need a selection change the port does not take from inbound traffic.
            Some(remoting)
                if remoting
                    .device_id
                    .as_deref()
                    .is_some_and(|d| !d.is_empty() && !d.eq_ignore_ascii_case(&ctx.phone.id)) =>
            {
                warn!("phoneCommand LaunchUri for another phone; answered 0, nothing launched");
            }
            Some(remoting) => ctx.app.emit(AppEvent::RemotingLaunch { phone: ctx.phone.clone(), uri: remoting }),
            None => ctx.app.emit(AppEvent::BringWindowForward { uri: uri.clone() }),
        },
        Some(Value::String(_)) | None => {
            warn!("phoneCommand LaunchUri without an absolute ms-phone URI; answered 1");
            return Ok(result_code::FAILURE);
        }
        Some(_) => return Err(Rejection::new("phoneCommand launchURI is not a String")),
    }
    Ok(result_code::SUCCESS)
}

/// `HotspotPermissionsSettingUpdate`: the consent and OEM Bluetooth flags into the hotspot record.
fn apply_hotspot_permissions(ctx: &InboundContext<'_>, payload: &ValueSet) -> Result<(), Rejection> {
    let consent = optional_bool(payload, key::HOTSPOT_USER_CONSENT)?;
    let oem_permission = optional_bool(payload, key::OEM_BLUETOOTH_PERMISSION)?;
    let oem_rfcomm = optional_bool(payload, key::OEM_BLUETOOTH_RFCOMM_ENABLED)?;
    ctx.app.update_record_for(ctx.phone, |r| r.hotspot.apply_permissions_update(consent, oem_permission, oem_rfcomm));
    Ok(())
}

/// `PostNotificationPermissionUsage`: the post-notifications permission from a known scenario.
fn apply_post_notification_usage(ctx: &InboundContext<'_>, payload: &ValueSet) -> Result<(), Rejection> {
    let granted = scenario(
        payload,
        key::POST_NOTIFICATION_SETTING_SCENARIO,
        "post_notification_permission_granted",
        "post_notification_permission_denied",
    )?;

    match granted {
        Some(granted) => {
            ctx.app.update_record_for(ctx.phone, |r| r.set_permission(permission::POST_NOTIFICATIONS, granted));
        }
        None => debug!("phoneCommand PostNotificationPermissionUsage without a known scenario"),
    }
    Ok(())
}

/// `NotificationsSensitivePermissionUsage`: the sensitive-notifications permission from a known
/// scenario.
fn apply_notifications_sensitive_usage(ctx: &InboundContext<'_>, payload: &ValueSet) -> Result<(), Rejection> {
    let granted = scenario(
        payload,
        key::NOTIFICATIONS_SENSITIVE_PERMISSION_SCENARIO,
        "notifications_sensitive_permission_granted",
        "notifications_sensitive_permission_denied",
    )?;

    match granted {
        Some(granted) => {
            ctx.app.update_record_for(ctx.phone, |r| r.set_permission(permission::NOTIFICATIONS_SENSITIVE, granted));
        }
        None => debug!("phoneCommand NotificationsSensitivePermissionUsage without a known scenario"),
    }
    Ok(())
}

#[async_trait]
impl ContentProcessor for PhoneCommand {
    async fn metadata(&mut self, ctx: &InboundContext<'_>, _payload: &ValueSet) -> Processed {
        Processed::completed(
            ValueSet::new()
                .with(key::CORRELATION_VECTOR, self.correlation_vector.clone())
                .with(key::CONTENT_TYPE, content_type::PHONE_COMMAND)
                .with(key::CONTRACT_VERSION, ctx.app.standard_keys.contract_version),
        )
    }

    async fn content(&mut self, ctx: &InboundContext<'_>, payload: &ValueSet) -> Processed {
        match self.run(ctx, payload) {
            Ok(code) => Processed::completed(simple_answer(code)),
            Err(rejection) => Processed::Rejected(rejection),
        }
    }
}

// ------------------------------------------------------------------------------------------------
// `/permission` request (PC → phone)
// ------------------------------------------------------------------------------------------------

impl AppLayer {
    /// A `/permission` KVP request asking the phone to show a permission prompt: `correlationVector`
    /// (the operation's correlation id — the request's trace correlation id),
    /// `contentType: "permission"`, `permissionType` (one `DevicePermissions` value, as `Int32`),
    /// `silent`, `permissionSessionId` (the app session id).
    /// `options` are the calling feature's (by default not user-initiated).
    pub fn permission_request(&self, permission_type: u32, silent: bool, options: RequestOptions) -> AppRequest {
        let trace = TraceContext::new_root(Vec::new());
        let payload = ValueSet::new()
            .with(key::CORRELATION_VECTOR, trace.correlation_id.clone())
            .with(key::CONTENT_TYPE, literal::PERMISSION)
            .with(key::PERMISSION_TYPE, permission_type as i32)
            .with(key::SILENT, silent)
            .with(key::PERMISSION_SESSION_ID, self.app_session_id.clone());

        AppRequest {
            route: route::PERMISSION.to_string(),
            kind: RequestKind::Kvp,
            payload,
            options,
            trace: Some(trace),
        }
    }
}

#[cfg(test)]
mod tests;

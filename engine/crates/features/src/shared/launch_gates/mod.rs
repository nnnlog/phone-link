//! The phone-apps gates: pure functions over the phone record. The phone apps read them, and so does
//! every feature that launches a phone app on the Mac (screen mirroring).
//!
//! | Gate | Rule |
//! |---|---|
//! | [`phone_apps_capable`] | capability bit `PhoneApps` (`0x8`) |
//! | [`recent_apps_supported`] | remoting `AppsOnWindowsSupported`, `RecentApps` and `RecentAppsMinimumDBVersion` |
//! | [`app_handoff_enabled`] | remoting `AppHandoff` |
//! | [`phone_screen_apps`] | contract version `≥ 3.91` and client model `1`, model `2` with revision `> 1`, or model `3` |
//! | [`remoting_supported`] | capability bit `Mirror` (`0x4`) and (client model or revision non-zero, or `inputInjectorAvailable`) |
//! | [`hosted_apps`] | remoting `AppsOnWindowsSupported`, one window per app allowed, contract `≥ 3.94`, Android `≥ 11` |
//! | [`apps_view_visible`] | remoting supported and phone-screen apps |
//!
//! The remoting capabilities are `0` until a connect reply or a `phoneApps` message reports them
//! ([`PhoneRecord::remoting_flags`]); the client model and revision are `0` until the device list or
//! a connect reply reports them.

use phonelink_app::PhoneRecord;
use phonelink_app::consts::capability;
use phonelink_core::config::AppConfig;

/// The phone's remoting capability flags.
pub mod remoting {
    pub const APPS_ON_WINDOWS_SUPPORTED: u32 = 0x1;
    pub const RECENT_APPS: u32 = 0x2;
    pub const APP_HANDOFF: u32 = 0x4;
    pub const SYNC_COORDINATOR_AVAILABLE: u32 = 0x8;
    pub const UNIFIED_STACK_SUPPORTED: u32 = 0x10;
    pub const ICE_ENABLED: u32 = 0x20;
    pub const SYNC_COORDINATOR_MIN_DB_VERSION: u32 = 0x40;
    pub const SYNC_COORDINATOR_IMPROVED_VERSION: u32 = 0x80;
    pub const NOTIFICATION_FALLBACK_RESULT: u32 = 0x100;
    pub const MULTIPLE_CONNECTIONS: u32 = 0x200;
    pub const SECURE_BLACK_SCREEN: u32 = 0x400;
    pub const RECENT_APPS_MINIMUM_DB_VERSION: u32 = 0x800;
    pub const PREVIEW_MODE_ENABLED: u32 = 0x1000;
    pub const AUDIO_SUPPORTED: u32 = 0x2000;
    pub const ADAPTIVE_LAYOUT_SUPPORTED: u32 = 0x4000;
    pub const VIRTUAL_DEVICE_STREAMING_SUPPORTED: u32 = 0x8000;
    pub const PLATFORM_NANO_FOR_PHONE_MIRRORING_SUPPORTED: u32 = 0x10000;
    pub const FREE_FORM_SUPPORTED: u32 = 0x20000;
}

/// `clientModelName`: which OEM integration the phone app runs with.
pub mod client_model {
    pub const DEFAULT: u32 = 0;
    /// A Samsung phone whose Link to Windows Service is present.
    pub const SG: u32 = 1;
    pub const MS1: u32 = 2;
    /// A phone whose maker implements the generic OEM integration.
    pub const GE: u32 = 3;
}

/// *Phone-screen apps* needs this contract version.
pub const PHONE_SCREEN_APPS_MIN_CONTRACT: f64 = 3.91;
/// *Hosted apps* needs this contract version.
pub const HOSTED_APPS_MIN_CONTRACT: f64 = 3.94;
/// *Hosted apps* needs this Android major version.
pub const HOSTED_APPS_MIN_ANDROID: i32 = 11;

/// The phone syncs its app list: capability bit `PhoneApps`.
pub fn phone_apps_capable(record: &PhoneRecord) -> bool {
    record.has_capability(capability::PHONE_APPS)
}

/// The recent-apps rule over remoting capability flags.
pub fn recent_apps_supported_by(flags: u32) -> bool {
    let all = remoting::APPS_ON_WINDOWS_SUPPORTED | remoting::RECENT_APPS | remoting::RECENT_APPS_MINIMUM_DB_VERSION;
    flags & all == all
}

/// *Recent apps supported*: the recent-apps sync, its connect keys and the recent list.
pub fn recent_apps_supported(record: &PhoneRecord) -> bool {
    recent_apps_supported_by(record.remoting_flags())
}

/// *App handoff enabled*: `configuration.AppHandoff`.
pub fn app_handoff_enabled(record: &PhoneRecord) -> bool {
    record.remoting_flags() & remoting::APP_HANDOFF != 0
}

fn model(record: &PhoneRecord) -> (u32, u32) {
    (record.client_model_name.unwrap_or(0), record.client_model_revision.unwrap_or(0))
}

/// *Phone-screen apps*: the phone runs apps for the Mac's screen — the Apps view and every launch.
pub fn phone_screen_apps(record: &PhoneRecord, app: &AppConfig) -> bool {
    let (name, revision) = model(record);
    phone_screen_apps_of(record.contract_version(app), name, revision)
}

/// [`phone_screen_apps`] over the values it reads.
pub fn phone_screen_apps_of(contract_version: f64, client_model: u32, revision: u32) -> bool {
    let model_ok = client_model == client_model::SG
        || (client_model == client_model::MS1 && revision > 1)
        || client_model == client_model::GE;
    contract_version >= PHONE_SCREEN_APPS_MIN_CONTRACT && model_ok
}

/// *Remoting supported*: the phone can show its screen on the Mac — the Apps view, deep links and the
/// notification launch.
pub fn remoting_supported(record: &PhoneRecord) -> bool {
    let (name, revision) = model(record);
    remoting_supported_of(
        record.has_capability(capability::MIRROR),
        name,
        revision,
        record.input_injector_available == Some(true),
    )
}

/// [`remoting_supported`] over the values it reads.
pub fn remoting_supported_of(mirror_capability: bool, client_model: u32, revision: u32, input_injector: bool) -> bool {
    mirror_capability && (client_model != 0 || revision != 0 || input_injector)
}

/// *Hosted apps*: each phone app opens in a window of its own (else the one phone-screen window).
/// `separate_window` is the PC's "one window per app" switch turned off when `true`.
pub fn hosted_apps(record: &PhoneRecord, app: &AppConfig, separate_window: bool) -> bool {
    hosted_apps_of(
        record.remoting_flags(),
        record.contract_version(app),
        record.android_major_version(),
        separate_window,
    )
}

/// [`hosted_apps`] over the values it reads.
pub fn hosted_apps_of(remoting: u32, contract_version: f64, android_major: Option<i32>, separate_window: bool) -> bool {
    remoting & remoting::APPS_ON_WINDOWS_SUPPORTED != 0
        && !separate_window
        && contract_version >= HOSTED_APPS_MIN_CONTRACT
        && android_major.is_some_and(|v| v >= HOSTED_APPS_MIN_ANDROID)
}

/// The Apps view shows: remoting supported and phone-screen apps.
pub fn apps_view_visible(record: &PhoneRecord, app: &AppConfig) -> bool {
    remoting_supported(record) && phone_screen_apps(record, app)
}

#[cfg(test)]
mod tests;

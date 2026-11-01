//! What the phone's `connect` reply says about mirroring, and the gates derived from it.

use phonelink_app::consts::capability;
use phonelink_app::phone_record::PhoneRecord;
use phonelink_core::config::AppConfig;
use phonelink_features::shared::launch_gates::{self, remoting};

/// One app per window, the phone screen only, or nothing (`config.appsFeatureType`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum AppsFeatureType {
    Disabled = 0,
    PhoneScreenOnly = 1,
    MultipleApps = 2,
}

/// The phone values mirroring reads, from the selected phone's record.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PhoneValues {
    /// Capability bit `Mirror` of `registeredPermissionTypes`.
    pub mirror_capability: bool,
    pub remoting: u32,
    pub client_model: u32,
    pub client_revision: u32,
    pub input_injector: bool,
    pub contract_version: f64,
    /// The Android major version (the text of the OS version before its first `.`).
    pub os_major: Option<i32>,
}

impl PhoneValues {
    pub fn from_record(record: &PhoneRecord, config: &AppConfig) -> Self {
        Self {
            mirror_capability: record.has_capability(capability::MIRROR),
            remoting: record.remoting_flags(),
            client_model: record.client_model_name.unwrap_or(0),
            client_revision: record.client_model_revision.unwrap_or(0),
            input_injector: record.input_injector_available.unwrap_or(false),
            contract_version: record.contract_version(config),
            os_major: record.android_major_version(),
        }
    }

    fn has(&self, bits: u32) -> bool {
        self.remoting & bits == bits
    }

    /// Mirroring is offered at all.
    pub fn remoting_supported(&self) -> bool {
        launch_gates::remoting_supported_of(
            self.mirror_capability,
            self.client_model,
            self.client_revision,
            self.input_injector,
        )
    }

    /// One window per app.
    pub fn hosted_apps(&self, separate_window: bool) -> bool {
        launch_gates::hosted_apps_of(self.remoting, self.contract_version, self.os_major, separate_window)
    }

    /// The phone screen and app launches.
    pub fn phone_screen_apps(&self) -> bool {
        launch_gates::phone_screen_apps_of(self.contract_version, self.client_model, self.client_revision)
    }

    pub fn apps_feature_type(&self, separate_window: bool) -> AppsFeatureType {
        if self.hosted_apps(separate_window) {
            AppsFeatureType::MultipleApps
        } else if self.phone_screen_apps() {
            AppsFeatureType::PhoneScreenOnly
        } else {
            AppsFeatureType::Disabled
        }
    }

    pub fn ice_support(&self) -> bool {
        self.has(remoting::ICE_ENABLED) && self.contract_version >= 3.99
    }

    pub fn audio(&self) -> bool {
        self.has(remoting::AUDIO_SUPPORTED)
    }

    pub fn notification_fallback(&self) -> bool {
        self.has(remoting::NOTIFICATION_FALLBACK_RESULT)
    }

    pub fn adaptive_layout(&self) -> bool {
        self.has(remoting::ADAPTIVE_LAYOUT_SUPPORTED)
    }

    pub fn nearby(&self) -> bool {
        self.has(remoting::VIRTUAL_DEVICE_STREAMING_SUPPORTED)
    }

    /// The phone accepts a mirroring connection.
    pub fn accepts_connection(&self) -> bool {
        self.remoting & (remoting::APPS_ON_WINDOWS_SUPPORTED | remoting::UNIFIED_STACK_SUPPORTED) != 0
    }
}

#[cfg(test)]
mod tests;

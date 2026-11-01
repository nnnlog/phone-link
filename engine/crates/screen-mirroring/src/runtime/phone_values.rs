//! The phone values the gates read and the stored settings.

use super::Inner;
use crate::gates::{AppsFeatureType, PhoneValues};
use crate::session::Session;
use crate::store::{PackageData, Settings};
use crate::{MirroringError, MirroringStatus, PackageAudio, SessionView, store};
use phonelink_core::sync::MutexExt;
use phonelink_core::types::DcgClient;
use std::sync::atomic::{AtomicBool, Ordering};
use tracing::warn;

impl Inner {
    pub(super) fn phone(&self) -> Result<DcgClient, MirroringError> {
        self.env.app.selected_phone().ok_or(MirroringError::NoPhone)
    }

    pub(super) fn values(&self) -> Result<PhoneValues, MirroringError> {
        let record = self.env.app.phone_record().ok_or(MirroringError::NoPhone)?;
        Ok(PhoneValues::from_record(&record, &self.env.config.app))
    }

    /// One window per app is turned off on the PC — the phone apps' switch, which decides *hosted
    /// apps* for both features.
    pub(super) fn separate_window(&self) -> bool {
        self.env.config.features.phone_apps.separate_window
    }

    pub(super) fn apps_feature_type(&self) -> AppsFeatureType {
        self.values().map_or(AppsFeatureType::Disabled, |v| v.apps_feature_type(self.separate_window()))
    }

    pub(super) fn all_features_disabled(&self) -> bool {
        self.env.app.phone_record().is_some_and(|r| r.all_features_disabled)
    }

    pub(super) fn settings(&self) -> Settings {
        let read = || -> Result<Settings, MirroringError> {
            let db = self.store.get(&self.phone()?)?;
            Ok(store::load_settings(&db)?)
        };
        or_default(read(), &self.settings_unread, "settings")
    }

    pub(super) fn package(&self, package: &str) -> PackageData {
        let read = || -> Result<PackageData, MirroringError> {
            let db = self.store.get(&self.phone()?)?;
            Ok(store::load_package(&db, package)?)
        };
        or_default(read(), &self.package_unread, "package")
    }

    pub(super) fn save_package(&self, package: &str, f: impl FnOnce(&mut PackageData)) -> Result<(), MirroringError> {
        let db = self.store.get(&self.phone()?)?;
        let mut data = store::load_package(&db, package)?;
        f(&mut data);
        Ok(store::save_package(&db, package, &data)?)
    }

    pub(in super::super) fn status(&self) -> Result<MirroringStatus, MirroringError> {
        let values = self.values()?;
        let st = self.state.lock_or_panic();
        Ok(MirroringStatus {
            values,
            remoting_supported: values.remoting_supported(),
            phone_screen_apps: values.phone_screen_apps(),
            apps_feature_type: values.apps_feature_type(self.separate_window()),
            phone_state: st.phone,
            phone_error: st.phone_error,
            nearby: st.nearby.clone(),
            settings: self.settings(),
            permission_accepted: st.permission_status.as_deref() == Some("accept"),
        })
    }

    pub(in super::super) fn sessions(&self) -> Vec<SessionView> {
        let st = self.state.lock_or_panic();
        let mut v: Vec<&Session> = st.sessions.values().collect();
        v.sort_by_key(|s| s.order);

        v.into_iter()
            .map(|s| {
                let data = self.package(&s.package);
                let runtime = st.package_audio.get(&s.package).copied().unwrap_or_default();
                SessionView {
                    id: s.id.clone(),
                    package_name: s.package.clone(),
                    phone_screen: s.is_phone_screen(),
                    phone_initiated: !s.pc_initiated,
                    state: s.machine.state,
                    details: s.machine.details.clone(),
                    launched: s.machine.launched(),
                    video_ready: s.machine.video_ready(),
                    device_resolution: s.display.device_resolution(),
                    initial_device_resolution: s.display.initial_device_resolution,
                    orientation: s.display.orientation,
                    rotation: s.display.rotation(),
                    video_size: s.display.video,
                    content_area: s.display.content_area,
                    navigation_buttons: s.display.navigation_buttons(),
                    button_order: s.display.button_order.clone(),
                    recent_task_id: s.recent_task_id,
                    screen_layout: s.current_layout,
                    move_to_virtual_display: s.move_to_virtual_display.as_ref().map(|(_, t)| *t),
                    audio: PackageAudio { volume: data.volume, muted: data.muted, ..runtime },
                    audio_capture_allowed: s.audio_capture_allowed,
                    audio_button: s.shown_audio_button(),
                }
            })
            .collect()
    }
}

/// A stored record read by `read`, or its defaults: without a phone silently, else with one line
/// when reads of `record_kind` start failing (`failing` remembers it until one succeeds), because
/// the status and the window list read it on every refresh.
fn or_default<T: Default>(read: Result<T, MirroringError>, failing: &AtomicBool, record_kind: &'static str) -> T {
    match read {
        Ok(v) => {
            failing.store(false, Ordering::Relaxed);
            v
        }
        Err(MirroringError::NoPhone) => T::default(),
        Err(e) => {
            if !failing.swap(true, Ordering::Relaxed) {
                warn!(record_kind, error = %e, "mirroring store not read; defaults are used until a read succeeds");
            }
            T::default()
        }
    }
}

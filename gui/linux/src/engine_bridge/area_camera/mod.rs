//! The phone as a camera: its state, setting and controls, the camera control window, and following
//! the v4l2loopback device — the apps that start or stop reading it, and it appearing or going. The
//! Camera tile (`camera.use`) turns the camera on and off by hand, for a v4l2loopback module that does not
//! report its readers.
//!
//! `query` names: `camera`, `camera.*`. `perform` names: `camera.*`.

use std::pin::Pin;
use std::sync::Arc;

use cxx_qt::{CxxQtType, Threading};
use phonelink_connected_camera::EffectLabel;
use phonelink_engine::api::FeatureApiError;
use phonelink_engine::api::connected_camera::{
    ConnectedCameraChange, OemEffectLabel, PhoneCameraEffect, PhoneCameraFace,
};
use phonelink_engine::api::engine::Engine;
use serde_json::{Value, json};
use tracing::{debug, info, warn};

use super::EngineBridge;
use super::features::{Failure, Revision, fail_later};
use crate::camera::{self, Unavailable, camera as loopback};
use crate::l10n::translate;

/// What the camera part keeps between calls.
#[derive(Default)]
pub(super) struct State {
    /// The phone that stopped being the camera when this phone's camera was turned on, for a note in
    /// the setting.
    replaced_phone: Option<String>,
    /// Whether the setting's request for the phone's camera permission is running.
    requesting_permission: bool,
}

/// `value` (such as `ko_KR.UTF-8`, or a `LANGUAGE` list) as a BCP 47 tag; `None` for the C locale.
fn bcp47(value: &str) -> Option<String> {
    let first = value.split(':').next()?;
    let tag = first.split(['.', '@']).next()?;
    (!tag.is_empty() && tag != "C" && tag != "POSIX").then(|| tag.replace('_', "-"))
}

/// The locale the phone names its own effects in: the app shows Korean on a Korean system and English
/// on any other, so the effect names follow the system's locale when it is in the app's language, and
/// the app's language otherwise (`en` on a German system, for example), to match the rest of the window.
fn effect_name_locale(system: Option<&str>) -> String {
    let app = if system.is_some_and(|value| value.starts_with("ko")) { "ko" } else { "en" };
    match system.and_then(bcp47) {
        Some(tag) if tag.split('-').next() == Some(app) => tag,
        _ => app.to_owned(),
    }
}

/// The label of an effect of the list.
fn effect_label(effect: &PhoneCameraEffect) -> String {
    let key = match effect {
        PhoneCameraEffect::None => EffectLabel::NoFilter,
        PhoneCameraEffect::Automatic => EffectLabel::Automatic,
        PhoneCameraEffect::Bokeh => EffectLabel::Blur,
        PhoneCameraEffect::FaceRetouch => EffectLabel::FaceRetouch,
        PhoneCameraEffect::Hdr => EffectLabel::Hdr,
        PhoneCameraEffect::Night => EffectLabel::Night,
        PhoneCameraEffect::Oem { label: OemEffectLabel::Known { label }, .. } => *label,
        PhoneCameraEffect::Oem { label: OemEffectLabel::Text { text }, .. } => return text.clone(),
    };
    let english = match key {
        EffectLabel::NoFilter => "No Effect",
        EffectLabel::Automatic => "Automatic",
        EffectLabel::Blur => "Blur",
        EffectLabel::FaceRetouch => "Face Retouch",
        EffectLabel::Hdr => "HDR",
        EffectLabel::Night => "Night",
        EffectLabel::AutoFraming => "Auto-Framing",
        EffectLabel::SoftFocus => "Soft Focus",
        EffectLabel::Stabilization => "Stabilization",
    };
    translate(english).to_owned()
}

/// The v4l2loopback device the camera is written to, or why there is none.
fn device_json(device: Result<std::path::PathBuf, Unavailable>) -> Value {
    match device {
        Ok(node) => json!({ "path": node.to_string_lossy(), "problem": null }),
        Err(Unavailable::NoLoopback) => json!({ "path": null, "problem": "noLoopback" }),
        Err(Unavailable::Unlabeled) => json!({ "path": null, "problem": "unlabeled" }),
    }
}

impl EngineBridge {
    fn camera_status(&self, engine: &Arc<Engine>) -> Value {
        let locale = effect_name_locale(crate::l10n::messages_locale().as_deref());
        let Ok(status) = engine.clone().connected_camera().status(locale) else { return Value::Null };
        json!({
            "available": status.mac_available,
            "capable": status.capable,
            "supported": status.supported,
            "enabled": status.camera_enabled,
            "registered": status.registered,
            "registrationFailed": status.registration_failed,
            "permissionRequired": status.permission_required,
            "cameraPermission": status.camera_permission,
            "state": format!("{:?}", status.state),
            "error": format!("{:?}", status.error),
            "errorAction": format!("{:?}", status.error_action),
            "face": status.face.map(|face| format!("{face:?}")),
            "effects": status.effects.iter().map(effect_label).collect::<Vec<_>>(),
            "selectedEffect": status.selected_effect,
            "inUse": loopback().in_use(),
            "controlWindow": status.control_window,
            "batteryLevel": status.battery_level,
            "charging": status.is_charging,
            "lowNetwork": status.low_network,
            "qrCodeUri": status.qr_code_uri,
            "replacedPhone": self.areas.camera.replaced_phone,
            "requestingPermission": self.areas.camera.requesting_permission,
        })
    }

    pub(super) fn query_camera(&self, engine: &Arc<Engine>, name: &str, _arg: &str) -> Option<Value> {
        match name {
            "camera" => Some(self.camera_status(engine)),
            // Known with no phone, unlike the camera's status.
            "camera.device" => Some(device_json(camera::device_node())),
            _ => None,
        }
    }

    pub(super) fn perform_camera(mut self: Pin<&mut Self>, engine: Arc<Engine>, name: &str, arg: &str) -> bool {
        if !name.starts_with("camera.") {
            return false;
        }

        let flag = arg == "true";
        let api = engine.clone().connected_camera();
        let done: Result<(), Failure> = match name {
            "camera.enable" => api.set_camera_enabled(flag).map_err(|e| Failure::setting(&e)),
            "camera.use" => match loopback().published() {
                Some(device) => {
                    loopback().set_in_use(flag);
                    api.client_changed(device, flag).map_err(|e| Failure::feature("Couldn't Turn On the Camera", &e))
                }
                None => Err(Failure::new(
                    "Couldn't Turn On the Camera",
                    translate("Your phone couldn't be added as a camera.").to_owned(),
                )),
            },
            "camera.pause" => api.pause().map_err(|e| Failure::feature("Couldn't Pause", &e)),
            "camera.resume" => api.resume().map_err(|e| Failure::feature("Couldn't Resume", &e)),
            "camera.switch" => {
                let face = match arg {
                    "Front" => Some(PhoneCameraFace::Front),
                    "Back" => Some(PhoneCameraFace::Back),
                    _ => None,
                };
                match face {
                    Some(face) => api.switch_camera(face).map_err(|e| Failure::feature("Couldn't Switch Cameras", &e)),
                    None => {
                        warn!(%arg, "camera.switch takes Front or Back");
                        Ok(())
                    }
                }
            }
            "camera.effect" => match arg.parse::<u32>() {
                Ok(index) => api.select_effect(index).map_err(|e| Failure::feature("Couldn't Apply the Effect", &e)),
                Err(_) => {
                    warn!(%arg, "camera.effect takes an effect's index");
                    Ok(())
                }
            },
            "camera.refresh" => api.refresh().map_err(|e| Failure::feature("Couldn't Reconnect", &e)),
            "camera.retry" => api.retry_registration().map_err(|e| Failure::feature("Couldn't Turn On the Camera", &e)),
            "camera.requestPermissions" => {
                api.request_permissions().map_err(|e| Failure::feature("Couldn't Ask Your Phone", &e))
            }
            "camera.requestPermission" => {
                self.as_mut().request_camera_permission(engine);
                Ok(())
            }
            "camera.closeControlWindow" => {
                api.close_control_window().map_err(|e| Failure::feature("Couldn't Close", &e))
            }
            "camera.closeError" => api.close_fatal_error().map_err(|e| Failure::feature("Couldn't Close", &e)),
            other => {
                warn!(action = other, "unknown action");
                Ok(())
            }
        };

        if let Err(failure) = done {
            self.as_mut().show_failure(failure);
        }
        self.bump(Revision::Settings);
        true
    }

    /// The setting's request for the camera permission: the phone shows its prompt; the setting shows
    /// a wait meanwhile.
    fn request_camera_permission(mut self: Pin<&mut Self>, engine: Arc<Engine>) {
        if self.areas.camera.requesting_permission {
            return;
        }

        self.as_mut().rust_mut().areas.camera.requesting_permission = true;
        let thread = self.qt_thread();
        self.runtime.spawn(async move {
            match engine.connected_camera().settings_request_permission().await {
                Ok(allowed) => info!(allowed, "the phone answered the camera permission request"),
                Err(e) => fail_later(&thread, Failure::feature("Couldn't Ask Your Phone", &e)),
            }
            let _ = thread.queue(|mut bridge| {
                bridge.as_mut().rust_mut().areas.camera.requesting_permission = false;
                bridge.bump(Revision::Settings);
            });
        });
    }

    pub(super) fn camera_changed(mut self: Pin<&mut Self>, change: ConnectedCameraChange) {
        if let ConnectedCameraChange::Replaced { name } = change {
            self.as_mut().rust_mut().areas.camera.replaced_phone = Some(name);
        }
        self.bump(Revision::Settings);
    }

    /// Apps started (`reading`) or stopped reading the published camera `device`: the engine is told, so
    /// that the phone streams only while an app reads the camera.
    fn camera_readers_changed(self: Pin<&mut Self>, device: String, reading: bool) {
        if let Some(engine) = self.engine.clone()
            && let Err(e) = engine.connected_camera().client_changed(device, reading)
        {
            match e {
                FeatureApiError::NotStarted | FeatureApiError::NoPhone => {
                    debug!("the camera's reader change was dropped; no phone session")
                }
                e => warn!(error = %e, "the camera's reader change was not sent"),
            }
        }
        self.bump(Revision::Settings);
    }

    /// Tells the engine whether the camera device is there now, and from then on, looking once a second,
    /// when it appears or goes and when apps start or stop reading the published camera (as the device
    /// reports it, else by the processes holding it open).
    pub(super) fn watch_camera_readers(self: Pin<&mut Self>) {
        static WATCHING: std::sync::Once = std::sync::Once::new();
        let Some(engine) = self.engine.clone() else { return };
        if let Err(e) = engine.clone().connected_camera().availability_changed() {
            info!(error = %e, "the camera's availability was not passed on");
        }

        let (thread, readers) = (self.qt_thread(), self.qt_thread());
        WATCHING.call_once(move || {
            camera::on_readers_changed(move |device, reading| {
                let _ = readers.queue(move |bridge| bridge.camera_readers_changed(device, reading));
            });
            std::thread::spawn(move || {
                let mut was_available = camera::device_node().is_ok();
                loop {
                    std::thread::sleep(std::time::Duration::from_secs(1));
                    let available = camera::device_node().is_ok();
                    if available != std::mem::replace(&mut was_available, available) {
                        info!(available, "the camera device came or went");
                        if let Err(e) = engine.clone().connected_camera().availability_changed() {
                            info!(error = %e, "the camera's availability was not passed on");
                        }
                        let _ = thread.queue(|bridge| bridge.bump(Revision::Settings));
                    }
                    loopback().scan_readers();
                }
            });
        });
    }
}

#[cfg(test)]
mod tests;

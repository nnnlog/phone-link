//! The engine's camera edge (`PhoneCamera`): the published cameras, each written to the loopback
//! device by an output of its own.

use std::collections::HashMap;
use std::fs::OpenOptions;
use std::sync::atomic::Ordering;
use std::sync::{Arc, Mutex, OnceLock};
use std::thread::JoinHandle;
use std::time::Instant;

use phonelink_connected_camera::{CameraPlaceholder, PhoneCamera};
use phonelink_core::sync::MutexExt;
use phonelink_engine::edge::EdgeError;
use tracing::{error, info, warn};

use super::Unavailable;
use super::device::loopback_device;
use super::frame::black;
use super::output::{Output, Shown, pace};
use super::v4l2::{set_format, subscribe_readers};
use crate::edges::unexpected;

/// A published camera's output and its pacing thread.
struct Publication {
    output: Arc<Output>,
    pacer: JoinHandle<()>,
}

impl Publication {
    /// Ends the pacing thread; the device closes with it.
    fn close(self) {
        self.output.close();
        if self.pacer.join().is_err() {
            error!("the camera's pacing thread panicked");
        }
    }
}

/// The app's one camera edge.
pub(crate) fn camera() -> &'static Arc<LoopbackCamera> {
    static CAMERA: OnceLock<Arc<LoopbackCamera>> = OnceLock::new();
    CAMERA.get_or_init(Arc::default)
}

#[derive(Default)]
pub(crate) struct LoopbackCamera {
    outputs: Mutex<HashMap<String, Publication>>,
    published: Mutex<Option<String>>,
    /// Why no device can be written to, as last logged. Every status read asks for availability,
    /// and the engine publishes nothing while the camera is unavailable, so the reason is logged
    /// there, once until it changes.
    unavailable: Mutex<Option<Unavailable>>,
}

impl LoopbackCamera {
    /// The device id of the published camera.
    pub(crate) fn published(&self) -> Option<String> {
        self.published.lock_or_recover().clone()
    }

    fn output(&self, device_id: &str) -> Option<Arc<Output>> {
        let outputs = self.outputs.lock_or_recover();
        outputs.get(device_id).map(|publication| publication.output.clone())
    }

    pub(super) fn published_output(&self) -> Option<Arc<Output>> {
        self.output(&self.published()?)
    }

    /// An app reads the published camera, as last told to the engine.
    pub(crate) fn in_use(&self) -> bool {
        self.published_output().is_some_and(|output| output.streaming.load(Ordering::SeqCst))
    }

    /// The Camera tile turned the published camera on or off: it is written to as while an app reads it.
    pub(crate) fn set_in_use(&self, in_use: bool) {
        if let Some(output) = self.published_output() {
            output.streaming.store(in_use, Ordering::SeqCst);
            output.wake();
        }
    }
}

impl PhoneCamera for LoopbackCamera {
    fn is_available(&self) -> bool {
        let choice = loopback_device();
        let mut logged = self.unavailable.lock_or_recover();
        match &choice {
            Ok(_) => *logged = None,
            Err(reason) if *logged != Some(*reason) => {
                info!(%reason, "the phone's camera is unavailable");
                *logged = Some(*reason);
            }
            Err(_) => {}
        }
        choice.is_ok()
    }

    fn publish(&self, device_id: String, _name: String) -> Result<(), EdgeError> {
        // Published again: the device is closed first, so that it has one writer.
        let replaced = self.outputs.lock_or_recover().remove(&device_id);
        if let Some(replaced) = replaced {
            replaced.close();
        }

        let choice = loopback_device().map_err(|reason| unexpected(reason.to_string()))?;
        let path = choice.node();
        if !choice.labeled {
            warn!(
                device = %path.display(),
                "no video device is labeled \"Phone Link\"; using the only v4l2loopback device"
            );
        }
        let device = OpenOptions::new()
            .write(true)
            .open(&path)
            .map_err(|e| unexpected(format!("{} did not open: {e}", path.display())))?;
        set_format(&device).map_err(|e| unexpected(format!("{} took no format: {e}", path.display())))?;
        let reports_readers = match subscribe_readers(&device) {
            Ok(()) => {
                info!("the camera's readers are followed through the device's client-usage events");
                true
            }
            Err(e) => {
                info!(error = %e, "the v4l2loopback module has no client-usage events; the processes holding the camera open are looked for once a second, and the Camera tile turns it on");
                false
            }
        };

        let output = Output::new(device_id.clone(), path, device, reports_readers)
            .map(Arc::new)
            .map_err(|e| unexpected(format!("the camera's pacing thread has no wake-up: {e}")))?;
        // Apps can open the device once it was written to.
        output.write(&black());
        let pacer = {
            let output = output.clone();
            std::thread::Builder::new()
                .name("camera".to_owned())
                .spawn(move || pace(output))
                .map_err(|e| unexpected(format!("the camera's pacing thread did not start: {e}")))?
        };

        let replaced = self.outputs.lock_or_recover().insert(device_id.clone(), Publication { output, pacer });
        if let Some(replaced) = replaced {
            replaced.close();
        }
        *self.published.lock_or_recover() = Some(device_id);
        Ok(())
    }

    fn remove(&self, device_id: String) {
        let removed = self.outputs.lock_or_recover().remove(&device_id);
        {
            let mut published = self.published.lock_or_recover();
            if published.as_deref() == Some(device_id.as_str()) {
                *published = None;
            }
        }
        if let Some(removed) = removed {
            removed.close();
        }
    }

    fn show_placeholder(&self, device_id: String, placeholder: CameraPlaceholder) {
        if let Some(output) = self.output(&device_id) {
            output.show(Shown::Placeholder(placeholder, Instant::now()));
        }
    }

    fn video_frame(&self, device_id: String, data: Vec<u8>, show: bool) -> bool {
        self.output(&device_id).is_some_and(|output| output.video(&data, show))
    }

    fn reset_decoder(&self, device_id: String) {
        if let Some(output) = self.output(&device_id) {
            output.reset_decoder();
        }
    }
}

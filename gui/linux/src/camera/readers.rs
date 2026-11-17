//! The apps reading the camera: the listener told when they start or stop, and the processes holding
//! the device open, looked for where the module does not report its readers.

use std::fs::{self, DirEntry};
use std::path::Path;
use std::sync::atomic::Ordering;
use std::sync::{Arc, Mutex};

use phonelink_core::sync::MutexExt;

use super::camera;
use super::loopback::LoopbackCamera;

/// Told with a published camera's device id when apps start (`true`) or stop reading it.
type ReadersListener = Arc<dyn Fn(String, bool) + Send + Sync>;

pub(super) fn readers_listener() -> &'static Mutex<Option<ReadersListener>> {
    static LISTENER: Mutex<Option<ReadersListener>> = Mutex::new(None);
    &LISTENER
}

/// Calls `listener` with a published camera's device id whenever apps start (`true`) or stop reading it:
/// at once for apps that already read the published camera.
pub(crate) fn on_readers_changed(listener: impl Fn(String, bool) + Send + Sync + 'static) {
    let listener: ReadersListener = Arc::new(listener);
    *readers_listener().lock_or_recover() = Some(listener.clone());
    if let Some(output) = camera().published_output()
        && output.streaming.load(Ordering::SeqCst)
    {
        listener(output.device_id.clone(), true);
    }
}

/// Another process holding `device` open: its name and process id.
fn other_reader(device: &Path) -> Option<String> {
    let own = std::process::id().to_string();
    let processes = fs::read_dir("/proc").ok()?;
    let pid = |process: &DirEntry| process.file_name().to_str().is_some_and(|n| n.bytes().all(|b| b.is_ascii_digit()));
    processes.flatten().filter(|process| pid(process) && process.file_name() != own.as_str()).find_map(|process| {
        let holds = fs::read_dir(process.path().join("fd"))
            .map(|fds| fds.flatten().any(|fd| fs::read_link(fd.path()).is_ok_and(|target| target == device)))
            .unwrap_or(false);
        holds.then(|| {
            let name = fs::read_to_string(process.path().join("comm")).unwrap_or_default();
            format!("{} ({})", name.trim(), process.file_name().to_string_lossy())
        })
    })
}

impl LoopbackCamera {
    /// Looks for another process holding the published device open, where the module does not report
    /// its readers, and reports when that changed from the last look of this publish and from what the
    /// engine was told.
    pub(crate) fn scan_readers(&self) {
        let Some(output) = self.published_output() else { return };
        if output.reports_readers.load(Ordering::SeqCst) {
            return;
        }

        let reading = other_reader(&output.path).is_some();
        let was_reading = output.scanned.swap(reading, Ordering::SeqCst);
        if reading == was_reading || reading == output.streaming.load(Ordering::SeqCst) {
            return;
        }
        output.report(reading);
    }
}

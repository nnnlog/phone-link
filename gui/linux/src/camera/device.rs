//! The video device the phone's camera is written to: the v4l2loopback device labeled `Phone Link`, or
//! the only one there is.

use std::fs;
use std::path::{Path, PathBuf};

const VIDEO4LINUX: &str = "/sys/class/video4linux";
/// The label the README has users give the v4l2loopback device (`card_label="Phone Link"`).
const DEVICE_LABEL: &str = "Phone Link";

/// A `videoN` device as sysfs lists it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct VideoDevice {
    pub(super) number: u32,
    /// Its `name` attribute, which for a v4l2loopback device is its `card_label`.
    pub(super) label: String,
    /// It sits under `/sys/devices/virtual`, as devices with no parent, v4l2loopback's among them, do.
    pub(super) loopback: bool,
}

/// The `videoN` devices under `class_dir` (laid out like `/sys/class/video4linux`), by number.
pub(super) fn video_devices(class_dir: &Path) -> Vec<VideoDevice> {
    let Ok(entries) = fs::read_dir(class_dir) else { return Vec::new() };
    let mut devices: Vec<VideoDevice> = entries
        .flatten()
        .filter_map(|entry| {
            let number = entry.file_name().to_str()?.strip_prefix("video")?.parse().ok()?;
            let label = fs::read_to_string(entry.path().join("name")).unwrap_or_default().trim().to_owned();
            let loopback =
                fs::canonicalize(entry.path()).is_ok_and(|path| path.to_string_lossy().contains("/virtual/"));
            Some(VideoDevice { number, label, loopback })
        })
        .collect();
    devices.sort_by_key(|device| device.number);
    devices
}

/// The device the phone's camera is written to.
#[derive(Debug, PartialEq, Eq)]
pub(super) struct Choice {
    pub(super) number: u32,
    /// It carries the label; otherwise it is the only loopback device there is.
    pub(super) labeled: bool,
}

impl Choice {
    pub(super) fn node(&self) -> PathBuf {
        PathBuf::from(format!("/dev/video{}", self.number))
    }
}

/// Why the phone's camera has no device to be written to.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub(crate) enum Unavailable {
    /// No v4l2loopback device is loaded.
    #[error("no v4l2loopback device; load one with card_label=\"{label}\"", label = DEVICE_LABEL)]
    NoLoopback,
    /// Several are, and none is labeled `Phone Link`.
    #[error(
        "several v4l2loopback devices and none labeled \"{label}\"; label one with card_label=\"{label}\"",
        label = DEVICE_LABEL
    )]
    Unlabeled,
}

/// The device labeled `Phone Link`, the lowest-numbered when several are; without one, the only
/// loopback device. Of several unlabeled loopback devices any could be another app's, so none is
/// picked.
pub(super) fn choose(devices: &[VideoDevice]) -> Result<Choice, Unavailable> {
    if let Some(device) = devices.iter().find(|device| device.label == DEVICE_LABEL) {
        return Ok(Choice { number: device.number, labeled: true });
    }

    let mut loopbacks = devices.iter().filter(|device| device.loopback);
    match (loopbacks.next(), loopbacks.next()) {
        (Some(only), None) => Ok(Choice { number: only.number, labeled: false }),
        (None, _) => Err(Unavailable::NoLoopback),
        (Some(_), Some(_)) => Err(Unavailable::Unlabeled),
    }
}

/// The device the phone's camera is written to among those sysfs lists, or why there is none.
pub(super) fn loopback_device() -> Result<Choice, Unavailable> {
    choose(&video_devices(Path::new(VIDEO4LINUX)))
}

/// The device node the phone's camera is written to, or why there is none.
pub(crate) fn device_node() -> Result<PathBuf, Unavailable> {
    loopback_device().map(|choice| choice.node())
}

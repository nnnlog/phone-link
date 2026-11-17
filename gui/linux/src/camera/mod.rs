//! The phone's camera on this computer: a v4l2loopback device that apps read like any camera.
//!
//! Each published camera has an output: the device opened for writing at 1280 × 720 I420, its H.264
//! decoder, and what it shows — the latest decoded picture or still image, black at first. Video and
//! still images only replace what it shows; a pacing thread writes it every 1/30 s while an app reads the
//! device, and steps the connecting animation every 200 ms, so the app gets a steady frame rate
//! whatever the phone sends.
//!
//! The apps reading the device are followed through v4l2loopback's client-usage event, which comes when
//! one starts or stops streaming, asked for anew with each publish. A module without that event has the
//! processes holding the device open looked for once a second instead (`scan_readers`), and the Camera
//! tile turns the camera on and off by hand. Each publish starts with no readers: apps already reading
//! the device are reported by its first event, or by the first look.

mod device;
mod frame;
mod loopback;
mod output;
mod placeholder;
mod readers;
#[cfg(test)]
mod tests;
mod v4l2;

pub(crate) use device::{Unavailable, device_node};
pub(crate) use loopback::camera;
pub(crate) use readers::on_readers_changed;

//! The V4L2 interface the camera's device is driven through, by ioctl: the output format, and
//! v4l2loopback's client-usage events.

use std::fs::File;
use std::os::fd::AsRawFd;

use tracing::debug;

use super::frame::{FRAME_BYTES, HEIGHT, WIDTH};

pub(super) const VIDIOC_S_FMT: libc::c_ulong = 0xC0D0_5605;
const V4L2_BUF_TYPE_VIDEO_OUTPUT: u32 = 2;
const V4L2_PIX_FMT_YUV420: u32 = 0x3231_5559;
const V4L2_FIELD_NONE: u32 = 1;

/// `_IOC(direction, 'V', number, size)`, a V4L2 ioctl's request.
pub(super) const fn video_ioctl(direction: libc::c_ulong, number: libc::c_ulong, size: usize) -> libc::c_ulong {
    (direction << 30) | ((size as libc::c_ulong) << 16) | ((b'V' as libc::c_ulong) << 8) | number
}

const IOC_WRITE: libc::c_ulong = 1;
const IOC_READ: libc::c_ulong = 2;
pub(super) const VIDIOC_DQEVENT: libc::c_ulong = video_ioctl(IOC_READ, 89, size_of::<V4l2Event>());
pub(super) const VIDIOC_SUBSCRIBE_EVENT: libc::c_ulong = video_ioctl(IOC_WRITE, 90, size_of::<V4l2EventSubscription>());
const V4L2_EVENT_PRIVATE_START: u32 = 0x0800_0000;
/// v4l2loopback's event that a capture stream started or stopped; its data begins with the count of
/// streaming readers.
pub(super) const V4L2_EVENT_PRI_CLIENT_USAGE: u32 = V4L2_EVENT_PRIVATE_START + 0x08E0_0000 + 1;
/// Ask for an event with the state at once, as well as for its changes.
const V4L2_EVENT_SUB_FL_SEND_INITIAL: u32 = 1;

/// `struct v4l2_format`.
#[repr(C, align(8))]
pub(super) struct V4l2Format([u32; 52]);

/// `struct v4l2_event_subscription`.
#[repr(C)]
#[derive(Default)]
pub(super) struct V4l2EventSubscription {
    kind: u32,
    id: u32,
    flags: u32,
    reserved: [u32; 5],
}

/// `struct v4l2_event` (with a 64-bit `struct timespec`).
#[repr(C)]
#[derive(Default)]
pub(super) struct V4l2Event {
    kind: u32,
    pub(super) data: V4l2EventData,
    /// The events still pending after this one.
    pub(super) pending: u32,
    sequence: u32,
    timestamp: [i64; 2],
    pub(super) id: u32,
    reserved: [u32; 8],
}

/// The event's data, a union of 64 bytes aligned for its 64-bit members.
#[repr(C, align(8))]
pub(super) struct V4l2EventData([u8; 64]);

impl Default for V4l2EventData {
    fn default() -> Self {
        Self([0; 64])
    }
}

/// Sets the output format of `device`: 1280 × 720 I420.
#[expect(unsafe_code, reason = "V4L2 ioctls have no safe wrapper in the dependencies")]
pub(super) fn set_format(device: &File) -> std::io::Result<()> {
    let mut format = V4l2Format([0; 52]);
    format.0[0] = V4L2_BUF_TYPE_VIDEO_OUTPUT;
    format.0[2] = WIDTH as u32;
    format.0[3] = HEIGHT as u32;
    format.0[4] = V4L2_PIX_FMT_YUV420;
    format.0[5] = V4L2_FIELD_NONE;
    format.0[6] = WIDTH as u32;
    format.0[7] = FRAME_BYTES as u32;
    let result = unsafe { libc::ioctl(device.as_raw_fd(), VIDIOC_S_FMT, &mut format) };
    if result < 0 { Err(std::io::Error::last_os_error()) } else { Ok(()) }
}

/// Asks `device` for v4l2loopback's client-usage events, the first at once with the readers there are;
/// an error from a module that has no such event.
#[expect(unsafe_code, reason = "V4L2 ioctls have no safe wrapper in the dependencies")]
pub(super) fn subscribe_readers(device: &File) -> std::io::Result<()> {
    let mut subscription = V4l2EventSubscription {
        kind: V4L2_EVENT_PRI_CLIENT_USAGE,
        flags: V4L2_EVENT_SUB_FL_SEND_INITIAL,
        ..V4l2EventSubscription::default()
    };
    let result = unsafe { libc::ioctl(device.as_raw_fd(), VIDIOC_SUBSCRIBE_EVENT, &mut subscription) };
    if result < 0 { Err(std::io::Error::last_os_error()) } else { Ok(()) }
}

/// The readers' count the pending events of `device` end with; `None` when none of them is a
/// client-usage event. Called only while an event is pending: the device is not opened non-blocking, so
/// asking without one would wait for one.
#[expect(unsafe_code, reason = "V4L2 ioctls have no safe wrapper in the dependencies")]
pub(super) fn dequeue_readers(device: &File) -> std::io::Result<Option<u32>> {
    let mut count = None;
    loop {
        let mut event = V4l2Event::default();
        let result = unsafe { libc::ioctl(device.as_raw_fd(), VIDIOC_DQEVENT, &mut event) };
        if result < 0 {
            return Err(std::io::Error::last_os_error());
        }

        if event.kind == V4L2_EVENT_PRI_CLIENT_USAGE {
            let [a, b, c, d, ..] = event.data.0;
            count = Some(u32::from_ne_bytes([a, b, c, d]));
        } else {
            debug!(event = event.kind, "the camera device sent an event it was not asked for");
        }
        if event.pending == 0 {
            return Ok(count);
        }
    }
}

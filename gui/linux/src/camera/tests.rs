//! The camera's unit tests: the device chosen, the V4L2 requests, frames from decoded pictures, the
//! pacing, and what an output shows.

use std::fs::{self, File};
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

use openh264::formats::YUVSource;
use phonelink_connected_camera::CameraPlaceholder;

use super::Unavailable;
use super::device::{Choice, VideoDevice, choose, video_devices};
use super::frame::{FRAME_BYTES, HEIGHT, WIDTH, black, copy_plane, frame_from};
use super::output::{FRAME_INTERVAL, Output, Shown, connecting_step, next_frame};
use super::v4l2::{
    V4L2_EVENT_PRI_CLIENT_USAGE, V4l2Event, V4l2EventSubscription, V4l2Format, VIDIOC_DQEVENT, VIDIOC_S_FMT,
    VIDIOC_SUBSCRIBE_EVENT, video_ioctl,
};

/// A temporary directory laid out like `/sys`, removed when the test ends.
struct Sysfs(PathBuf);

impl Sysfs {
    fn new() -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let root = std::env::temp_dir().join(format!(
            "phonelink-video4linux-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(root.join("class/video4linux")).unwrap();
        Self(root)
    }

    /// Adds `videoN` under the device directory `parent`, linked from the class directory as the
    /// kernel links it.
    fn device(&self, parent: &str, number: u32, label: &str) -> &Self {
        let device = self.0.join(parent).join(format!("video4linux/video{number}"));
        fs::create_dir_all(&device).unwrap();
        fs::write(device.join("name"), format!("{label}\n")).unwrap();
        std::os::unix::fs::symlink(&device, self.class_dir().join(format!("video{number}"))).unwrap();
        self
    }

    fn class_dir(&self) -> PathBuf {
        self.0.join("class/video4linux")
    }
}

impl Drop for Sysfs {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn device(number: u32, label: &str, loopback: bool) -> VideoDevice {
    VideoDevice { number, label: label.to_owned(), loopback }
}

#[test]
fn devices_are_listed_by_number_with_their_labels() {
    let sysfs = Sysfs::new();
    sysfs
        .device("devices/pci0000:00/0000:00:14.0/usb1/1-5/1-5:1.0", 0, "Integrated Camera")
        .device("devices/virtual", 10, "Phone Link")
        .device("devices/virtual", 2, "Dummy video device (0x0000)");
    assert_eq!(
        video_devices(&sysfs.class_dir()),
        [
            device(0, "Integrated Camera", false),
            device(2, "Dummy video device (0x0000)", true),
            device(10, "Phone Link", true)
        ]
    );
    assert_eq!(video_devices(&sysfs.0.join("missing")), []);
}

#[test]
fn the_labeled_device_is_chosen() {
    let devices = [device(2, "Dummy video device (0x0000)", true), device(10, "Phone Link", true)];
    assert_eq!(choose(&devices), Ok(Choice { number: 10, labeled: true }));
    assert_eq!(Choice { number: 10, labeled: true }.node(), Path::new("/dev/video10"));
}

#[test]
fn without_a_label_only_a_single_loopback_device_is_chosen() {
    let single = [device(0, "Integrated Camera", false), device(4, "Dummy video device (0x0000)", true)];
    assert_eq!(choose(&single), Ok(Choice { number: 4, labeled: false }));

    let several = [device(4, "OBS Virtual Camera", true), device(5, "Dummy video device (0x0001)", true)];
    assert_eq!(choose(&several), Err(Unavailable::Unlabeled));
    assert!(Unavailable::Unlabeled.to_string().contains("card_label=\"Phone Link\""));

    let none = [device(0, "Integrated Camera", false)];
    assert_eq!(choose(&none), Err(Unavailable::NoLoopback));
    assert!(Unavailable::NoLoopback.to_string().contains("card_label=\"Phone Link\""));
}

/// The requests and layouts of `linux/videodev2.h` on a 64-bit computer, and v4l2loopback's event.
#[cfg(target_pointer_width = "64")]
#[test]
fn the_event_requests_match_the_kernels() {
    assert_eq!(size_of::<V4l2Event>(), 136);
    assert_eq!(std::mem::offset_of!(V4l2Event, data), 8);
    assert_eq!(std::mem::offset_of!(V4l2Event, pending), 72);
    assert_eq!(std::mem::offset_of!(V4l2Event, id), 96);
    assert_eq!(size_of::<V4l2EventSubscription>(), 32);
    assert_eq!(VIDIOC_DQEVENT, 0x8088_5659);
    assert_eq!(VIDIOC_SUBSCRIBE_EVENT, 0x4020_565A);
    assert_eq!(video_ioctl(3, 5, size_of::<V4l2Format>()), VIDIOC_S_FMT);
    assert_eq!(V4L2_EVENT_PRI_CLIENT_USAGE, 0x10E0_0001);
}

/// A decoded picture: its planes with their strides.
struct Picture {
    size: (usize, usize),
    strides: (usize, usize, usize),
    planes: [Vec<u8>; 3],
}

impl Picture {
    /// A picture whose luma bytes are 1, 2, 3… row after row and whose chroma bytes are 200 and 50,
    /// each row padded with 0 to its stride.
    fn new(width: usize, height: usize, padding: usize) -> Self {
        let (chroma_width, chroma_height) = (width.div_ceil(2), height.div_ceil(2));
        let plane = |width: usize, rows: usize, value: &dyn Fn(usize) -> u8| {
            let mut plane = Vec::new();
            for row in 0..rows {
                plane.extend((0..width).map(|column| value(row * width + column)));
                plane.extend(std::iter::repeat_n(0, padding));
            }
            plane
        };
        Self {
            size: (width, height),
            strides: (width + padding, chroma_width + padding, chroma_width + padding),
            planes: [
                plane(width, height, &|index| (index % 250) as u8 + 1),
                plane(chroma_width, chroma_height, &|_| 200),
                plane(chroma_width, chroma_height, &|_| 50),
            ],
        }
    }
}

impl YUVSource for Picture {
    fn dimensions(&self) -> (usize, usize) {
        self.size
    }
    fn strides(&self) -> (usize, usize, usize) {
        self.strides
    }
    fn y(&self) -> &[u8] {
        &self.planes[0]
    }
    fn u(&self) -> &[u8] {
        &self.planes[1]
    }
    fn v(&self) -> &[u8] {
        &self.planes[2]
    }
}

fn luma(frame: &[u8], x: usize, y: usize) -> u8 {
    frame[y * WIDTH + x]
}

fn chroma(frame: &[u8], plane: usize, x: usize, y: usize) -> u8 {
    frame[WIDTH * HEIGHT + plane * WIDTH * HEIGHT / 4 + y * WIDTH / 2 + x]
}

#[test]
fn a_smaller_picture_is_copied_to_the_top_left_and_padded_with_black() {
    let frame = frame_from(&Picture::new(6, 4, 3));
    assert_eq!(frame.len(), FRAME_BYTES);
    assert_eq!([luma(&frame, 0, 0), luma(&frame, 5, 0), luma(&frame, 0, 1), luma(&frame, 5, 3)], [1, 6, 7, 24]);
    assert_eq!([luma(&frame, 6, 0), luma(&frame, 0, 4), luma(&frame, WIDTH - 1, HEIGHT - 1)], [16, 16, 16]);
    assert_eq!([chroma(&frame, 0, 2, 1), chroma(&frame, 1, 2, 1)], [200, 50]);
    assert_eq!([chroma(&frame, 0, 3, 0), chroma(&frame, 1, 0, 2)], [128, 128], "black around it");
}

#[test]
fn a_larger_picture_is_cut_to_the_frame() {
    let picture = Picture::new(WIDTH + 4, HEIGHT + 2, 0);
    let frame = frame_from(&picture);
    assert_eq!(frame.len(), FRAME_BYTES);
    let row = |y: usize| &frame[y * WIDTH..(y + 1) * WIDTH];
    assert_eq!(row(0), &picture.planes[0][..WIDTH]);
    assert_eq!(row(HEIGHT - 1), &picture.planes[0][(HEIGHT - 1) * (WIDTH + 4)..][..WIDTH]);
    assert!(frame[WIDTH * HEIGHT..WIDTH * HEIGHT * 5 / 4].iter().all(|&byte| byte == 200));
    assert!(frame[WIDTH * HEIGHT * 5 / 4..].iter().all(|&byte| byte == 50));
}

#[test]
fn a_picture_of_the_frames_size_is_the_frame() {
    let picture = Picture::new(WIDTH, HEIGHT, 32);
    let frame = frame_from(&picture);
    for y in [0, 1, HEIGHT / 2, HEIGHT - 1] {
        assert_eq!(&frame[y * WIDTH..(y + 1) * WIDTH], &picture.planes[0][y * (WIDTH + 32)..][..WIDTH]);
    }
    assert!(frame[WIDTH * HEIGHT..].iter().all(|&byte| byte == 200 || byte == 50));
}

#[test]
fn a_short_plane_ends_the_copy() {
    let mut target = vec![9; 8];
    copy_plane(&mut target, (4, 2), &[1, 2, 3, 4, 5], 4, (4, 2));
    assert_eq!(target, [1, 2, 3, 4, 9, 9, 9, 9]);
}

#[test]
fn the_connecting_animation_steps_every_200_ms() {
    let ms = Duration::from_millis;
    assert_eq!(connecting_step(ms(0)), 0);
    assert_eq!(connecting_step(ms(199)), 0);
    assert_eq!(connecting_step(ms(200)), 1);
    assert_eq!(connecting_step(ms(15 * 200 + 10)), 15);
    assert_eq!(connecting_step(ms(16 * 200)), 0, "it repeats");
}

#[test]
fn frames_keep_their_pace_without_catching_up() {
    let start = Instant::now();
    assert_eq!(next_frame(start, start), start + FRAME_INTERVAL);
    assert_eq!(next_frame(start, start + Duration::from_millis(5)), start + FRAME_INTERVAL, "a late write");
    let stalled = start + Duration::from_millis(500);
    assert_eq!(next_frame(start, stalled), stalled + FRAME_INTERVAL, "no burst after a stall");
}

#[test]
fn what_the_camera_shows_is_replaced() {
    let (device, _) = UnixStream::pair().unwrap();
    let file = File::from(std::os::fd::OwnedFd::from(device));
    let output = Output::new("phone".to_owned(), PathBuf::from("/dev/null"), file, false).unwrap();
    let mut drawn = None;
    assert_eq!(*output.frame_now(&mut drawn), black(), "black at first");

    output.show(Shown::Placeholder(CameraPlaceholder::Black, Instant::now()));
    let still = output.frame_now(&mut drawn);
    assert!(Arc::ptr_eq(&still, &output.frame_now(&mut drawn)), "a still image is drawn once");

    let picture = Arc::new(vec![7; FRAME_BYTES]);
    output.show(Shown::Frame(picture.clone()));
    assert!(Arc::ptr_eq(&output.frame_now(&mut drawn), &picture));
}

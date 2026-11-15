use std::collections::HashMap;
use std::fs::{File, OpenOptions};
use std::io::Write;
use std::os::fd::AsRawFd;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, OnceLock, PoisonError};
use std::time::Duration;

use openh264::decoder::Decoder;
use openh264::formats::YUVSource;
use phonelink_connected_camera::{CameraPlaceholder, PhoneCamera};
use phonelink_engine::edge::EdgeError;

mod placeholder;

const WIDTH: usize = 1280;
const HEIGHT: usize = 720;
const FRAME_BYTES: usize = WIDTH * HEIGHT * 3 / 2;

const VIDIOC_S_FMT: libc::c_ulong = 0xC0D0_5605;
const V4L2_BUF_TYPE_VIDEO_OUTPUT: u32 = 2;
const V4L2_PIX_FMT_YUV420: u32 = 0x3231_5559;
const V4L2_FIELD_NONE: u32 = 1;

#[repr(C, align(8))]
struct V4l2Format([u32; 52]);

fn loopback_device() -> Option<PathBuf> {
    let mut devices: Vec<_> = std::fs::read_dir("/sys/class/video4linux")
        .ok()?
        .flatten()
        .filter(|entry| {
            std::fs::canonicalize(entry.path()).is_ok_and(|path| path.to_string_lossy().contains("/virtual/"))
        })
        .map(|entry| PathBuf::from("/dev").join(entry.file_name()))
        .collect();
    devices.sort();
    devices.into_iter().next()
}

fn set_format(device: &File) -> std::io::Result<()> {
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

fn black() -> Vec<u8> {
    let mut frame = vec![16; WIDTH * HEIGHT];
    frame.resize(FRAME_BYTES, 128);
    frame
}

fn fitted(picture: &impl YUVSource) -> Vec<u8> {
    let (source_width, source_height) = picture.dimensions();
    let mut frame = black();
    if source_width == 0 || source_height == 0 {
        return frame;
    }
    let scale = (WIDTH as f64 / source_width as f64).min(HEIGHT as f64 / source_height as f64);
    let shown_width = ((source_width as f64 * scale) as usize).min(WIDTH) & !1;
    let shown_height = ((source_height as f64 * scale) as usize).min(HEIGHT) & !1;
    let left = ((WIDTH - shown_width) / 2) & !1;
    let top = ((HEIGHT - shown_height) / 2) & !1;
    let (y_stride, u_stride, v_stride) = picture.strides();
    let (luma, chroma) = frame.split_at_mut(WIDTH * HEIGHT);
    let (u_plane, v_plane) = chroma.split_at_mut(WIDTH * HEIGHT / 4);

    for row in 0..shown_height {
        let source_row = row * source_height / shown_height;
        for column in 0..shown_width {
            let source_column = column * source_width / shown_width;
            luma[(top + row) * WIDTH + left + column] = picture.y()[source_row * y_stride + source_column];
        }
    }
    for row in 0..shown_height / 2 {
        let source_row = row * (source_height / 2) / (shown_height / 2).max(1);
        for column in 0..shown_width / 2 {
            let source_column = column * (source_width / 2) / (shown_width / 2).max(1);
            let target = (top / 2 + row) * (WIDTH / 2) + left / 2 + column;
            u_plane[target] = picture.u()[source_row * u_stride + source_column];
            v_plane[target] = picture.v()[source_row * v_stride + source_column];
        }
    }
    frame
}

fn other_reader(device: &std::path::Path) -> Option<String> {
    let own = std::process::id().to_string();
    let processes = std::fs::read_dir("/proc").ok()?;
    let pid = |process: &std::fs::DirEntry| {
        process.file_name().to_str().is_some_and(|n| n.bytes().all(|b| b.is_ascii_digit()))
    };
    processes.flatten().filter(|process| pid(process) && process.file_name() != own.as_str()).find_map(|process| {
        let holds = std::fs::read_dir(process.path().join("fd"))
            .map(|fds| fds.flatten().any(|fd| std::fs::read_link(fd.path()).is_ok_and(|target| target == device)))
            .unwrap_or(false);
        holds.then(|| {
            let name = std::fs::read_to_string(process.path().join("comm")).unwrap_or_default();
            format!("{} ({})", name.trim(), process.file_name().to_string_lossy())
        })
    })
}

struct Output {
    path: PathBuf,
    device: File,
    decoder: Option<Decoder>,
    failures: u64,
}

impl Output {
    fn write(&mut self, frame: &[u8]) {
        if let Err(e) = self.device.write_all(frame) {
            eprintln!("the camera frame was not written: {e}");
        }
    }
}

pub fn camera() -> &'static Arc<LoopbackCamera> {
    static CAMERA: OnceLock<Arc<LoopbackCamera>> = OnceLock::new();
    CAMERA.get_or_init(Arc::default)
}

#[derive(Default)]
pub struct LoopbackCamera {
    outputs: Mutex<HashMap<String, Output>>,
    published: Mutex<Option<String>>,
    in_use: AtomicBool,
    placeholder_generation: AtomicU64,
}

impl LoopbackCamera {
    pub fn published(&self) -> Option<String> {
        self.published.lock().unwrap_or_else(PoisonError::into_inner).clone()
    }

    pub fn in_use(&self) -> bool {
        self.in_use.load(Ordering::Relaxed)
    }

    pub fn set_in_use(&self, in_use: bool) {
        self.in_use.store(in_use, Ordering::Relaxed);
    }

    pub fn reader(&self) -> Option<String> {
        let id = self.published()?;
        let path =
            self.outputs.lock().unwrap_or_else(PoisonError::into_inner).get(&id).map(|output| output.path.clone())?;
        other_reader(&path)
    }

    fn write_to(&self, device_id: &str, frame: &[u8]) -> bool {
        match self.outputs.lock().unwrap_or_else(PoisonError::into_inner).get_mut(device_id) {
            Some(output) => {
                output.write(frame);
                true
            }
            None => false,
        }
    }

    fn repeat_placeholder(&'static self, device_id: String, placeholder: CameraPlaceholder, generation: u64) {
        let still = (placeholder != CameraPlaceholder::Connecting).then(|| placeholder::frame(placeholder, 0));
        std::thread::spawn(move || {
            for step in (0..placeholder::CONNECTING_FRAMES).cycle() {
                let frame = still.clone().unwrap_or_else(|| placeholder::frame(placeholder, step));
                if self.placeholder_generation.load(Ordering::SeqCst) != generation
                    || !self.write_to(&device_id, &frame)
                {
                    return;
                }
                std::thread::sleep(Duration::from_millis(200));
            }
        });
    }
}

impl PhoneCamera for LoopbackCamera {
    fn is_available(&self) -> bool {
        loopback_device().is_some()
    }

    fn publish(&self, device_id: String, _name: String) -> Result<(), EdgeError> {
        let unexpected = |message: String| EdgeError::Unexpected { message };
        let path = loopback_device().ok_or_else(|| unexpected("no v4l2loopback device".to_owned()))?;
        let device = OpenOptions::new()
            .write(true)
            .open(&path)
            .map_err(|e| unexpected(format!("{} did not open: {e}", path.display())))?;
        set_format(&device).map_err(|e| unexpected(format!("{} took no format: {e}", path.display())))?;
        let mut output = Output { path, device, decoder: None, failures: 0 };
        output.write(&black());
        self.outputs.lock().unwrap_or_else(PoisonError::into_inner).insert(device_id.clone(), output);
        *self.published.lock().unwrap_or_else(PoisonError::into_inner) = Some(device_id);
        Ok(())
    }

    fn remove(&self, device_id: String) {
        self.outputs.lock().unwrap_or_else(PoisonError::into_inner).remove(&device_id);
        let mut published = self.published.lock().unwrap_or_else(PoisonError::into_inner);
        if published.as_deref() == Some(device_id.as_str()) {
            *published = None;
            self.set_in_use(false);
        }
    }

    fn show_placeholder(&self, device_id: String, placeholder: CameraPlaceholder) {
        let generation = self.placeholder_generation.fetch_add(1, Ordering::SeqCst) + 1;
        camera().repeat_placeholder(device_id, placeholder, generation);
    }

    fn video_frame(&self, device_id: String, data: Vec<u8>, show: bool) -> bool {
        let mut outputs = self.outputs.lock().unwrap_or_else(PoisonError::into_inner);
        let Some(output) = outputs.get_mut(&device_id) else { return false };
        if output.decoder.is_none() {
            output.decoder = Decoder::new().inspect_err(|e| eprintln!("no H.264 decoder: {e}")).ok();
        }
        let Some(decoder) = output.decoder.as_mut() else { return false };
        let frame = match crate::mirror::decode_access_unit(decoder, &data, |picture| fitted(picture)) {
            Ok(Some(frame)) => frame,
            Ok(None) => return false,
            Err(e) => {
                if output.failures < 8 {
                    eprintln!("a camera frame did not decode: {e}");
                }
                output.failures += 1;
                return false;
            }
        };
        if show {
            self.placeholder_generation.fetch_add(1, Ordering::SeqCst);
            output.write(&frame);
        }
        true
    }

    fn reset_decoder(&self, device_id: String) {
        if let Some(output) = self.outputs.lock().unwrap_or_else(PoisonError::into_inner).get_mut(&device_id) {
            output.decoder = None;
        }
    }
}

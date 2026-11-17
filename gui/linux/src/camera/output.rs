//! A published camera's output: its device, its decoder and what it shows, and the pacing thread that
//! writes it while an app reads the device.

use std::fs::File;
use std::io::{ErrorKind, Read, Write};
use std::os::fd::AsRawFd;
use std::os::unix::net::UnixStream;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use openh264::decoder::Decoder;
use phonelink_connected_camera::CameraPlaceholder;
use phonelink_core::sync::MutexExt;
use tracing::{debug, error, info, warn};

use super::frame::{black, frame_from};
use super::placeholder;
use super::readers::readers_listener;
use super::v4l2::dequeue_readers;

/// How often the camera's frame is written while an app reads it: 30 times a second.
pub(super) const FRAME_INTERVAL: Duration = Duration::from_nanos(1_000_000_000 / 30);
/// How long each image of the connecting animation shows.
const CONNECTING_STEP: Duration = Duration::from_millis(200);

/// What a camera shows.
#[derive(Clone)]
pub(super) enum Shown {
    /// A decoded picture, or black.
    Frame(Arc<Vec<u8>>),
    /// A still image or the connecting animation, since it was asked for.
    Placeholder(CameraPlaceholder, Instant),
}

/// What a camera shows, with a number that changes each time it does.
struct Latest {
    shown: Shown,
    revision: u64,
}

/// The step of the connecting animation `elapsed` after it started: a new image every 200 ms.
pub(super) fn connecting_step(elapsed: Duration) -> usize {
    let steps = elapsed.as_millis() / CONNECTING_STEP.as_millis();
    (steps % placeholder::CONNECTING_FRAMES as u128) as usize
}

/// When the pacing thread writes the frame after one due at `due`, written at `now`: a frame interval on,
/// or a frame interval after `now` when it fell further behind than that.
pub(super) fn next_frame(due: Instant, now: Instant) -> Instant {
    if now.saturating_duration_since(due) > FRAME_INTERVAL { now + FRAME_INTERVAL } else { due + FRAME_INTERVAL }
}

#[derive(Default)]
struct Decoding {
    decoder: Option<Decoder>,
    failures: u64,
}

/// What the pacing thread's wait ended with.
#[derive(Default)]
struct Ready {
    /// The device has an event pending.
    event: bool,
    /// The device reports an error or hung up: its events end.
    failed: bool,
}

/// One published camera: its device, decoder and what it shows.
pub(super) struct Output {
    pub(super) device_id: String,
    pub(super) path: PathBuf,
    device: File,
    /// The module reports the device's readers (v4l2loopback's client-usage event).
    pub(super) reports_readers: AtomicBool,
    /// An app reads the device, as last told to the engine or turned on with the Camera tile.
    pub(super) streaming: AtomicBool,
    /// Another process held the device open when the readers were last looked for.
    pub(super) scanned: AtomicBool,
    latest: Mutex<Latest>,
    decoding: Mutex<Decoding>,
    /// A frame was not written; logged once until one is.
    write_failed: AtomicBool,
    closed: AtomicBool,
    /// Wakes the pacing thread: the output closed, or its readers changed.
    wake: UnixStream,
    woken: UnixStream,
}

impl Output {
    pub(super) fn new(device_id: String, path: PathBuf, device: File, reports_readers: bool) -> std::io::Result<Self> {
        let (wake, woken) = UnixStream::pair()?;
        wake.set_nonblocking(true)?;
        woken.set_nonblocking(true)?;
        Ok(Self {
            device_id,
            path,
            device,
            reports_readers: AtomicBool::new(reports_readers),
            streaming: AtomicBool::new(false),
            scanned: AtomicBool::new(false),
            latest: Mutex::new(Latest { shown: Shown::Frame(Arc::new(black())), revision: 0 }),
            decoding: Mutex::default(),
            write_failed: AtomicBool::new(false),
            closed: AtomicBool::new(false),
            wake,
            woken,
        })
    }

    pub(super) fn write(&self, frame: &[u8]) {
        match (&self.device).write_all(frame) {
            Ok(()) => self.write_failed.store(false, Ordering::Relaxed),
            Err(e) => {
                if !self.write_failed.swap(true, Ordering::Relaxed) {
                    warn!(error = %e, "a camera frame was not written; later failures are not logged until one is");
                }
            }
        }
    }

    pub(super) fn wake(&self) {
        // A full socket has woken the thread already.
        let _ = (&self.wake).write_all(&[1]);
    }

    /// Ends the pacing thread: it returns once it is woken.
    pub(super) fn close(&self) {
        self.closed.store(true, Ordering::SeqCst);
        self.wake();
    }

    /// The camera shows `shown` from now on.
    pub(super) fn show(&self, shown: Shown) {
        let mut latest = self.latest.lock_or_recover();
        latest.shown = shown;
        latest.revision += 1;
    }

    /// Apps started (`true`) or stopped reading the device: the pacing thread and the listener are told.
    pub(super) fn report(&self, streaming: bool) {
        self.streaming.store(streaming, Ordering::SeqCst);
        self.wake();
        let listener = readers_listener().lock_or_recover().clone();
        match listener {
            Some(listener) => listener(self.device_id.clone(), streaming),
            None => debug!(
                is_streaming = streaming,
                "the camera's readers changed before anyone listens; they are told later"
            ),
        }
    }

    /// Decodes `data`; shows the picture when `show`. Whether it decoded to a picture.
    pub(super) fn video(&self, data: &[u8], show: bool) -> bool {
        let mut decoding = self.decoding.lock_or_recover();
        if decoding.decoder.is_none() {
            decoding.decoder = Decoder::new().inspect_err(|e| error!(error = %e, "no H.264 decoder")).ok();
        }
        let Some(decoder) = decoding.decoder.as_mut() else { return false };
        let frame = match crate::mirror::decode_access_unit(decoder, data, |picture| frame_from(picture)) {
            Ok(Some(frame)) => frame,
            Ok(None) => return false,
            Err(e) => {
                if decoding.failures < 8 {
                    warn!(error = %e, "a camera frame did not decode");
                }
                decoding.failures += 1;
                return false;
            }
        };
        drop(decoding);

        if show {
            self.show(Shown::Frame(Arc::new(frame)));
        }
        true
    }

    /// Drops the decoder: the next video frame starts a new one.
    pub(super) fn reset_decoder(&self) {
        self.decoding.lock_or_recover().decoder = None;
    }

    /// Waits until `timeout` passes (`None`: until something happens), the device has an event pending
    /// (with `events`) or the thread is woken.
    #[expect(unsafe_code, reason = "poll has no safe wrapper in the dependencies")]
    fn wait(&self, events: bool, timeout: Option<Duration>) -> Ready {
        let mut fds = [
            libc::pollfd { fd: self.woken.as_raw_fd(), events: libc::POLLIN, revents: 0 },
            // A negative descriptor is not waited on.
            libc::pollfd { fd: if events { self.device.as_raw_fd() } else { -1 }, events: libc::POLLPRI, revents: 0 },
        ];
        let timeout =
            timeout.map_or(-1, |timeout| i32::try_from(timeout.as_micros().div_ceil(1000)).unwrap_or(i32::MAX));
        let result = unsafe { libc::poll(fds.as_mut_ptr(), fds.len() as libc::nfds_t, timeout) };
        if result < 0 {
            let error = std::io::Error::last_os_error();
            if error.kind() != ErrorKind::Interrupted {
                error!(error = %error, "the camera's pacing thread could not wait; it tries again a frame later");
                std::thread::sleep(FRAME_INTERVAL);
            }
            return Ready::default();
        }

        if fds[0].revents & libc::POLLIN != 0 {
            let mut buffer = [0; 64];
            while (&self.woken).read(&mut buffer).is_ok_and(|read| read > 0) {}
        }
        let device = fds[1].revents;
        Ready {
            event: device & libc::POLLPRI != 0,
            failed: device & (libc::POLLERR | libc::POLLHUP | libc::POLLNVAL) != 0,
        }
    }

    /// The frame to write now, drawing a still image (kept in `drawn` while it shows) when it is one.
    pub(super) fn frame_now(&self, drawn: &mut Option<(u64, usize, Arc<Vec<u8>>)>) -> Arc<Vec<u8>> {
        let (shown, revision) = {
            let latest = self.latest.lock_or_recover();
            (latest.shown.clone(), latest.revision)
        };
        let (placeholder, since) = match shown {
            Shown::Frame(frame) => return frame,
            Shown::Placeholder(placeholder, since) => (placeholder, since),
        };

        let step = if placeholder == CameraPlaceholder::Connecting { connecting_step(since.elapsed()) } else { 0 };
        match drawn {
            Some((drawn_revision, drawn_step, frame)) if *drawn_revision == revision && *drawn_step == step => {
                frame.clone()
            }
            _ => {
                let frame = Arc::new(placeholder::frame(placeholder, step));
                *drawn = Some((revision, step, frame.clone()));
                frame
            }
        }
    }
}

/// The pacing thread of `output`: writes what it shows every 1/30 s while an app reads it, follows the
/// device's client-usage events, and ends when the output closes.
pub(super) fn pace(output: Arc<Output>) {
    // The engine takes reports of the camera's readers once publish has returned: the first is read a
    // frame after the black frame publish wrote.
    std::thread::sleep(FRAME_INTERVAL);
    let mut events = output.reports_readers.load(Ordering::SeqCst);
    // What the events said last; each publish starts with no readers.
    let mut reading = false;
    let mut due = Instant::now();
    let mut drawn = None;

    loop {
        if output.closed.load(Ordering::SeqCst) {
            return;
        }
        let timeout = output.streaming.load(Ordering::SeqCst).then(|| due.saturating_duration_since(Instant::now()));
        let ready = output.wait(events, timeout);
        if output.closed.load(Ordering::SeqCst) {
            return;
        }

        if events && (ready.event || ready.failed) {
            let readers = if ready.event {
                dequeue_readers(&output.device)
            } else {
                Err(std::io::Error::other("the device reported an error"))
            };
            match readers {
                Ok(Some(count)) if (count > 0) != reading => {
                    reading = count > 0;
                    info!(count, "apps started or stopped reading the camera");
                    output.report(reading);
                }
                Ok(_) => {}
                Err(e) => {
                    error!(error = %e, "the camera's client-usage events ended; the processes holding it open are looked for instead");
                    events = false;
                    output.reports_readers.store(false, Ordering::SeqCst);
                }
            }
        }

        if !output.streaming.load(Ordering::SeqCst) {
            continue;
        }
        let now = Instant::now();
        if now < due {
            continue;
        }
        due = next_frame(due, now);
        output.write(&output.frame_now(&mut drawn));
    }
}

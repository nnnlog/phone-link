//! How much decoded audio may wait to play, and which frames are dropped past it.

use std::time::Duration;

/// The depth the queue drains back to, in milliseconds, for frames `frame_ms` long: two frames, at
/// least 100 ms.
pub(super) fn desired_ms(frame_ms: u32) -> u64 {
    (2 * u64::from(frame_ms)).max(100)
}

/// The queue's maximum, in milliseconds, for frames `frame_ms` long: four frames, at least 170 ms.
fn maximum_ms(frame_ms: u32) -> u64 {
    (4 * u64::from(frame_ms)).max(170)
}

/// How much decoded audio waits to play, with bounds from the stream's frame length.
///
/// The bounds are 100 and 170 ms for the phone's 40 ms frames. Frames play as they come; past the
/// maximum they are dropped until what waits is back at the desired depth. No silence is added before a
/// frame.
#[derive(Debug)]
pub(super) struct AudioQueue {
    pub(super) desired: Duration,
    pub(super) maximum: Duration,
    /// Frames are being dropped until what waits is back at the desired depth.
    pub(super) draining: bool,
}

impl AudioQueue {
    pub(super) fn new(frame_ms: u32) -> Self {
        Self {
            desired: Duration::from_millis(desired_ms(frame_ms)),
            maximum: Duration::from_millis(maximum_ms(frame_ms)),
            draining: false,
        }
    }

    /// How much audio waits ahead of a frame: `backlog` frames, `frame` long each, not yet written, and
    /// what the stream holds, as its `latency` says.
    ///
    /// The stream holds at most its target latency (the desired depth: the server takes no more), so a
    /// latency past that is the output device's own (a Bluetooth headset's, say). That part is not
    /// counted, since dropping frames cannot shorten it.
    pub(super) fn waiting(&self, frame: Duration, backlog: u32, latency: Duration) -> Duration {
        frame * backlog + latency.min(self.desired)
    }

    /// Whether to play a decoded frame that finds `waiting` audio ahead of it.
    pub(super) fn admit(&mut self, waiting: Duration) -> bool {
        if waiting > self.maximum {
            self.draining = true;
        } else if waiting <= self.desired {
            self.draining = false;
        }
        !self.draining
    }
}

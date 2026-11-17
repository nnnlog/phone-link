//! One app's audio player: frames handed to a thread of its own that plays them in order.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::mpsc::{Receiver, Sender, channel};

use phonelink_screen_mirroring::edge::AudioStreamFormat;

use super::playback::Playback;

/// What a player and its playing thread share.
#[derive(Default)]
struct FrameHandoff {
    /// Frames sent and not yet taken.
    backlog: AtomicUsize,
    /// The player was replaced or ended: nothing more plays.
    stopped: AtomicBool,
}

/// The player's thread: plays each frame as it comes, until the player stops or goes.
fn play_frames(app: String, format: AudioStreamFormat, frames: Receiver<Vec<u8>>, handoff: Arc<FrameHandoff>) {
    let Some(mut playback) = Playback::new(app, &format) else { return };
    playback.open_when_due();
    for frame in frames {
        let backlog = handoff.backlog.fetch_sub(1, Ordering::SeqCst).saturating_sub(1);
        if handoff.stopped.load(Ordering::SeqCst) {
            break;
        }
        playback.play(frame, u32::try_from(backlog).unwrap_or(u32::MAX));
    }
    // Replaced or ended: the frames still waiting go with the channel, and the server's buffer here.
    playback.flush();
}

/// One app's audio, played from its start to its end.
pub struct Player {
    frames: Sender<Vec<u8>>,
    handoff: Arc<FrameHandoff>,
}

impl Player {
    /// Starts playing the audio of `app`, in `format`, on a thread of its own.
    pub fn start(app: &str, format: AudioStreamFormat) -> Self {
        let (frames, received) = channel();
        let handoff = Arc::new(FrameHandoff::default());
        let (app, thread_handoff) = (app.to_owned(), handoff.clone());
        std::thread::spawn(move || play_frames(app, format, received, thread_handoff));
        Self { frames, handoff }
    }

    /// Hands one frame to the player's thread.
    pub fn play(&self, frame: Vec<u8>) {
        self.handoff.backlog.fetch_add(1, Ordering::SeqCst);
        if self.frames.send(frame).is_err() {
            // The format is not played: the thread has gone.
            self.handoff.backlog.fetch_sub(1, Ordering::SeqCst);
        }
    }
}

impl Drop for Player {
    /// The app's audio was replaced or ended: what still waits to play is dropped.
    fn drop(&mut self) {
        self.handoff.stopped.store(true, Ordering::SeqCst);
    }
}

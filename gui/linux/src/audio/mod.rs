//! The audio of mirrored apps: each app's stream played on the default output of `PulseAudio` (or
//! `PipeWire`'s `PulseAudio` server), and the outputs followed so that the engine knows whether one
//! exists. A call's far end plays through a player too, and the default input is followed with the
//! outputs, for whether a call can be taken on this computer.
//!
//! Each app's player decodes its frames in order on a thread of its own and writes them to a stream that
//! asks the server for about two frames of latency (`buffer_attr`). What waits to play — the frames not
//! yet written and what the server holds — is bounded by `AudioQueue`: past its maximum, decoded frames
//! are dropped until it is back at its desired depth. A frame that decodes to nothing is skipped; a
//! stream that does not open or stops taking frames is opened again, at most once a second, while frames
//! keep coming. A player that is replaced or ended drops the frames still waiting and the server's buffer.

mod decoding;
mod outputs;
mod playback;
mod player;
mod queue;

pub use outputs::{has_microphone, has_output, on_devices_changed, on_outputs_changed, watch_outputs};
pub use player::Player;

#[cfg(test)]
mod tests;

//! A call's voice on this computer.
//!
//! The phone's Hands-Free voice link is a Bluetooth SCO socket (`sco`). Narrow-band voice crosses it as
//! plain samples; wide-band voice as mSBC frames (`msbc`) coded by the system's SBC library (`sbc`).
//! `exchange` turns what each packet from the phone carries into samples to play, and the microphone's
//! samples (`sound`) into the packet sent back; `carry` runs that for a link until it ends. `edge` is all
//! of it as the engine's voice edge.

mod carry;
mod edge;
mod exchange;
mod msbc;
mod sbc;
mod sco;
mod sound;
#[cfg(test)]
mod tests;

pub(crate) use edge::BluezVoice;

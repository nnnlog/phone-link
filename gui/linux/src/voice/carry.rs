//! One voice link's loop: packets read from the phone until the link ends or is stopped, what they
//! carry handed on to be played, and a packet of the same length sent back for each.

use std::io;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use super::exchange::Exchange;

/// A connected voice link's packets.
pub(super) trait Packets {
    /// Reads one packet into `packet`, waiting at most `timeout`: its length, `None` when none came,
    /// and `Some(0)` when the link ended.
    fn read(&mut self, packet: &mut [u8], timeout: Duration) -> io::Result<Option<usize>>;

    /// Writes one packet.
    fn write(&mut self, packet: &[u8]) -> io::Result<()>;
}

/// The call's sound on this computer, as the loop uses it.
pub(super) struct Sound<'a> {
    /// Plays the far end's samples.
    pub(super) heard: &'a mut dyn FnMut(&[i16]),
    /// Gives exactly as many of the microphone's samples as it is asked for.
    pub(super) spoken: &'a mut dyn FnMut(usize) -> Vec<i16>,
}

/// Exchanges packets over `link` until it ends or `stopped` is set, looking at `stopped` at least
/// every `poll`. `room` is the longest packet the link delivers.
pub(super) fn exchange_packets(
    link: &mut dyn Packets,
    room: usize,
    exchange: &mut Exchange,
    sound: Sound<'_>,
    stopped: &AtomicBool,
    poll: Duration,
) -> io::Result<()> {
    let mut packet = vec![0u8; room];
    while !stopped.load(Ordering::SeqCst) {
        let len = match link.read(&mut packet, poll)? {
            None => continue,
            Some(0) => return Ok(()),
            Some(len) => len,
        };

        let heard = exchange.heard(&packet[..len]);
        if !heard.is_empty() {
            (sound.heard)(&heard);
        }
        link.write(&exchange.reply(len, sound.spoken))?;
    }
    Ok(())
}

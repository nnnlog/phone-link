//! mSBC frames on a voice link: each 57-byte frame of 120 samples travels behind a two-byte header
//! that numbers it, with one byte of padding after it.

/// The samples one frame holds: 7.5 ms at 16 kHz.
pub(super) const FRAME_SAMPLES: usize = 120;
/// The length of one coded frame.
pub(super) const FRAME_LEN: usize = 57;
/// A frame on the link: the header, the frame, the padding.
pub(super) const FRAMED_LEN: usize = HEADER_LEN + FRAME_LEN + 1;
pub(super) const SAMPLE_RATE: u32 = 16_000;

const HEADER_LEN: usize = 2;
/// The header's first byte.
const HEADER_MARK: u8 = 0x01;
/// The header's second byte for the frames numbered 0 to 3, in turn.
const HEADER_NUMBERS: [u8; 4] = [0x08, 0x38, 0xC8, 0xF8];
/// The first byte of every coded frame.
const FRAME_MARK: u8 = 0xAD;

/// Puts coded frames on the link, numbered in turn.
#[derive(Debug, Default)]
pub(super) struct Framer {
    next: usize,
}

impl Framer {
    pub(super) fn frame(&mut self, coded: &[u8; FRAME_LEN]) -> [u8; FRAMED_LEN] {
        let mut framed = [0u8; FRAMED_LEN];
        framed[0] = HEADER_MARK;
        framed[1] = HEADER_NUMBERS[self.next];
        framed[HEADER_LEN..HEADER_LEN + FRAME_LEN].copy_from_slice(coded);

        self.next = (self.next + 1) % HEADER_NUMBERS.len();
        framed
    }
}

/// Finds the coded frames in the link's bytes, which arrive in packets of any length.
#[derive(Debug, Default)]
pub(super) struct Deframer {
    pending: Vec<u8>,
}

impl Deframer {
    /// Takes `received` and returns the frames now whole. Bytes before a frame's header are dropped.
    pub(super) fn push(&mut self, received: &[u8]) -> Vec<[u8; FRAME_LEN]> {
        self.pending.extend_from_slice(received);

        let mut frames = Vec::new();
        loop {
            let Some(start) = self.pending.windows(HEADER_LEN + 1).position(starts_a_frame) else {
                // A header may be cut at the end: its first bytes wait for the next packet.
                let keep = self.pending.len().min(HEADER_LEN);
                self.pending.drain(..self.pending.len() - keep);
                return frames;
            };
            self.pending.drain(..start);
            if self.pending.len() < HEADER_LEN + FRAME_LEN {
                return frames;
            }

            let mut frame = [0u8; FRAME_LEN];
            frame.copy_from_slice(&self.pending[HEADER_LEN..HEADER_LEN + FRAME_LEN]);
            frames.push(frame);
            self.pending.drain(..HEADER_LEN + FRAME_LEN);
        }
    }
}

fn starts_a_frame(bytes: &[u8]) -> bool {
    bytes[0] == HEADER_MARK && HEADER_NUMBERS.contains(&bytes[1]) && bytes[2] == FRAME_MARK
}

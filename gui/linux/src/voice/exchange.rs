//! What one voice link exchanges: each packet from the phone becomes samples to play, and for each
//! one a packet of the same length goes back, filled from the microphone.

use std::collections::VecDeque;

use super::msbc::{self, Deframer, FRAME_LEN, FRAME_SAMPLES, FRAMED_LEN, Framer};

/// The rate of narrow-band voice, which the adapter codes itself.
const NARROW_BAND_RATE: u32 = 8_000;

/// Codes wide-band voice one frame at a time.
pub(super) trait FrameCodec: Send {
    /// `None` when the samples did not code.
    fn encode(&mut self, samples: &[i16; FRAME_SAMPLES]) -> Option<[u8; FRAME_LEN]>;
    /// `None` when the frame did not decode.
    fn decode(&mut self, frame: &[u8; FRAME_LEN]) -> Option<[i16; FRAME_SAMPLES]>;
}

enum Coding {
    /// 16-bit samples as they are; a sample cut by a packet's end waits in `half`.
    NarrowBand {
        half: Option<u8>,
    },
    WideBand {
        codec: Box<dyn FrameCodec>,
        framer: Framer,
        deframer: Deframer,
    },
}

/// One link's exchange.
pub(super) struct Exchange {
    coding: Coding,
    /// The bytes coded and not yet sent.
    outgoing: VecDeque<u8>,
}

impl Exchange {
    pub(super) fn narrow_band() -> Self {
        Self { coding: Coding::NarrowBand { half: None }, outgoing: VecDeque::new() }
    }

    pub(super) fn wide_band(codec: Box<dyn FrameCodec>) -> Self {
        let coding = Coding::WideBand { codec, framer: Framer::default(), deframer: Deframer::default() };
        Self { coding, outgoing: VecDeque::new() }
    }

    /// The rate of the samples heard and spoken.
    pub(super) fn sample_rate(&self) -> u32 {
        match self.coding {
            Coding::NarrowBand { .. } => NARROW_BAND_RATE,
            Coding::WideBand { .. } => msbc::SAMPLE_RATE,
        }
    }

    /// The samples `received` carries. A wide-band frame that does not decode is as long a silence.
    pub(super) fn heard(&mut self, received: &[u8]) -> Vec<i16> {
        match &mut self.coding {
            Coding::NarrowBand { half } => {
                let bytes: Vec<u8> = half.take().into_iter().chain(received.iter().copied()).collect();
                let (samples, cut) = bytes.as_chunks::<2>();
                *half = cut.first().copied();
                samples.iter().map(|sample| i16::from_le_bytes(*sample)).collect()
            }
            Coding::WideBand { codec, deframer, .. } => deframer
                .push(received)
                .iter()
                .flat_map(|frame| codec.decode(frame).unwrap_or([0; FRAME_SAMPLES]))
                .collect(),
        }
    }

    /// The next `len` bytes to send. `microphone` gives exactly as many samples as it is asked for.
    pub(super) fn reply(&mut self, len: usize, microphone: &mut dyn FnMut(usize) -> Vec<i16>) -> Vec<u8> {
        while self.outgoing.len() < len {
            match &mut self.coding {
                Coding::NarrowBand { .. } => {
                    let samples = microphone((len - self.outgoing.len()).div_ceil(2));
                    self.outgoing.extend(samples.iter().flat_map(|sample| sample.to_le_bytes()));
                }
                Coding::WideBand { codec, framer, .. } => {
                    let mut samples = [0i16; FRAME_SAMPLES];
                    for (sample, spoken) in samples.iter_mut().zip(microphone(FRAME_SAMPLES)) {
                        *sample = spoken;
                    }
                    // Samples that do not code go out as a frame the phone drops.
                    let framed = codec.encode(&samples).map_or([0; FRAMED_LEN], |coded| framer.frame(&coded));
                    self.outgoing.extend(framed);
                }
            }
        }
        self.outgoing.drain(..len).collect()
    }
}

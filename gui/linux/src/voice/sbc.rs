//! The system's SBC library (`libsbc`) in its mSBC mode: 16 kHz mono voice coded 120 samples at a time
//! into 57-byte frames.

use std::ffi::{c_int, c_ulong, c_void};

use super::exchange::FrameCodec;
use super::msbc::{FRAME_LEN, FRAME_SAMPLES};

/// `sbc_t` of `sbc/sbc.h`.
#[repr(C)]
struct SbcState {
    flags: c_ulong,
    frequency: u8,
    blocks: u8,
    subbands: u8,
    mode: u8,
    allocation: u8,
    bitpool: u8,
    endian: u8,
    private: *mut c_void,
    private_alloc_base: *mut c_void,
}

#[expect(unsafe_code, reason = "the system's SBC library is a C library")]
#[link(name = "sbc")]
unsafe extern "C" {
    fn sbc_init_msbc(sbc: *mut SbcState, flags: c_ulong) -> c_int;
    fn sbc_decode(
        sbc: *mut SbcState,
        input: *const c_void,
        input_len: usize,
        output: *mut c_void,
        output_len: usize,
        written: *mut usize,
    ) -> isize;
    fn sbc_encode(
        sbc: *mut SbcState,
        input: *const c_void,
        input_len: usize,
        output: *mut c_void,
        output_len: usize,
        written: *mut isize,
    ) -> isize;
    fn sbc_finish(sbc: *mut SbcState);
}

const SAMPLES_LEN: usize = FRAME_SAMPLES * 2;

/// One direction's coder: frames are coded one after another, each from the last one's state.
struct Coder {
    state: Box<SbcState>,
}

impl Coder {
    #[expect(unsafe_code, reason = "the system's SBC library is a C library")]
    fn new() -> Option<Self> {
        let mut state = Box::new(SbcState {
            flags: 0,
            frequency: 0,
            blocks: 0,
            subbands: 0,
            mode: 0,
            allocation: 0,
            bitpool: 0,
            endian: 0,
            private: std::ptr::null_mut(),
            private_alloc_base: std::ptr::null_mut(),
        });
        // SAFETY: `state` is a writable `sbc_t`, which the library fills in.
        let initialized = unsafe { sbc_init_msbc(&raw mut *state, 0) };
        (initialized == 0).then_some(Self { state })
    }
}

// SAFETY: the library keeps a coder's state in memory that only this value points to, and nothing in
// it is tied to the thread that made it; a coder is used by one thread at a time (`&mut self`).
#[expect(unsafe_code, reason = "the system's SBC library is a C library")]
unsafe impl Send for Coder {}

impl Drop for Coder {
    #[expect(unsafe_code, reason = "the system's SBC library is a C library")]
    fn drop(&mut self) {
        // SAFETY: `state` was initialized by `sbc_init_msbc` and is finished once.
        unsafe { sbc_finish(&raw mut *self.state) };
    }
}

/// The wide-band voice codec of one link: a coder for what is sent and one for what is received.
pub(super) struct Msbc {
    encoder: Coder,
    decoder: Coder,
}

impl Msbc {
    /// `None` when the library does not start.
    pub(super) fn new() -> Option<Self> {
        Some(Self { encoder: Coder::new()?, decoder: Coder::new()? })
    }
}

impl FrameCodec for Msbc {
    #[expect(unsafe_code, reason = "the system's SBC library is a C library")]
    fn encode(&mut self, samples: &[i16; FRAME_SAMPLES]) -> Option<[u8; FRAME_LEN]> {
        let mut input = [0u8; SAMPLES_LEN];
        for (bytes, sample) in input.as_chunks_mut::<2>().0.iter_mut().zip(samples) {
            *bytes = sample.to_le_bytes();
        }
        let mut frame = [0u8; FRAME_LEN];
        let mut written: isize = 0;

        // SAFETY: the pointers and lengths describe `input` and `frame`, and `written` is writable;
        // the library reads at most `input_len` bytes and writes at most `output_len`.
        let consumed = unsafe {
            sbc_encode(
                &raw mut *self.encoder.state,
                input.as_ptr().cast(),
                input.len(),
                frame.as_mut_ptr().cast(),
                frame.len(),
                &raw mut written,
            )
        };
        (usize::try_from(consumed) == Ok(input.len()) && usize::try_from(written) == Ok(frame.len())).then_some(frame)
    }

    #[expect(unsafe_code, reason = "the system's SBC library is a C library")]
    fn decode(&mut self, frame: &[u8; FRAME_LEN]) -> Option<[i16; FRAME_SAMPLES]> {
        let mut output = [0u8; SAMPLES_LEN];
        let mut written: usize = 0;

        // SAFETY: the pointers and lengths describe `frame` and `output`, and `written` is writable;
        // the library reads at most `input_len` bytes and writes at most `output_len`.
        let consumed = unsafe {
            sbc_decode(
                &raw mut *self.decoder.state,
                frame.as_ptr().cast(),
                frame.len(),
                output.as_mut_ptr().cast(),
                output.len(),
                &raw mut written,
            )
        };
        if consumed <= 0 || written != output.len() {
            return None;
        }

        let mut samples = [0i16; FRAME_SAMPLES];
        for (sample, bytes) in samples.iter_mut().zip(output.as_chunks::<2>().0) {
            *sample = i16::from_le_bytes(*bytes);
        }
        Some(samples)
    }
}

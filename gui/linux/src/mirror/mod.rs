//! Screen mirroring on this computer: the presenter that decodes each window's video and plays its
//! apps' audio, how the windows are sized and placed, the keyboard's keys and input source, and the
//! JSON the mirroring views read.

mod h264;
mod input_source;
mod keys;
mod presenter;
pub mod sizing;
pub mod views;

pub(crate) use h264::decode_access_unit;
pub use input_source::keyboard_input_source;
pub use keys::{Keyboard, caps_lock_lit};
pub use presenter::{display_geometry, presenter};

//! The bottom layers of the Phone Link engine: the vocabulary (`types`, `time`, `sync`),
//! the constants and the network client, the log, the configuration and the codecs, the OS
//! edges, the client identification and the seams the layers above implement.

#![cfg_attr(feature = "test-support", warn(unreachable_pub))]

uniffi::setup_scaffolding!();

pub mod codec;
pub mod config;
pub mod constants;
pub mod edge;
pub mod identification;
pub mod logging;
pub mod net;
pub mod seams;
pub mod sync;
pub mod tap;
pub mod time;
pub mod types;

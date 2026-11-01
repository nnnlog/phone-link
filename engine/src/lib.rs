//! Protocol and feature core of the macOS Phone Link app: the engine, its `uniffi` surface, and
//! the installed set of features, on top of the crates under `crates/`. The crate builds the
//! library that `gui/` links through `uniffi` (with every crate beneath it) and the headless
//! `phonelink` CLI.
//!
//! The public modules are what those two use: [`api`] (the `uniffi` surface), [`engine`] (the core
//! an embedder builds and runs), [`edge`] (the OS edges), [`config`], [`constants`], [`codec`] (the
//! CLI's decoder) and [`tap`]. The other layers are re-exported at the paths this crate's code uses;
//! the `test-support` feature makes them public so the tests in `tests/` drive each layer through its
//! own interface by one path.

// With the layers public, a `pub` item no path reaches should be `pub(crate)`. In the app build every
// layer is private, so the lint would flag every item there.
#![cfg_attr(feature = "test-support", warn(unreachable_pub))]

uniffi::setup_scaffolding!();

pub mod api;
pub use phonelink_core::codec;
pub use phonelink_core::config;
pub use phonelink_core::constants;
pub use phonelink_core::edge;
pub mod engine;
pub use phonelink_core::tap;

// The layers beneath the public modules: private to the crate, public under the `test-support`
// feature. Each is declared twice, like the `features` module, which rustfmt reaches only through a
// plain `mod` line.
#[cfg(feature = "test-support")]
pub use phonelink_app as app;
#[cfg(not(feature = "test-support"))]
use phonelink_app as app;
#[cfg(feature = "test-support")]
pub use phonelink_transport::auth;
#[cfg(not(feature = "test-support"))]
use phonelink_transport::auth;
#[cfg(feature = "test-support")]
pub use phonelink_transport::dcg;
#[cfg(not(feature = "test-support"))]
use phonelink_transport::dcg;
#[cfg(feature = "test-support")]
pub mod features;
#[cfg(not(feature = "test-support"))]
mod features;
#[cfg(feature = "test-support")]
pub use phonelink_core::identification;
#[cfg(not(feature = "test-support"))]
use phonelink_core::identification;
#[cfg(feature = "test-support")]
pub use phonelink_core::logging;
#[cfg(not(feature = "test-support"))]
use phonelink_core::logging;
#[cfg(feature = "test-support")]
pub use phonelink_core::net;
#[cfg(feature = "test-support")]
pub use phonelink_core::seams;
#[cfg(not(feature = "test-support"))]
use phonelink_core::seams;
#[cfg(feature = "test-support")]
pub use phonelink_core::sync;
#[cfg(not(feature = "test-support"))]
use phonelink_core::sync;
#[cfg(feature = "test-support")]
pub use phonelink_core::time;
#[cfg(not(feature = "test-support"))]
use phonelink_core::time;
#[cfg(feature = "test-support")]
pub use phonelink_core::types;
#[cfg(not(feature = "test-support"))]
use phonelink_core::types;
#[cfg(feature = "test-support")]
pub use phonelink_nano as nano;
#[cfg(not(feature = "test-support"))]
use phonelink_nano as nano;
#[cfg(feature = "test-support")]
pub use phonelink_transport::platform;
#[cfg(not(feature = "test-support"))]
use phonelink_transport::platform;
#[cfg(feature = "test-support")]
pub use phonelink_transport::relay;
#[cfg(not(feature = "test-support"))]
use phonelink_transport::relay;

#[cfg(feature = "test-support")]
pub use phonelink_proto as proto;

#[cfg(test)]
mod invariants;

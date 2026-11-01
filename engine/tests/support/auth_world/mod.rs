//! Offline test support for the control plane (`src/auth`): a fake DCG server on `127.0.0.1`, a
//! fake Microsoft account service and sign-in web view for the device-bound broker, a fake
//! `WakeHub` (`wake_hub`), an independent JWT/certificate verifier, and a harness that wires
//! an `AuthService` to them. `tests/main.rs` declares it as `crate::support::auth_world`.
#![allow(dead_code)]

pub mod fake_dcg;
pub mod fake_msa;
pub mod fake_web;
pub mod harness;
pub mod jwt;
pub mod wake_hub;

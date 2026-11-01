//! The transport layers of the Phone Link engine: `auth` (sign-in, enrollment, trust, wake),
//! `relay` (the hub connections), `dcg` (the fragment layer) and `platform` (the platform
//! messages, sessions and routes).

#![cfg_attr(feature = "test-support", warn(unreachable_pub))]

pub mod auth;
pub mod dcg;
pub mod platform;
pub mod relay;

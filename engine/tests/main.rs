//! Every integration test in one test binary: the tests are grouped by layer (`app`, `auth`, `relay`,
//! `dcg`, `platform`, `nano`, `features/<feature>`, `engine`, `api`, `e2e`), and the fakes and fixtures
//! they share are `support`'s modules, declared once for all of them. One binary links the crate and its dependencies
//! once, where a binary per file linked them once per file. Run one file's tests with
//! `cargo test --test engine <layer>::<file>::`.

// Tests lock their own mutexes and unwrap what they expect: a poisoned lock or a missing value fails
// the test that hit it.
#![allow(clippy::disallowed_methods, clippy::unwrap_used)]

mod support;

mod api;
mod app;
mod auth;
mod cli;
mod codec;
mod dcg;
mod e2e;
mod engine;
mod features;
mod nano;
mod platform;
mod relay;

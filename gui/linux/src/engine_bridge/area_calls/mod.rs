//! Calls and contacts: the call history, call control over Bluetooth, and the contacts list.
//!
//! The call history comes with the prompts that stand in for it. Call control (dialing, answering,
//! declining, ending, swapping, declining with a text) comes with the incoming-call window, its ring
//! and the call window, and the question of a Bluetooth pairing. The contacts list comes with its
//! prompts and cards.
//!
//! `query` names: `contacts`, `contact`, `contactsState`, `calls`, `callControl`, `callsState`.
//! `perform` names: `calls.*`, `contacts.*`.
//! `areaEvent` names: `failure`, `dialed`, `bluetoothPairing` (area `calls`) and `show` (area `contacts`).

mod alerts;
mod calls_json;
mod contacts_json;
mod control;
mod presentation;
mod queries;
mod requests;
mod ringer;
mod routing;
mod state;
mod sync_wait;
#[cfg(test)]
mod tests;

pub(super) use state::State;

//! Calls and contacts: the call history, call control over Bluetooth, and the contacts list.
//!
//! The call history comes with the prompts that stand in for it. Call control (dialing, answering,
//! declining, ending, swapping, declining with a text, and for a call heard on this computer muting,
//! holding, the keypad and the transfer to the phone and back) comes with the incoming-call window,
//! its ring, the call window, the notice of a call taken on the phone, and the question of a Bluetooth
//! pairing. A `tel:` link opens the Calls section's keypad with its number. The contacts list comes with
//! its prompts and cards.
//!
//! `query` names: `contacts`, `contact`, `contactsState`, `calls`, `callControl`, `callsState`.
//! `perform` names: `calls.*`, `contacts.*`.
//! `areaEvent` names: `failure`, `dialed`, `bluetoothPairing`, `phoneCallNoticesOff` (area `calls`) and
//! `show` (area `contacts`).

mod alerts;
mod calls_json;
mod contacts_json;
mod control;
mod phone_call_notice;
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

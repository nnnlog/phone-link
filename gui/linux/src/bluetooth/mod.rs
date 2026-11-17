//! Bluetooth Classic through `BlueZ`: the radio and the phone among the paired devices on the adapter
//! in use, discovery and pairing (the user confirms the code in Phone Link's own question), RFCOMM
//! channels to a service on the phone through registered profiles, and the messaging proximity state,
//! which a watcher keeps from `BlueZ`'s signals so reading it never waits for `BlueZ`.

mod bluez;
mod deadline;
mod discovery;
mod edge;
mod names;
mod objects;
mod pairing;
mod profile;
mod rfcomm;
mod standins;
#[cfg(test)]
mod tests;
mod watch;

pub(crate) use edge::BluezBluetooth;
pub(crate) use pairing::{answer_pairing_question, on_pairing_question};
pub(crate) use standins::NoBluetoothProximity;

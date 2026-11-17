//! The proximity edge for a computer whose `BlueZ` cannot be reached at all.

use phonelink_messaging::{PhoneProximity, ProximityPhone, ProximityState};

/// The proximity when `BlueZ` cannot be reached at all (no system bus): the computer's Bluetooth
/// counts as off, as it does when `BlueZ` has no powered adapter.
pub(crate) struct NoBluetoothProximity;

impl PhoneProximity for NoBluetoothProximity {
    fn state(&self, _phone: ProximityPhone) -> ProximityState {
        ProximityState::BluetoothOff
    }
}

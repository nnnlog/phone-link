//! The phone's Bluetooth device for the UI ([`crate::features::bluetooth`]): why a feature that
//! reaches the phone over Bluetooth could not have it.
//!
//! Every such feature's error carries this one, so the UI words a pairing failure the same way
//! whichever feature ran into it.

use crate::features::bluetooth::BluetoothFailure;

/// Why the phone's Bluetooth device is not at hand.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error, uniffi::Error)]
pub enum BluetoothError {
    #[error("Bluetooth is off")]
    BluetoothOff,
    #[error("the phone did not grant its Bluetooth permission")]
    PermissionNotGranted,
    #[error("the phone did not become discoverable")]
    NotDiscoverable,
    #[error("the phone was not found")]
    PhoneNotFound,
    #[error("pairing failed: {reason}")]
    PairingFailed { reason: String },
    #[error("internal: {message}")]
    Internal { message: String },
}

impl From<BluetoothFailure> for BluetoothError {
    fn from(f: BluetoothFailure) -> Self {
        match f {
            BluetoothFailure::BluetoothOff => Self::BluetoothOff,
            BluetoothFailure::PermissionNotGranted => Self::PermissionNotGranted,
            BluetoothFailure::NotDiscoverable => Self::NotDiscoverable,
            BluetoothFailure::PhoneNotFound => Self::PhoneNotFound,
            BluetoothFailure::PairingFailed(reason) => Self::PairingFailed { reason },
            BluetoothFailure::Internal(message) => Self::Internal { message },
        }
    }
}

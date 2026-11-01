//! The Mac keyboard's virtual key codes as the USB HID keyboard-page usages the phone receives.
//!
//! Only usages Phone Link on Windows sends are produced: Command, Tab, Fn and the media and volume
//! keys reach the phone as nothing, and the keypad's Enter and / arrive as the main Return and /
//! keys, as they do from a Windows PC.

mod table;

/// What a Mac key sends.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum KeyMapping {
    /// Its usage goes down and up with the key.
    Usage(u8),
    /// A press and release of the usage when the key goes down; nothing when it goes up.
    Tap(u8),
    /// The key is not sent.
    NotSent,
}

/// HID usages the feature names.
pub mod usage {
    pub const CAPS_LOCK: u8 = 0x39;
    pub const NUM_LOCK: u8 = 0x53;
    pub const F15: u8 = 0x6A;
    pub const F18: u8 = 0x6D;
    pub const F19: u8 = 0x6E;
    pub const LANG1: u8 = 0x90;
    pub const LEFT_CONTROL: u8 = 0xE0;
}

/// The mapping of the Mac virtual key code `key_code` (`kVK_*`).
pub fn mac_key(key_code: u16) -> KeyMapping {
    table::MAC_KEYS.iter().find(|(code, _)| *code == key_code).map_or(
        // Volume, mute, the contextual-menu key and anything else.
        KeyMapping::NotSent,
        |(_, mapping)| *mapping,
    )
}

/// Whether `key_code` is an Option key.
pub fn is_option(key_code: u16) -> bool {
    matches!(key_code, 0x3A | 0x3D)
}

#[cfg(feature = "test-support")]
/// The usages a Windows PC can send, for checks.
///
/// Its keys' usages less Tab, Print Screen, Pause, the keypad's / and Enter, Application, Power and
/// the keys after F24 other than the international and language keys.
pub fn is_sent_by_windows(u: u8) -> bool {
    matches!(
        u,
        0x04..=0x2A
            | 0x2C..=0x45
            | 0x47
            | 0x49..=0x53
            | 0x55..=0x57
            | 0x59..=0x64
            | 0x67..=0x73
            | 0x85
            | 0x87..=0x8C
            | 0x90..=0x94
            | 0xE0..=0xE2
            | 0xE4..=0xE6
    )
}

#[cfg(test)]
mod tests;

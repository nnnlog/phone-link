//! The Mac keyboard's virtual key codes as the USB HID keyboard-page usages the phone receives.
//!
//! A key sends the usage Phone Link on Windows sends for it: Fn and the media and volume keys reach the
//! phone as nothing, and the keypad's Enter and / arrive as the main Return and / keys. Three things
//! differ, so that a Mac's keys do on the phone what they do on the Mac:
//!
//! - Tab is sent (Phone Link on Windows keeps it for its own window).
//! - Command is not a key of the phone's: a key pressed with it goes with Control instead, and the
//!   Mac's line and document keys go as the phone's (`mac_chord`).
//! - Option with an arrow or a delete key goes with Control, the phone's modifier for words and
//!   paragraphs, in place of Alt.

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

/// What a key pressed with Command, or an arrow or delete key pressed with Option, sends in place of
/// its own usage: presses and releases the phone reads as the Mac shortcut's meaning.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Chord {
    /// The usage, with whatever is held.
    Plain(u8),
    /// The usage with Control.
    Control(u8),
    /// The usage with Control while Alt is lifted.
    ControlForAlt(u8),
    /// Shift+Home, then Backspace: what is left of the cursor on its line goes.
    DeleteToLineStart,
}

/// The Mac modifier that gives a key another meaning on the phone.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum MacModifier {
    Command,
    Option,
}

/// HID usages the feature names.
pub mod usage {
    pub const BACKSPACE: u8 = 0x2A;
    pub const TAB: u8 = 0x2B;
    pub const CAPS_LOCK: u8 = 0x39;
    pub const HOME: u8 = 0x4A;
    pub const END: u8 = 0x4D;
    pub const NUM_LOCK: u8 = 0x53;
    pub const F15: u8 = 0x6A;
    pub const F18: u8 = 0x6D;
    pub const F19: u8 = 0x6E;
    pub const LANG1: u8 = 0x90;
    pub const LEFT_CONTROL: u8 = 0xE0;
    pub const LEFT_SHIFT: u8 = 0xE1;
    pub const LEFT_ALT: u8 = 0xE2;
    pub const RIGHT_ALT: u8 = 0xE6;
}

/// Mac virtual key codes (`kVK_*`) the chords name.
mod key_code {
    pub(super) const BACKSPACE: u16 = 0x33;
    pub(super) const FORWARD_DELETE: u16 = 0x75;
    pub(super) const LEFT: u16 = 0x7B;
    pub(super) const RIGHT: u16 = 0x7C;
    pub(super) const DOWN: u16 = 0x7D;
    pub(super) const UP: u16 = 0x7E;
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

/// The chord the Mac virtual key code `key_code` sends when pressed with `modifier`, if it has one.
///
/// With Command, the arrows are the Mac's line and document keys (Home and End, with Control for the
/// document's), Delete removes to the start of the line, and every other key that has a usage goes
/// with Control, the phone's shortcut modifier. With Option, the arrows and the delete keys — the
/// Mac's moves by a word or a paragraph and its deletes by a word — go with Control, which means the
/// same on the phone, where Alt with them means something else (Alt+Left is the start of the line,
/// or Back in a browser; Alt+Up the start of the text).
pub fn mac_chord(key_code: u16, modifier: MacModifier) -> Option<Chord> {
    let own = match mac_key(key_code) {
        // The modifier and lock keys keep their own meaning beside Command.
        KeyMapping::Usage(u) if u != usage::CAPS_LOCK && u != usage::NUM_LOCK && u < usage::LEFT_CONTROL => u,
        KeyMapping::Usage(_) | KeyMapping::Tap(_) | KeyMapping::NotSent => return None,
    };

    match (modifier, key_code) {
        (MacModifier::Command, key_code::LEFT) => Some(Chord::Plain(usage::HOME)),
        (MacModifier::Command, key_code::RIGHT) => Some(Chord::Plain(usage::END)),
        (MacModifier::Command, key_code::UP) => Some(Chord::Control(usage::HOME)),
        (MacModifier::Command, key_code::DOWN) => Some(Chord::Control(usage::END)),
        (MacModifier::Command, key_code::BACKSPACE) => Some(Chord::DeleteToLineStart),
        (MacModifier::Command, _) => Some(Chord::Control(own)),
        (
            MacModifier::Option,
            key_code::LEFT
            | key_code::RIGHT
            | key_code::UP
            | key_code::DOWN
            | key_code::BACKSPACE
            | key_code::FORWARD_DELETE,
        ) => Some(Chord::ControlForAlt(own)),
        (MacModifier::Option, _) => None,
    }
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

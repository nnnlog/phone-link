//! The computer's keys as a mirrored window sends them: Linux key codes as the Mac virtual key codes
//! the engine maps, the modifier flags held with them, and Caps Lock as the computer has it.

use std::path::Path;

const FLAG_CAPS_LOCK: u64 = 1 << 16;
const FLAG_SHIFT: u64 = 1 << 17;
const FLAG_CONTROL: u64 = 1 << 18;
const FLAG_OPTION: u64 = 1 << 19;

const CAPS_LOCK: u16 = 0x39;

/// Linux key codes (`KEY_*`, evdev) as the Mac virtual key codes (`kVK_*`) the engine maps. Korean
/// keyboards' Hangul and Hanja keys are `kVK_JIS_Kana` and `kVK_JIS_Eisu`, which reach the phone as LANG1
/// and LANG2.
static EVDEV_TO_MAC: [(u32, u16); 113] = [
    (1, 0x35),
    (2, 0x12),
    (3, 0x13),
    (4, 0x14),
    (5, 0x15),
    (6, 0x17),
    (7, 0x16),
    (8, 0x1A),
    (9, 0x1C),
    (10, 0x19),
    (11, 0x1D),
    (12, 0x1B),
    (13, 0x18),
    (14, 0x33),
    (15, 0x30),
    (16, 0x0C),
    (17, 0x0D),
    (18, 0x0E),
    (19, 0x0F),
    (20, 0x11),
    (21, 0x10),
    (22, 0x20),
    (23, 0x22),
    (24, 0x1F),
    (25, 0x23),
    (26, 0x21),
    (27, 0x1E),
    (28, 0x24),
    (29, 0x3B),
    (30, 0x00),
    (31, 0x01),
    (32, 0x02),
    (33, 0x03),
    (34, 0x05),
    (35, 0x04),
    (36, 0x26),
    (37, 0x28),
    (38, 0x25),
    (39, 0x29),
    (40, 0x27),
    (41, 0x32),
    (42, 0x38),
    (43, 0x2A),
    (44, 0x06),
    (45, 0x07),
    (46, 0x08),
    (47, 0x09),
    (48, 0x0B),
    (49, 0x2D),
    (50, 0x2E),
    (51, 0x2B),
    (52, 0x2F),
    (53, 0x2C),
    (54, 0x3C),
    (55, 0x43),
    (56, 0x3A),
    (57, 0x31),
    (58, CAPS_LOCK),
    (59, 0x7A),
    (60, 0x78),
    (61, 0x63),
    (62, 0x76),
    (63, 0x60),
    (64, 0x61),
    (65, 0x62),
    (66, 0x64),
    (67, 0x65),
    (68, 0x6D),
    (69, 0x47),
    (71, 0x59),
    (72, 0x5B),
    (73, 0x5C),
    (74, 0x4E),
    (75, 0x56),
    (76, 0x57),
    (77, 0x58),
    (78, 0x45),
    (79, 0x53),
    (80, 0x54),
    (81, 0x55),
    (82, 0x52),
    (83, 0x41),
    (86, 0x0A),
    (87, 0x67),
    (88, 0x6F),
    (89, 0x5E),
    (96, 0x4C),
    (97, 0x3E),
    (98, 0x4B),
    (100, 0x3D),
    (102, 0x73),
    (103, 0x7E),
    (104, 0x74),
    (105, 0x7B),
    (106, 0x7C),
    (107, 0x77),
    (108, 0x7D),
    (109, 0x79),
    (110, 0x72),
    (111, 0x75),
    (117, 0x51),
    (121, 0x5F),
    (122, 0x68),
    (123, 0x66),
    (124, 0x5D),
    (183, 0x69),
    (184, 0x6B),
    (185, 0x71),
    (186, 0x6A),
    (187, 0x40),
    (188, 0x4F),
    (189, 0x50),
    (190, 0x5A),
];

fn mac_key(evdev: u32) -> Option<u16> {
    EVDEV_TO_MAC.iter().find(|(code, _)| *code == evdev).map(|(_, mac)| *mac)
}

fn modifier_flag(mac: u16) -> u64 {
    match mac {
        0x38 | 0x3C => FLAG_SHIFT,
        0x3B | 0x3E => FLAG_CONTROL,
        0x3A | 0x3D => FLAG_OPTION,
        _ => 0,
    }
}

/// The keyboards' LED class devices, which hold each keyboard's Caps Lock light.
const LEDS: &str = "/sys/class/leds";

/// Whether a keyboard's Caps Lock light under `leds` (laid out like `/sys/class/leds`) is on; `None`
/// when no keyboard has one to read.
fn caps_lock_light(leds: &Path) -> Option<bool> {
    let entries = std::fs::read_dir(leds).ok()?;
    let mut found = false;
    for entry in entries.flatten() {
        if !entry.file_name().to_string_lossy().ends_with("::capslock") {
            continue;
        }
        let Ok(brightness) = std::fs::read_to_string(entry.path().join("brightness")) else { continue };
        found = true;
        if brightness.trim().parse::<u32>().is_ok_and(|level| level > 0) {
            return Some(true);
        }
    }
    found.then_some(false)
}

/// Whether Caps Lock is on, as the keyboards' lights show it; `None` without a light to read.
pub fn caps_lock_lit() -> Option<bool> {
    caps_lock_light(Path::new(LEDS))
}

/// The keyboard of the mirrored windows: the modifiers held and Caps Lock.
#[derive(Default)]
pub struct Keyboard {
    held: Vec<u16>,
    caps_lock: bool,
}

impl Keyboard {
    pub fn caps_lock(&self) -> bool {
        self.caps_lock
    }

    /// Caps Lock as the computer has it now; the key's next press turns it over from there.
    pub fn set_caps_lock(&mut self, on: bool) {
        self.caps_lock = on;
    }

    /// No modifier is held any more (the window lost the keyboard).
    pub fn release_all(&mut self) {
        self.held.clear();
    }

    /// The Mac key code and modifier flags of Linux key `evdev` going down or up; `None` for a key the
    /// engine does not map.
    pub fn event(&mut self, evdev: u32, down: bool) -> Option<(u16, u64)> {
        let mac = mac_key(evdev)?;
        if mac == CAPS_LOCK && down {
            self.caps_lock = !self.caps_lock;
        }

        if modifier_flag(mac) != 0 {
            self.held.retain(|held| *held != mac);
            if down {
                self.held.push(mac);
            }
        }
        let flags = self.held.iter().fold(0, |flags, held| flags | modifier_flag(*held));
        Some((mac, flags | if self.caps_lock { FLAG_CAPS_LOCK } else { 0 }))
    }
}

#[cfg(test)]
mod tests;

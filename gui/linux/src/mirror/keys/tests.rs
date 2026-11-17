use super::*;

#[test]
fn option_flag_follows_alt() {
    let mut keyboard = Keyboard::default();
    assert_eq!(keyboard.event(56, true), Some((0x3A, FLAG_OPTION)));
    assert_eq!(keyboard.event(30, true), Some((0x00, FLAG_OPTION)));
    assert_eq!(keyboard.event(56, false), Some((0x3A, 0)));
}

#[test]
fn caps_lock_toggles_on_press() {
    let mut keyboard = Keyboard::default();
    assert_eq!(keyboard.event(58, true), Some((CAPS_LOCK, FLAG_CAPS_LOCK)));
    assert_eq!(keyboard.event(58, false), Some((CAPS_LOCK, FLAG_CAPS_LOCK)));
    assert_eq!(keyboard.event(58, true), Some((CAPS_LOCK, 0)));
}

#[test]
fn unmapped_key_is_dropped() {
    assert_eq!(Keyboard::default().event(125, true), None);
}

/// `KEY_HANGEUL` and `KEY_HANJA` are `kVK_JIS_Kana` and `kVK_JIS_Eisu`.
#[test]
fn korean_keys_are_the_language_keys() {
    let mut keyboard = Keyboard::default();
    assert_eq!(keyboard.event(122, true), Some((0x68, 0)));
    assert_eq!(keyboard.event(123, true), Some((0x66, 0)));
    assert_eq!(keyboard.event(123, false), Some((0x66, 0)));
}

#[test]
fn each_linux_key_has_one_mac_key() {
    let mut codes: Vec<u32> = EVDEV_TO_MAC.iter().map(|(code, _)| *code).collect();
    assert!(codes.is_sorted(), "the table is kept in key code order");
    codes.dedup();
    assert_eq!(codes.len(), EVDEV_TO_MAC.len());
}

#[test]
fn caps_lock_follows_the_computer() {
    let mut keyboard = Keyboard::default();
    keyboard.set_caps_lock(true);
    assert_eq!(keyboard.event(30, true), Some((0x00, FLAG_CAPS_LOCK)));
    assert_eq!(keyboard.event(58, true), Some((CAPS_LOCK, 0)), "the next press turns it off");
}

/// A temporary directory laid out like `/sys/class/leds`, removed when the test ends.
struct Leds(std::path::PathBuf);

impl Leds {
    fn new(name: &str) -> Self {
        let root = std::env::temp_dir().join(format!("phonelink-leds-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        Self(root)
    }

    fn light(&self, name: &str, brightness: &str) -> &Self {
        let dir = self.0.join(name);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("brightness"), brightness).unwrap();
        self
    }
}

impl Drop for Leds {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

#[test]
fn caps_lock_is_read_from_any_keyboards_light() {
    let leds = Leds::new("on");
    leds.light("input3::numlock", "1\n").light("input3::capslock", "0\n").light("input7::capslock", "1\n");
    assert_eq!(caps_lock_light(&leds.0), Some(true));

    let off = Leds::new("off");
    off.light("input3::capslock", "0\n").light("tpacpi::power", "1\n");
    assert_eq!(caps_lock_light(&off.0), Some(false));

    let none = Leds::new("none");
    none.light("input3::numlock", "1\n");
    assert_eq!(caps_lock_light(&none.0), None, "no Caps Lock light");
    assert_eq!(caps_lock_light(&none.0.join("missing")), None);
}

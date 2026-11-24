use super::*;

#[test]
fn keys_map_to_the_usages_windows_sends() {
    assert_eq!(mac_key(0x00), KeyMapping::Usage(0x04));
    assert_eq!(mac_key(0x24), KeyMapping::Usage(0x28));
    assert_eq!(mac_key(0x33), KeyMapping::Usage(0x2A));
    assert_eq!(mac_key(0x7E), KeyMapping::Usage(0x52));
    assert_eq!(mac_key(0x3A), KeyMapping::Usage(0xE2));
    assert_eq!(mac_key(0x3D), KeyMapping::Usage(0xE6));
    assert_eq!(mac_key(0x68), KeyMapping::Tap(0x90));
    assert_eq!(mac_key(0x4C), KeyMapping::Usage(0x28), "keypad Enter is Return");
    assert_eq!(mac_key(0x4B), KeyMapping::Usage(0x38), "keypad / is the main /");
    assert_eq!(mac_key(0x47), KeyMapping::Usage(0x53), "Clear is Num Lock");

    assert!(is_option(0x3A) && is_option(0x3D) && !is_option(0x3B));

    assert_eq!(mac_key(0x30), KeyMapping::Usage(usage::TAB), "Tab is sent, unlike from a Windows PC");

    for not_sent in [0x37, 0x36, 0x3F, 0x48, 0x49, 0x4A, 0x6E] {
        assert_eq!(mac_key(not_sent), KeyMapping::NotSent, "key {not_sent:#x}");
    }

    for k in 0..=0x7Fu16 {
        match mac_key(k) {
            KeyMapping::Usage(u) | KeyMapping::Tap(u) => {
                assert!(u == usage::TAB || is_sent_by_windows(u), "key {k:#x} → {u:#x}");
                assert!(u != 0xE3 && u != 0xE7, "GUI is never sent");
            }
            KeyMapping::NotSent => {}
        }
    }

    // Every key has a usage of its own except the keypad's Enter and /, which share the main keys'.
    let keys: Vec<u16> =
        (0..=0x7Fu16).filter(|k| *k != 0x4C && *k != 0x4B && matches!(mac_key(*k), KeyMapping::Usage(_))).collect();
    let mapped: std::collections::HashSet<KeyMapping> = keys.iter().map(|k| mac_key(*k)).collect();
    assert_eq!(mapped.len(), keys.len());
}

#[test]
fn the_key_table_names_each_key_code_once() {
    let codes: std::collections::HashSet<u16> = table::MAC_KEYS.iter().map(|(code, _)| *code).collect();
    assert_eq!(codes.len(), table::MAC_KEYS.len());
}

#[test]
fn command_and_option_keys_have_the_chords_of_their_mac_meaning() {
    use Chord::{Control, ControlForAlt, DeleteToLineStart, Plain};
    use MacModifier::{Command, Option as Opt};

    // (key code, modifier, chord)
    let cases = [
        (0x08, Command, Some(Control(0x06))),       // C
        (0x24, Command, Some(Control(0x28))),       // Return
        (0x30, Command, Some(Control(usage::TAB))), // Tab
        (0x7B, Command, Some(Plain(usage::HOME))),
        (0x7C, Command, Some(Plain(usage::END))),
        (0x7E, Command, Some(Control(usage::HOME))),
        (0x7D, Command, Some(Control(usage::END))),
        (0x33, Command, Some(DeleteToLineStart)),
        (0x75, Command, Some(Control(0x4C))),
        (0x7B, Opt, Some(ControlForAlt(0x50))),
        (0x7C, Opt, Some(ControlForAlt(0x4F))),
        (0x33, Opt, Some(ControlForAlt(usage::BACKSPACE))),
        (0x75, Opt, Some(ControlForAlt(0x4C))),
        (0x7E, Opt, Some(ControlForAlt(0x52))),
        (0x7D, Opt, Some(ControlForAlt(0x51))),
        // Option with any other key stays Alt with that key.
        (0x00, Opt, None),
        (0x73, Opt, None),
        // Modifier and lock keys, the Kana key and keys that are not sent have no chord.
        (0x38, Command, None),
        (0x3B, Command, None),
        (0x39, Command, None),
        (0x47, Command, None),
        (0x68, Command, None),
        (0x37, Command, None),
        (0x3A, Opt, None),
    ];

    for (key, modifier, chord) in cases {
        assert_eq!(mac_chord(key, modifier), chord, "key {key:#x} with {modifier:?}");
    }
}

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

    for not_sent in [0x30, 0x37, 0x36, 0x3F, 0x48, 0x49, 0x4A, 0x6E] {
        assert_eq!(mac_key(not_sent), KeyMapping::NotSent, "key {not_sent:#x}");
    }

    for k in 0..=0x7Fu16 {
        match mac_key(k) {
            KeyMapping::Usage(u) | KeyMapping::Tap(u) => {
                assert!(is_sent_by_windows(u), "key {k:#x} → {u:#x}");
                assert!(u != 0x2B && u != 0xE3 && u != 0xE7, "Tab and GUI are never sent");
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

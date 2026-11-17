use super::*;

#[test]
fn wifi_switched_off_or_not_ready_is_unavailable() {
    assert!(wifi_usable(true, true, 30));
    assert!(wifi_usable(true, true, 100));
    assert!(!wifi_usable(false, true, 30), "switched off in NetworkManager");
    assert!(!wifi_usable(true, false, 30), "switched off by the hardware switch");
    assert!(!wifi_usable(true, true, 20), "the device is unavailable");
    assert!(!wifi_usable(true, true, 10), "the device is unmanaged");
}

use super::*;

#[test]
fn only_no_connectivity_is_no_internet() {
    assert!(!internet_available(NM_CONNECTIVITY_NONE));
    // Unknown, behind a portal, limited, full.
    for connectivity in [NM_CONNECTIVITY_UNKNOWN, 2, 3, 4] {
        assert!(internet_available(connectivity), "{connectivity}");
    }
}

use super::*;
use phonelink_core::types::{DcgClient, Environment};

#[test]
fn regions_from_codes() {
    assert_eq!(region_from_code("KR").as_deref(), Some("KR"));
    assert_eq!(region_from_code("kr").as_deref(), Some("KR"));
    assert_eq!(region_from_code("ko-KR").as_deref(), Some("KR"));
    assert_eq!(region_from_code("ko_KR").as_deref(), Some("KR"));
    assert_eq!(region_from_code("zh-Hant-TW").as_deref(), Some("TW"));
    assert_eq!(region_from_code("es-419"), None);
    assert_eq!(region_from_code(""), None);
}

#[test]
fn e164_with_and_without_a_region() {
    assert_eq!(to_e164("010-1234-5678", Some("KR")).as_deref(), Some("+821012345678"));
    assert_eq!(to_e164("(650) 555-0100", Some("US")).as_deref(), Some("+16505550100"));
    assert_eq!(to_e164("+44 20 7946 0000", None).as_deref(), Some("+442079460000"));
    assert_eq!(to_e164("+44 20 7946 0000", Some("KR")).as_deref(), Some("+442079460000"));
    assert_eq!(to_e164("650 555 0100", None), None, "a national number needs a region");
    assert_eq!(to_e164("someone@example.com", Some("US")), None);
}

#[test]
fn phone_country_falls_back_to_the_pc_only_while_no_code_was_received() {
    let mut r = PhoneRecord::new(&DcgClient::new("p", Environment::Prod));
    assert_eq!(phone_country(&r, Some("de-DE")).as_deref(), Some("DE"));

    r.country_code = Some("ko-KR".into());
    assert_eq!(phone_country(&r, Some("DE")).as_deref(), Some("KR"));

    assert_eq!(phone_country(&PhoneRecord::new(&DcgClient::new("p", Environment::Prod)), None), None);

    // A received code without a two-letter region: no region, not the PC's.
    for code in ["", "es-419"] {
        r.country_code = Some(code.into());
        assert_eq!(phone_country(&r, Some("US")), None, "{code:?}");
        assert_eq!(to_e164("650 555 0100", phone_country(&r, Some("US")).as_deref()), None, "{code:?}");
    }

    assert_eq!(to_e164("+1 650 555 0100", None).as_deref(), Some("+16505550100"));
}

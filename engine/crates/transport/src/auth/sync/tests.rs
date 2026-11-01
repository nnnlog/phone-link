use super::*;
use phonelink_core::types::DeviceCertificates;

fn item(custom: Option<&str>, os: Option<&str>) -> DeviceInfoItem {
    DeviceInfoItem {
        id: "p".into(),
        custom_data: custom.map(str::to_string),
        os_name: os.map(str::to_string),
        ..Default::default()
    }
}

#[test]
fn async_trust_capability() {
    assert!(is_async_trust_capable(&item(Some(r#"{"yppCapabilities":32}"#), Some("Android"))));
    assert!(is_async_trust_capable(&item(Some(r#"{"yppCapabilities":63}"#), None)));
    assert!(!is_async_trust_capable(&item(Some(r#"{"yppCapabilities":31}"#), Some("Android"))));
    assert!(!is_async_trust_capable(&item(Some("not json"), None)));
    assert!(is_async_trust_capable(&item(None, Some("IOS"))));
}

#[test]
fn certificate_selection_prefers_pki() {
    let mut d = item(None, None);
    d.certificates = Some(DeviceCertificates {
        ct_self_signed: Some(vec!["ct".into()]),
        self_signed: Some(vec!["ss".into()]),
        pki: Some(vec!["pki".into(), "pki2".into()]),
    });
    assert_eq!(peer_certificate_b64(&d), Some("pki"));

    d.certificates.as_mut().unwrap().pki = None;
    assert_eq!(peer_certificate_b64(&d), Some("ss"));

    d.certificates.as_mut().unwrap().self_signed = Some(vec![]);
    assert_eq!(peer_certificate_b64(&d), None, "CTSelfSigned is never selected");
}

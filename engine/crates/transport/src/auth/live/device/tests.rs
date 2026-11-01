use super::*;

#[test]
fn credentials_have_the_right_shape() {
    let (m, p) = random_credentials();

    assert_eq!(m.len(), 16);
    assert!(m.starts_with("02"));
    assert_eq!(p.len(), 20);
    assert!(m.chars().chain(p.chars()).all(|c| c.is_ascii_alphanumeric()));

    // Two calls differ (random).
    assert_ne!(random_credentials().0, random_credentials().0);
}

#[test]
fn request_carries_the_fixed_components() {
    let xml = device_add_request("02abcdefghijklmn", "0123456789abcdefghij");

    assert!(xml.starts_with("<DeviceAddRequest>"));
    assert!(xml.contains("<Membername>02abcdefghijklmn</Membername>"));
    assert!(xml.contains("<Component name=\"4101\" error=\"-2147024894\"/>"));
    assert!(xml.contains("<Component name=\"4113\">AA==</Component>"));
    assert!(xml.contains("<Component name=\"4145\">AQAAAA==</Component>"));
    assert!(xml.contains(&format!("<Component name=\"8196\">{CLEP_CHALLENGE_V2}</Component>")));
    assert!(xml.contains(&format!("<Component name=\"8197\">{CLEP_CHALLENGE_V4}</Component>")));

    // No XML declaration and no SOAP envelope.
    assert!(!xml.contains("<?xml"));
    assert!(!xml.contains("Envelope"));
}

#[test]
fn parses_a_response() {
    let body = r#"<DeviceAddResponse Success="true">
            <puid>0003BFFD12345678</puid>
            <License>
              <SPLicenseBlock>QUJD</SPLicenseBlock>
              <Binding Binding_Type="Hardware"><DeviceID>device-abc</DeviceID></Binding>
            </License>
            <HWDeviceID>hw-xyz</HWDeviceID>
            <GlobalDeviceID>g-123</GlobalDeviceID>
          </DeviceAddResponse>"#;

    let cred = parse_device_add_response(body, "02m", "pw").unwrap();
    assert_eq!(cred.puid, "0003BFFD12345678");
    assert_eq!(cred.sp_license_block, "QUJD");
    assert_eq!(cred.hw_device_id, "hw-xyz");
    assert_eq!(cred.device_id, "device-abc");
    assert_eq!(cred.membername, "02m");
}

#[test]
fn rejects_unsuccessful_and_incomplete() {
    assert!(matches!(
        parse_device_add_response(r#"<DeviceAddResponse Success="false"/>"#, "m", "p").unwrap_err(),
        DeviceError::NotSuccessful
    ));
    assert!(matches!(
        parse_device_add_response(r#"<DeviceAddResponse><puid>x</puid></DeviceAddResponse>"#, "m", "p").unwrap_err(),
        DeviceError::Missing("SPLicenseBlock")
    ));
}

use super::*;
use phonelink_proto::side_channel::v1::{SideChannelError, SideChannelHotspotReservation, SideChannelHotspotResponse};

#[test]
fn frames_round_trip_across_pieces() {
    let body = vec![7u8; 300];
    let frame = encode_frame(&body);
    assert_eq!(&frame[..2], &[0xAC, 0x02]);

    let mut d = FrameDecoder::default();
    d.push(&frame[..1]);
    assert_eq!(d.next_frame().unwrap(), None);
    d.push(&frame[1..10]);
    assert_eq!(d.next_frame().unwrap(), None);

    d.push(&frame[10..]);
    d.push(&encode_frame(b""));
    assert_eq!(d.next_frame().unwrap(), Some(body));
    assert_eq!(d.next_frame().unwrap(), Some(Vec::new()));
    assert_eq!(d.next_frame().unwrap(), None);
}

#[test]
fn oversized_and_malformed_lengths_are_rejected() {
    let mut d = FrameDecoder::default();
    d.push(&encode_frame(&vec![0u8; MAX_FRAME + 1])[..4]);
    assert_eq!(d.next_frame(), Err(FrameError::TooLarge(MAX_FRAME + 1)));

    let mut d = FrameDecoder::default();
    d.push(&[0xFF; 5]);
    assert_eq!(d.next_frame(), Err(FrameError::MalformedLength));
}

#[test]
fn the_request_carries_context_credentials_type_and_jwt() {
    let trace = request_trace();
    let request = credentials_request(&trace, "jwt".into());
    let bytes = request.encode_to_vec();

    let back = SideChannelClientRequest::decode(bytes.as_slice()).unwrap();
    let context = back.context.unwrap();

    assert_eq!(context.trace_id, trace.trace_id);
    assert_eq!(context.parent_id, "");
    assert_eq!(context.trace_state.get("ypTriggerId").map(String::as_str), Some("RequestCredentials"));
    assert_eq!(context.trace_state.get("isuserinitiated").map(String::as_str), Some("False"));
    assert_eq!(back.authorization.unwrap().signed_jwt_payload, "jwt");
    assert_eq!(
        back.request,
        Some(side_channel_client_request::Request::HotspotRequest(SideChannelHotspotRequest { r#type: 2 }))
    );
}

#[test]
fn authorization_data_keeps_its_order() {
    let phone = DcgClient::new("p", phonelink_core::types::Environment::Prod);
    assert_eq!(
        authorization_data("PK", "N", &phone).to_json(),
        r#"{"publicKey":"PK","nonce":"N","DCG-Environment":"Prod"}"#
    );
    assert_eq!(B64.decode(new_nonce()).unwrap().len(), 128);
}

fn reply(status: SideChannelHotspotResponseStatus, nonce: &str) -> SideChannelServerResponse {
    SideChannelServerResponse {
        ok: true,
        error: None,
        authorization: Some(SideChannelAuthorization { signed_jwt_payload: "reply-jwt".into() }),
        context: None,
        response: Some(side_channel_server_response::Response::HotspotResponse(SideChannelHotspotResponse {
            status: status as i32,
            reservation: Some(SideChannelHotspotReservation { status: 2, nonce: nonce.into() }),
            r#type: 2,
        })),
    }
}

#[test]
fn reply_checks() {
    use SideChannelHotspotResponseStatus as S;
    assert_eq!(check_reply(&reply(S::Success, "n"), "n"), Ok("reply-jwt".into()));
    assert_eq!(check_reply(&reply(S::Success, "other"), "n"), Err(ReplyError::InvalidNonce));
    assert_eq!(check_reply(&reply(S::Untrusted, "n"), "n"), Err(ReplyError::Untrusted));
    assert_eq!(check_reply(&reply(S::ErrorOther, "n"), "n"), Err(ReplyError::PhoneError(S::ErrorOther)));

    let mut not_capable = reply(S::NotHotspotCapable, "");
    if let Some(side_channel_server_response::Response::HotspotResponse(h)) = &mut not_capable.response {
        h.reservation.as_mut().unwrap().status = SideChannelHotspotCapableStatus::AirplaneMode as i32;
    }
    assert_eq!(check_reply(&not_capable, "n"), Err(ReplyError::Unavailable(Unavailable::AirplaneMode)));

    let mut no_auth = reply(S::Success, "n");
    no_auth.authorization = None;
    assert_eq!(check_reply(&no_auth, "n"), Err(ReplyError::AuthorizationMissing));

    // A scenario-service error is still read; any other error is not.
    let mut scenario = reply(S::Success, "n");
    scenario.ok = false;
    scenario.error =
        Some(SideChannelError { r#type: SideChannelErrorType::ScenarioServiceError as i32, msg: String::new() });
    assert!(check_reply(&scenario, "n").is_ok());
    scenario.error.as_mut().unwrap().r#type = SideChannelErrorType::CryptoTrustTimeout as i32;
    let e = check_reply(&scenario, "n").unwrap_err();
    assert!(e.is_retryable());

    let mut empty = reply(S::Success, "n");
    empty.response = None;
    assert_eq!(check_reply(&empty, "n"), Err(ReplyError::NoHotspotResponse));
}

#[test]
fn credentials_split_by_utf16_units_and_are_checked() {
    assert_eq!(
        split_credentials("핫스팟password1", "3"),
        Ok(Credentials { ssid: "핫스팟".into(), passphrase: "password1".into() })
    );
    assert!(split_credentials("abcpassword", "0").is_err());
    assert!(split_credentials("abcpassword", "x").is_err());
    assert!(split_credentials("abcpassword", "12").is_err());
    assert!(split_credentials("abcshort", "3").is_err());
    assert!(split_credentials(&format!("{}password", "가".repeat(33)), "33").is_err());

    // An emoji is two UTF-16 units, as the phone counts it.
    assert_eq!(
        split_credentials("📶Sam password1", "5"),
        Ok(Credentials { ssid: "📶Sam".into(), passphrase: " password1".into() })
    );
    assert!(split_credentials("📶Sam password1", "1").is_err());
}

#[test]
fn encrypted_credentials_are_read_as_text() {
    assert_eq!(
        encrypted_credentials(Some(r#"{"encryptedCredentials":"QUJD","encryptedSsidLength":"4"}"#)),
        Ok(("QUJD".into(), "4".into()))
    );
    assert_eq!(
        encrypted_credentials(Some(r#"{"encryptedCredentials":"QUJD","encryptedSsidLength":4}"#)),
        Ok(("QUJD".into(), "4".into()))
    );
    assert_eq!(encrypted_credentials(Some(r#"{"encryptedCredentials":"QUJD"}"#)), Err(CredentialsError::Missing));
    assert_eq!(encrypted_credentials(Some("[]")), Err(CredentialsError::Json));
    assert_eq!(encrypted_credentials(None), Err(CredentialsError::Missing));
}

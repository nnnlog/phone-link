//! The in-call controls' guards, and what a new connection offers the phone.

use super::*;

fn call(status: CallStatus, role: CallRole) -> Call {
    Call {
        id: 1,
        index: 1,
        incoming: false,
        status,
        waiting: false,
        number: String::new(),
        talking_since_ms: None,
        role,
    }
}

#[test]
fn the_controls_follow_the_call_and_where_its_audio_is() {
    use CallRole::{Primary, Untracked};
    use CallStatus::{Dialing, Held, Incoming, Talking};

    // (status, role, tracked calls, on the computer, can take audio, three-way) → (mute, hold, keypad, transfer)
    let cases = [
        ((Talking, Primary, 1, true, true, true), (true, true, true, true)),
        ((Held, Primary, 1, true, true, true), (false, true, false, true)),
        ((Dialing, Primary, 1, true, true, true), (false, false, false, true)),
        ((Talking, Primary, 2, true, true, true), (true, false, true, true)),
        ((Talking, Primary, 1, true, true, false), (true, false, true, true)),
        ((Talking, Primary, 1, false, true, true), (false, false, false, true)),
        ((Talking, Primary, 1, false, false, true), (false, false, false, false)),
        ((Talking, Untracked, 0, false, true, true), (false, false, false, true)),
        ((Talking, Untracked, 1, true, true, true), (false, false, false, true)),
        ((Incoming, Untracked, 0, false, true, true), (false, false, false, false)),
    ];

    for (given, expected) in cases {
        let (status, role, tracked, on_computer, can_take, three_way) = given;
        let c = controls(&call(status, role), tracked, on_computer, can_take, three_way);
        assert_eq!((c.mute, c.hold, c.keypad, c.transfer), expected, "controls of {given:?}");
    }
}

#[test]
fn a_connection_offers_wide_band_only_when_both_sides_negotiate() {
    let both = [VoiceCodec::Msbc, VoiceCodec::Cvsd];
    let negotiating = at::AG_CODEC_NEGOTIATION | at::AG_THREE_WAY;

    let state = VoiceState::for_connection(&both, negotiating);
    assert_eq!((state.offered.as_slice(), state.negotiated), (both.as_slice(), true));

    let state = VoiceState::for_connection(&both, at::AG_THREE_WAY);
    assert_eq!((state.offered.as_slice(), state.negotiated), ([VoiceCodec::Cvsd].as_slice(), false));

    let state = VoiceState::for_connection(&[VoiceCodec::Cvsd], negotiating);
    assert_eq!((state.offered.as_slice(), state.negotiated), ([VoiceCodec::Cvsd].as_slice(), false));

    let state = VoiceState::for_connection(&[], negotiating);
    assert!(state.offered.is_empty() && !state.negotiated);
}

#[test]
fn codec_ids_are_listed_in_ascending_order() {
    assert_eq!(codec_ids(&[VoiceCodec::Msbc, VoiceCodec::Cvsd]), [1, 2]);
    assert_eq!(codec_ids(&[]), Vec::<u8>::new());
}

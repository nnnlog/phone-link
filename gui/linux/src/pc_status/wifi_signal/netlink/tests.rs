use super::*;
use crate::pc_status::wifi_signal::testing::station_answer;
use crate::pc_status::wifi_signal::{Station, station_of};

const NL80211_FAMILY: u16 = 0x1c;

/// A netlink message as the kernel sends it: header, then `payload`.
fn kernel_message(kind: u16, sequence: u32, payload: &[u8]) -> Vec<u8> {
    let len = NLMSG_HDRLEN + payload.len();
    let mut message = Vec::new();
    message.extend_from_slice(&(len as u32).to_ne_bytes());
    message.extend_from_slice(&kind.to_ne_bytes());
    message.extend_from_slice(&0x2u16.to_ne_bytes());
    message.extend_from_slice(&sequence.to_ne_bytes());
    message.extend_from_slice(&4242u32.to_ne_bytes());
    message.extend_from_slice(payload);
    message.resize(aligned(message.len()), 0);
    message
}

#[test]
fn a_request_has_the_headers_and_padded_attributes() {
    let mut attributes = Vec::new();
    put_attribute(&mut attributes, CTRL_ATTR_FAMILY_NAME, b"nl80211\0");
    assert_eq!(attributes.len(), 12);
    let request = generic_message(GENL_ID_CTRL, NLM_F_REQUEST, 7, CTRL_CMD_GETFAMILY, 1, &attributes);
    let messages = netlink_messages(&request).unwrap();
    assert_eq!(messages.len(), 1);
    assert_eq!((messages[0].kind, messages[0].sequence), (GENL_ID_CTRL, 7));
    assert_eq!(&messages[0].payload[..GENL_HDRLEN], &[CTRL_CMD_GETFAMILY, 1, 0, 0]);
    let parsed = netlink_attributes(&messages[0].payload[GENL_HDRLEN..]).unwrap();
    assert_eq!(parsed, [(CTRL_ATTR_FAMILY_NAME, &b"nl80211\0"[..])]);
}

#[test]
fn the_family_id_comes_from_the_controllers_answer() {
    let mut attributes = Vec::new();
    put_attribute(&mut attributes, CTRL_ATTR_FAMILY_NAME, b"nl80211\0");
    put_attribute(&mut attributes, CTRL_ATTR_FAMILY_ID, &NL80211_FAMILY.to_ne_bytes());
    let answer = [vec![1, 2, 0, 0], attributes].concat();
    assert_eq!(family_id_of(&answer).unwrap(), NL80211_FAMILY);
    assert!(family_id_of(&[1, 2, 0, 0]).is_err());
}

#[test]
fn a_dump_collects_answers_until_it_is_done() {
    let first = [
        kernel_message(NL80211_FAMILY, 5, &station_answer(Some(-61), Some(-58), false)),
        kernel_message(NL80211_FAMILY, 4, &station_answer(None, Some(-30), false)),
    ]
    .concat();
    let mut answers = Vec::new();
    assert!(!take_answers(&first, NL80211_FAMILY, 5, true, &mut answers).unwrap());
    let done = kernel_message(NLMSG_DONE, 5, &0i32.to_ne_bytes());
    assert!(take_answers(&done, NL80211_FAMILY, 5, true, &mut answers).unwrap());
    assert_eq!(answers.len(), 1, "the answer to another request is skipped");
    assert_eq!(station_of(&answers[0]).unwrap(), Station { tdls_peer: false, signal: Some(-61) });
}

#[test]
fn the_kernels_failures_carry_their_error_number() {
    // An error message's number is followed by the header of the request it answers.
    let no_device = kernel_message(NLMSG_ERROR, 9, &[&(-libc::ENODEV).to_ne_bytes()[..], &[0; 16]].concat());
    let error = take_answers(&no_device, NL80211_FAMILY, 9, true, &mut Vec::new()).unwrap_err();
    assert_eq!(error.raw_os_error(), Some(libc::ENODEV));
    let unsupported = kernel_message(NLMSG_DONE, 9, &(-libc::EOPNOTSUPP).to_ne_bytes());
    let error = take_answers(&unsupported, NL80211_FAMILY, 9, true, &mut Vec::new()).unwrap_err();
    assert_eq!(error.raw_os_error(), Some(libc::EOPNOTSUPP));
}

#[test]
fn a_request_ends_with_its_one_answer() {
    let answer = kernel_message(GENL_ID_CTRL, 1, &[1, 2, 0, 0]);
    let mut answers = Vec::new();
    assert!(take_answers(&answer, GENL_ID_CTRL, 1, false, &mut answers).unwrap());
    assert_eq!(answers, [vec![1, 2, 0, 0]]);
}

#[test]
fn malformed_netlink_answers_are_errors() {
    let mut short = kernel_message(NL80211_FAMILY, 1, &station_answer(None, Some(-40), false));
    short.truncate(10);
    assert!(netlink_messages(&short).is_err());
    let mut overlong = kernel_message(NL80211_FAMILY, 1, &[]);
    overlong[0] = 200;
    assert!(netlink_messages(&overlong).is_err());
    assert!(netlink_attributes(&[8, 0, 1, 0, 0]).is_err());
    let unexpected = kernel_message(0x42, 1, &[]);
    assert!(take_answers(&unexpected, NL80211_FAMILY, 1, true, &mut Vec::new()).is_err());
}

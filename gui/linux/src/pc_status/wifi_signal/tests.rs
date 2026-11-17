use super::testing::station_answer;
use super::*;

#[test]
fn a_station_gives_its_average_signal_else_its_last() {
    let station = |answer: Vec<u8>| station_of(&answer).unwrap();
    assert_eq!(station(station_answer(Some(-61), Some(-58), false)).signal, Some(-61));
    assert_eq!(station(station_answer(None, Some(-58), false)).signal, Some(-58));
    assert_eq!(station(station_answer(None, None, false)).signal, None);
    assert!(station(station_answer(None, Some(-70), true)).tdls_peer);
}

#[test]
fn the_access_point_is_the_station_that_is_no_tdls_peer() {
    let access_point = Station { tdls_peer: false, signal: Some(-55) };
    let peer = Station { tdls_peer: true, signal: Some(-30) };
    assert_eq!(access_point_signal(&[peer, access_point]).unwrap(), Some(-55));
    assert_eq!(access_point_signal(&[peer]).unwrap(), None);
    assert_eq!(access_point_signal(&[]).unwrap(), None);
    assert!(access_point_signal(&[access_point, access_point]).is_err());
}

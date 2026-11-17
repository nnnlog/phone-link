//! nl80211 answers as the kernel sends them, for the tests of their reading.

use super::netlink::put_attribute;
use super::{
    NL80211_ATTR_IFINDEX, NL80211_ATTR_STA_INFO, NL80211_STA_FLAG_TDLS_PEER, NL80211_STA_INFO_SIGNAL,
    NL80211_STA_INFO_SIGNAL_AVG, NL80211_STA_INFO_STA_FLAGS,
};

const NLA_F_NESTED: u16 = 1 << 15;
const NL80211_CMD_NEW_STATION: u8 = 19;
const NL80211_ATTR_MAC: u16 = 6;

/// The payload of an nl80211 answer that describes one station.
pub(super) fn station_answer(signal_avg: Option<i8>, signal: Option<i8>, tdls_peer: bool) -> Vec<u8> {
    let mut info = Vec::new();
    if let Some(signal) = signal {
        put_attribute(&mut info, NL80211_STA_INFO_SIGNAL, &signal.to_ne_bytes());
    }
    if let Some(average) = signal_avg {
        put_attribute(&mut info, NL80211_STA_INFO_SIGNAL_AVG, &average.to_ne_bytes());
    }
    let flag = 1u32 << NL80211_STA_FLAG_TDLS_PEER;
    let set = if tdls_peer { flag } else { 0 };
    put_attribute(&mut info, NL80211_STA_INFO_STA_FLAGS, &[flag.to_ne_bytes(), set.to_ne_bytes()].concat());
    let mut attributes = Vec::new();
    put_attribute(&mut attributes, NL80211_ATTR_IFINDEX, &3u32.to_ne_bytes());
    put_attribute(&mut attributes, NL80211_ATTR_MAC, &[2, 0, 0, 0, 0, 1]);
    put_attribute(&mut attributes, NL80211_ATTR_STA_INFO | NLA_F_NESTED, &info);
    [vec![NL80211_CMD_NEW_STATION, 1, 0, 0], attributes].concat()
}

//! The Wi-Fi signal of the access point an interface is associated with, from the kernel's nl80211
//! station list over generic netlink.

mod netlink;

use std::ffi::CString;
use std::io;

use netlink::{GENL_HDRLEN, GenericNetlink, attribute_of, malformed, netlink_attributes, put_attribute, u32_at};

#[cfg(test)]
mod testing;
#[cfg(test)]
mod tests;

const NL80211: &str = "nl80211";
const NL80211_CMD_GET_STATION: u8 = 17;
const NL80211_ATTR_IFINDEX: u16 = 3;
const NL80211_ATTR_STA_INFO: u16 = 21;
/// The signal of the last frame received from the station, in dBm (a `u8` holding an `i8`).
const NL80211_STA_INFO_SIGNAL: u16 = 7;
/// The average signal, in dBm (a `u8` holding an `i8`).
const NL80211_STA_INFO_SIGNAL_AVG: u16 = 13;
/// A `struct nl80211_sta_flag_update`: a `u32` mask of the flags given, then a `u32` of those set.
const NL80211_STA_INFO_STA_FLAGS: u16 = 17;
const NL80211_STA_FLAG_TDLS_PEER: u32 = 6;

/// The signal, in dBm, of the access point the Wi-Fi interface `interface` is associated with, from
/// the interface's nl80211 station list (what `iw dev <interface> station dump` shows); `None` while it
/// is associated with none, once the interface is gone, and when its driver lists no stations or
/// reports no signal in dBm.
pub(super) fn station_signal(interface: &str) -> io::Result<Option<i32>> {
    let Some(index) = interface_index(interface)? else { return Ok(None) };
    let mut netlink = GenericNetlink::open()?;
    let family = netlink.family_id(NL80211)?;
    let mut attributes = Vec::new();
    put_attribute(&mut attributes, NL80211_ATTR_IFINDEX, &index.to_ne_bytes());
    let answers = match netlink.ask(family, true, NL80211_CMD_GET_STATION, 0, &attributes) {
        Ok(answers) => answers,
        // The interface went away after NetworkManager named it, or its driver keeps no station list.
        Err(e) if matches!(e.raw_os_error(), Some(libc::ENODEV | libc::EOPNOTSUPP)) => return Ok(None),
        Err(e) => return Err(e),
    };
    let stations = answers.iter().map(Vec::as_slice).map(station_of).collect::<io::Result<Vec<_>>>()?;
    access_point_signal(&stations)
}

/// The kernel's index of the network interface `name`; `None` when there is no such interface.
#[expect(unsafe_code, reason = "if_nametoindex has no safe wrapper in the dependencies")]
fn interface_index(name: &str) -> io::Result<Option<u32>> {
    let name = CString::new(name).map_err(|_| io::Error::other("an interface name holds a NUL"))?;
    // SAFETY: `name` is a NUL-terminated string that outlives the call.
    let index = unsafe { libc::if_nametoindex(name.as_ptr()) };
    if index != 0 {
        return Ok(Some(index));
    }
    let error = io::Error::last_os_error();
    if error.raw_os_error() == Some(libc::ENODEV) { Ok(None) } else { Err(error) }
}

/// What a station in an interface's nl80211 station list says about itself.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Station {
    /// A TDLS peer: a device on the same network talked to directly, not the access point.
    tdls_peer: bool,
    /// Its signal in dBm: the average where the driver keeps one, else that of the last frame.
    signal: Option<i32>,
}

/// The station one answer to `NL80211_CMD_GET_STATION` describes.
fn station_of(answer: &[u8]) -> io::Result<Station> {
    let attributes =
        netlink_attributes(answer.get(GENL_HDRLEN..).ok_or_else(|| malformed("a station answer is cut short"))?)?;
    let info = attribute_of(&attributes, NL80211_ATTR_STA_INFO)
        .ok_or_else(|| malformed("a station comes without its information"))?;
    let info = netlink_attributes(info)?;
    let tdls_peer = match attribute_of(&info, NL80211_STA_INFO_STA_FLAGS) {
        Some(flags) => {
            let (Some(mask), Some(set)) = (u32_at(flags, 0), u32_at(flags, 4)) else {
                return Err(malformed("a station's flags are cut short"));
            };
            let tdls_peer = 1 << NL80211_STA_FLAG_TDLS_PEER;
            mask & set & tdls_peer != 0
        }
        None => false,
    };
    let signal = attribute_of(&info, NL80211_STA_INFO_SIGNAL_AVG)
        .or_else(|| attribute_of(&info, NL80211_STA_INFO_SIGNAL))
        .map(|signal| match signal {
            [dbm] => Ok(i32::from(i8::from_ne_bytes([*dbm]))),
            _ => Err(malformed("a station's signal is not one byte")),
        })
        .transpose()?;
    Ok(Station { tdls_peer, signal })
}

/// The signal of the access point among an interface's stations: every station but a TDLS peer is the
/// access point, of which a client has one at most.
fn access_point_signal(stations: &[Station]) -> io::Result<Option<i32>> {
    let mut access_points = stations.iter().filter(|station| !station.tdls_peer);
    let access_point = access_points.next();
    if access_points.next().is_some() {
        return Err(io::Error::other("the Wi-Fi interface lists more than one access point"));
    }
    Ok(access_point.and_then(|station| station.signal))
}

use super::standins::{FixedPcStatus, PcState};
use super::*;

#[test]
fn rssi_bars() {
    for (rssi, bars) in [
        (-120, 0),
        (-100, 0),
        (-99, 0),
        (-89, 0),
        (-88, 1),
        (-78, 1),
        (-77, 2),
        (-67, 2),
        (-66, 3),
        (-56, 3),
        (-55, 4),
        (-30, 4),
    ] {
        assert_eq!(rssi_to_bars(rssi), bars, "{rssi} dBm");
    }
}

#[test]
fn laptop_on_wifi() {
    let pc = FixedPcStatus::new(PcState {
        battery: Some(PcBattery { current_capacity: 4321, max_capacity: 5000, on_ac_power: true }),
        low_power_mode: true,
        path: PcInternetPath::WiFi,
        wifi_rssi: Some(-60),
        failing: false,
    });

    let r = pc_status_response(&*pc);

    assert_eq!(r.status, DeviceStatusResponseStatus::Ok as i32);
    let info = r.info.unwrap();
    assert_eq!((info.battery_percent, info.battery_charging, info.battery_power_save), (86, true, true));
    assert_eq!((info.network_type(), info.wifi_signal_level), (NetworkType::Wifi, 3));
    assert_eq!(
        DeviceStatus {
            battery_percent: 86,
            battery_charging: true,
            battery_power_save: true,
            network_type: 1,
            wifi_signal_level: 3,
            ..Default::default()
        },
        info,
        "every other field unset"
    );
}

#[test]
fn desktop_and_paths() {
    let pc = FixedPcStatus::new(PcState::default());
    let info = read_pc_status(&*pc).unwrap();
    assert_eq!(
        info,
        DeviceStatus { battery_percent: -1, network_type: NetworkType::Ethernet as i32, ..Default::default() }
    );

    // Wi-Fi without an RSSI: both network fields unset.
    pc.set(|s| {
        s.path = PcInternetPath::WiFi;
        s.low_power_mode = true;
    });
    assert_eq!(read_pc_status(&*pc).unwrap(), DeviceStatus { battery_percent: -1, ..Default::default() });

    pc.set(|s| s.path = PcInternetPath::Cellular);
    assert_eq!(read_pc_status(&*pc).unwrap().network_type(), NetworkType::Cellular);

    for path in [PcInternetPath::Unavailable, PcInternetPath::Other] {
        pc.set(|s| s.path = path);
        assert_eq!(read_pc_status(&*pc).unwrap().network_type, 0, "{path:?}");
    }

    // A battery without a positive maximum counts as none.
    pc.set(|s| s.battery = Some(PcBattery { current_capacity: 10, max_capacity: 0, on_ac_power: false }));
    assert_eq!(read_pc_status(&*pc).unwrap().battery_percent, -1);
    pc.set(|s| s.battery = Some(PcBattery { current_capacity: 120, max_capacity: 100, on_ac_power: false }));
    assert_eq!(read_pc_status(&*pc).unwrap().battery_percent, 100);
}

#[test]
fn a_failing_read_is_unknown_error_without_info() {
    let pc = FixedPcStatus::new(PcState { failing: true, ..Default::default() });
    assert_eq!(pc_status_response(&*pc), unknown_error_response());
    assert_eq!(unknown_error_response().info, None);
}

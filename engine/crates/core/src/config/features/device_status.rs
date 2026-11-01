//! The options and timers of the `device_status` feature, at their defaults.

use std::time::Duration;

/// Every option, timer and limit of device status, at its default.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeviceStatusConfig {
    /// The Android 15 DND rule support (default on): the rule-disabled prompt and the command `9` a
    /// reverted DND turn-off offers. Only on is supported: the flag also gates reading
    /// `doNotDisturbRuleEnabled`, which the shared App layer records from every phone-content payload
    /// (on inbound dispatch) whatever the flag, so off is refused at install.
    pub android15_do_not_disturb: bool,
    /// The DND confirmation timer.
    pub do_not_disturb_confirmation: Duration,
    /// The ringer confirmation delay.
    pub ringer_confirmation: Duration,
    /// Ring my phone (default on): the ring control exists for a phone that can ring.
    pub ring_my_phone_enabled: bool,
    /// A ringing control offers stop (default on).
    pub stop_ring_my_phone_enabled: bool,
    /// How long the ring control shows the phone ringing.
    pub ring_my_phone_countdown: Duration,
    /// The timeout of a ring or stop request.
    pub ring_my_phone_request_timeout: Duration,
    /// The low-battery notification fires at or below this level (percent).
    pub low_battery_threshold: i32,
    /// At most one low-battery notification per this interval.
    pub low_battery_interval: Duration,
}

impl Default for DeviceStatusConfig {
    fn default() -> Self {
        Self {
            android15_do_not_disturb: true,
            do_not_disturb_confirmation: Duration::from_secs(5),
            ringer_confirmation: Duration::from_secs(20),
            ring_my_phone_enabled: true,
            stop_ring_my_phone_enabled: true,
            ring_my_phone_countdown: Duration::from_secs(20),
            ring_my_phone_request_timeout: Duration::from_secs(120),
            low_battery_threshold: 9,
            low_battery_interval: Duration::from_secs(3600),
        }
    }
}

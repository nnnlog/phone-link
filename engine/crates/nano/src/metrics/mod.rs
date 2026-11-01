//! Connection metrics: a real-time document over 3-second windows and a cumulative one since the
//! connection opened.
//!
//! Every connection's aggregators count the events of every Nano
//! connection and UDP socket in the process (STUN, TURN and checks included), not only their own.

use std::sync::{Arc, LazyLock, Mutex, Weak};

use super::transport::stats::Staggered;
use phonelink_core::sync::MutexExt;

/// Real-time windows: 3.0 s, 50 accumulators 60 ms apart.
const WINDOW: f64 = 3.0;
const ACCUMULATORS: usize = 50;
const METRICS_VERSION: &str = "1";

/// One event of the process-wide stream.
#[derive(Debug, Clone, Copy)]
pub enum MetricEvent {
    DatagramSent(usize),
    DatagramReceived(usize),
    /// A rate-controller sample (taken while sending): RTT in seconds, bandwidth in bit/s.
    RateSample {
        rtt: f64,
        bandwidth: f64,
    },
    /// An outbound packet declared lost; `timeout` when by retransmission timeout.
    PacketLost {
        timeout: bool,
    },
}

#[derive(Debug, Clone, Default)]
struct Running {
    count: u64,
    min: f64,
    max: f64,
    sum: f64,
    mean: f64,
    m2: f64,
}

impl Running {
    fn new() -> Self {
        Self { min: f64::MAX, max: f64::MIN, ..Default::default() }
    }

    fn add(&mut self, x: f64) {
        self.count += 1;
        self.min = self.min.min(x);
        self.max = self.max.max(x);
        self.sum += x;

        let delta = x - self.mean;
        self.mean += delta / self.count as f64;
        self.m2 += delta * (x - self.mean);
    }

    fn to_json(&self) -> String {
        let avg = if self.count == 0 { 0.0 } else { self.sum / self.count as f64 };
        let stddev = if self.count < 2 { 0.0 } else { (self.m2 / (self.count - 1) as f64).sqrt() };

        object(&[
            ("min", number(round3(self.min))),
            ("max", number(round3(self.max))),
            ("avg", number(round3(avg))),
            ("count", self.count.to_string()),
            ("stddev", number(round3(stddev))),
        ])
    }
}

/// One connection's aggregators.
pub struct Aggregators {
    origin: std::time::Instant,
    rtt: Staggered,
    bandwidth: Staggered,
    sent: Staggered,
    dropped: Staggered,
    received: Staggered,
    rtt_all: Running,
    bandwidth_all: Running,
    bytes_sent: u64,
    bytes_received: u64,
    packets_sent: u64,
    packets_received: u64,
    packets_dropped: u64,
    packets_dropped_timeout: u64,
}

impl Aggregators {
    fn new() -> Self {
        let window = || Staggered::new(ACCUMULATORS, WINDOW, WINDOW / ACCUMULATORS as f64);
        Self {
            origin: std::time::Instant::now(),
            rtt: window(),
            bandwidth: window(),
            sent: window(),
            dropped: window(),
            received: window(),
            rtt_all: Running::new(),
            bandwidth_all: Running::new(),
            bytes_sent: 0,
            bytes_received: 0,
            packets_sent: 0,
            packets_received: 0,
            packets_dropped: 0,
            packets_dropped_timeout: 0,
        }
    }

    fn record(&mut self, event: MetricEvent) {
        let t = self.origin.elapsed().as_secs_f64();
        match event {
            MetricEvent::DatagramSent(n) => {
                self.sent.add(t, 1.0);
                self.packets_sent += 1;
                self.bytes_sent += n as u64;
            }
            MetricEvent::DatagramReceived(n) => {
                self.received.add(t, 1.0);
                self.packets_received += 1;
                self.bytes_received += n as u64;
            }
            MetricEvent::RateSample { rtt, bandwidth } => {
                self.rtt.add(t, rtt);
                self.bandwidth.add(t, bandwidth);
                self.rtt_all.add(rtt);
                self.bandwidth_all.add(bandwidth);
            }
            MetricEvent::PacketLost { timeout } => {
                self.dropped.add(t, 1.0);
                self.packets_dropped += 1;
                if timeout {
                    self.packets_dropped_timeout += 1;
                }
            }
        }
    }

    /// `{"rttavg","bandwidthavg","packetlossrate","packetsreceivedintimewindow","metricsVersion"}`.
    pub fn realtime(&self) -> String {
        let (sent, dropped) = (self.sent.sum(), self.dropped.sum());
        let loss = if sent + dropped == 0.0 { 0.0 } else { dropped / (sent + dropped) };

        object(&[
            ("rttavg", number(round3(self.rtt.mean()))),
            ("bandwidthavg", number(round3(self.bandwidth.mean()))),
            ("packetlossrate", number(round3(loss))),
            ("packetsreceivedintimewindow", if self.received.count() > 0 { "1" } else { "0" }.into()),
            ("metricsVersion", METRICS_VERSION.into()),
        ])
    }

    /// `{"rtt":{…},"bandwidth":{…},"data":{…},"metricsVersion":"1"}`.
    pub fn cumulative(&self) -> String {
        let data = object(&[
            ("bytesSent", self.bytes_sent.to_string()),
            ("bytesReceived", self.bytes_received.to_string()),
            ("packetsSent", self.packets_sent.to_string()),
            ("packetsReceived", self.packets_received.to_string()),
            ("packetsDropped", self.packets_dropped.to_string()),
            ("packetsDroppedByTimeout", self.packets_dropped_timeout.to_string()),
        ]);

        format!(
            r#"{{"rtt":{},"bandwidth":{},"data":{},"metricsVersion":"{METRICS_VERSION}"}}"#,
            self.rtt_all.to_json(),
            self.bandwidth_all.to_json(),
            data
        )
    }
}

/// Rounds to 3 decimals, half away from zero (values too large to scale stay as they are).
fn round3(x: f64) -> f64 {
    let scaled = x * 1000.0;
    if scaled.is_finite() { scaled.round() / 1000.0 } else { x }
}

/// `printf("%.17g")`: 17 significant digits, trailing zeros removed, exponent form below 1e-5 and
/// from 1e17.
fn g17(x: f64) -> String {
    if x == 0.0 {
        return "0".into();
    }

    let exp = x.abs().log10().floor() as i32;
    let strip = |s: String| -> String {
        if s.contains('.') { s.trim_end_matches('0').trim_end_matches('.').to_string() } else { s }
    };

    if !(-5..17).contains(&exp) {
        let s = format!("{x:.16e}");
        let (mantissa, e) = s.split_once('e').expect("exponent form");
        let e: i32 = e.parse().expect("exponent");
        format!("{}e{}{:02}", strip(mantissa.to_string()), if e < 0 { '-' } else { '+' }, e.abs())
    } else {
        strip(format!("{x:.*}", (16 - exp).max(0) as usize))
    }
}

/// `x` with at most 17 significant digits (`nan`, `inf`, `-inf` as such).
fn number(x: f64) -> String {
    if x.is_nan() {
        "nan".into()
    } else if x.is_infinite() {
        if x > 0.0 { "inf".into() } else { "-inf".into() }
    } else {
        g17(x)
    }
}

/// A compact JSON object of string values, keys in the given order.
fn object(fields: &[(&str, String)]) -> String {
    let body: Vec<String> = fields
        .iter()
        .map(|(k, v)| format!("{}:{}", serde_json::Value::from(*k), serde_json::Value::from(v.as_str())))
        .collect();
    format!("{{{}}}", body.join(","))
}

static REGISTRY: LazyLock<Mutex<Vec<Weak<Mutex<Aggregators>>>>> = LazyLock::new(Mutex::default);

/// Registers a new set of aggregators for the process-wide event stream.
pub fn register() -> Arc<Mutex<Aggregators>> {
    let a = Arc::new(Mutex::new(Aggregators::new()));
    let mut r = REGISTRY.lock_or_panic();
    r.retain(|w| w.strong_count() > 0);
    r.push(Arc::downgrade(&a));
    a
}

/// Records one event in every registered set.
pub fn record(event: MetricEvent) {
    let live: Vec<Arc<Mutex<Aggregators>>> = REGISTRY.lock_or_panic().iter().filter_map(Weak::upgrade).collect();
    for a in live {
        a.lock_or_panic().record(event);
    }
}

#[cfg(test)]
mod tests;

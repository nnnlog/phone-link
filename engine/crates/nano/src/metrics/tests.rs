use super::*;
use serde_json::Value;

#[test]
fn documents() {
    let a = register();
    record(MetricEvent::DatagramSent(100));
    record(MetricEvent::DatagramSent(100));
    record(MetricEvent::DatagramSent(100));
    record(MetricEvent::PacketLost { timeout: true });
    record(MetricEvent::DatagramReceived(50));
    record(MetricEvent::RateSample { rtt: 0.0123456, bandwidth: 1_000_000.0 });

    let a = a.lock_or_panic();
    let rt: Value = serde_json::from_str(&a.realtime()).unwrap();
    assert_eq!(rt["rttavg"], "0.012");
    assert_eq!(rt["packetlossrate"], "0.25");
    assert_eq!(rt["packetsreceivedintimewindow"], "1");
    assert!(a.realtime().starts_with(r#"{"rttavg""#), "keys in the specified order");

    let c: Value = serde_json::from_str(&a.cumulative()).unwrap();
    assert!(c["data"]["packetsSent"].as_str().unwrap().parse::<u64>().unwrap() >= 3);
    assert!(c["rtt"]["count"].as_str().unwrap().parse::<u64>().unwrap() >= 1);

    let empty = Aggregators::new();
    let e: Value = serde_json::from_str(&empty.cumulative()).unwrap();
    assert_eq!(e["rtt"]["min"], "1.7976931348623157e+308", "before the first sample min holds the double extreme");

    assert_eq!(g17(0.012), "0.012");
    assert_eq!(g17(1_000_000.0), "1000000");
    assert_eq!(g17(0.000001), "9.9999999999999995e-07");
}

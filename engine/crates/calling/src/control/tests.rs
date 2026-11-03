//! The channel and the service-level connection against the scripted Audio Gateway.

use std::sync::Arc;
use std::time::Duration;

use super::link::{Link, LinkError, LinkEvent};
use super::testing::{ADDRESS, FakeAg};
use super::*;

async fn open(ag: &Arc<FakeAg>, command_timeout: Duration) -> (Arc<Link>, mpsc::UnboundedReceiver<LinkEvent>) {
    let edge: Arc<dyn phonelink_bluetooth::PhoneBluetooth> = ag.clone();
    let runtime = tokio::runtime::Handle::current();
    tokio::task::spawn_blocking(move || {
        Link::open(
            edge,
            ADDRESS,
            AG_SERVICE_UUID,
            Duration::from_secs(1),
            Duration::from_millis(20),
            command_timeout,
            &runtime,
        )
    })
    .await
    .unwrap()
    .expect("the channel opens")
}

#[tokio::test(flavor = "multi_thread")]
async fn the_service_level_connection_runs_in_order_and_names_the_indicators() {
    let ag = FakeAg::new();
    let (link, _events) = open(&ag, Duration::from_secs(1)).await;

    let indicators = service_level_connection(&link).await.expect("the connection is established");

    assert_eq!(indicators, ["call", "callsetup", "service", "signal", "roam", "battchg", "callheld"]);
    assert_eq!(
        ag.commands(),
        ["AT+BRSF=102", "AT+CIND=?", "AT+CIND?", "AT+CMER=3,0,0,1", "AT+CHLD=?", "AT+CLIP=1", "AT+CCWA=1"]
    );
    assert_eq!(ag.opened(), [AG_SERVICE_UUID]);
}

#[tokio::test(flavor = "multi_thread")]
async fn a_refused_step_fails_the_connection() {
    let ag = FakeAg::new();
    ag.refuse("AT+CMER");
    let (link, _events) = open(&ag, Duration::from_secs(1)).await;

    let e = service_level_connection(&link).await.unwrap_err();
    assert_eq!(e, SlcError::Refused { step: "AT+CMER", result: Final::Error });
}

#[tokio::test(flavor = "multi_thread")]
async fn unprompted_lines_go_to_the_events_and_a_closed_channel_ends_them() {
    let ag = FakeAg::new();
    let (link, mut events) = open(&ag, Duration::from_secs(1)).await;

    ag.ring("+15550100");
    let mut lines = Vec::new();
    while lines.len() < 3 {
        match events.recv().await.expect("an event") {
            LinkEvent::Unsolicited(line) => lines.push(line),
            LinkEvent::Closed => panic!("closed early"),
        }
    }
    assert_eq!(lines, [Line::Ciev { index: 2, value: 1 }, Line::Ring, Line::Clip(Some("+15550100".into()))]);

    let response = link.command(command::CLCC, Some("+CLCC")).await.unwrap();
    assert_eq!(clcc_entries(response.info).len(), 1);

    ag.drop_channel();
    assert_eq!(events.recv().await, Some(LinkEvent::Closed));
    assert_eq!(link.command(command::CLCC, Some("+CLCC")).await, Err(LinkError::Closed));
}

#[tokio::test(flavor = "multi_thread")]
async fn three_unanswered_commands_close_the_channel() {
    let ag = FakeAg::new();
    ag.ignore("AT+CLCC");
    let (link, mut events) = open(&ag, Duration::from_millis(50)).await;

    for _ in 0..3 {
        assert_eq!(link.command(command::CLCC, Some("+CLCC")).await, Err(LinkError::TimedOut("AT+CLCC".into())));
    }
    assert!(link.is_closed());
    assert_eq!(events.recv().await, Some(LinkEvent::Closed));
}

#[test]
fn the_dial_failure_follows_the_phone_status() {
    let mut record = PhoneRecord::default();
    assert_eq!(dial_failure(&record), DialFailure::Failed);

    record.status_keys.airplane_mode = Some(true);
    assert_eq!(dial_failure(&record), DialFailure::Failed, "airplane mode the phone does not report");
    record.status_capabilities |= status_capability::AIRPLANE_MODE;
    assert_eq!(dial_failure(&record), DialFailure::AirplaneMode);

    record.status_keys.network_signal_strength = Some(-1);
    assert_eq!(dial_failure(&record), DialFailure::NoNetwork);
}

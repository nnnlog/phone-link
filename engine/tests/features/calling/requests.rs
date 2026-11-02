//! The PC's side of call history: its connect contribution and the reply, the `/calling` request
//! and its triggers, the call-log `/permission` prompt.

use crate::support::app_world;
use crate::support::calling_world;
use crate::support::common::{eventually, payload};

use app_world::*;
use calling_world::*;
use phonelink_engine::app::consts::permission;
use phonelink_engine::app::{AppOutcome, ConnectKind, ConnectSendResult};
use phonelink_engine::codec::valueset::{self, Value, ValueSet};
use phonelink_engine::features::calling::CallsAccess;

fn connect_payload(h: &Harness) -> ValueSet {
    let reqs = h.messenger.requests_on("/connect/v1");
    valueset::decode(&reqs.last().expect("a connect was sent").message.payload).unwrap()
}

async fn connect(h: &Harness) {
    match h.app.send_connect(ConnectKind::Background, "Test").await {
        ConnectSendResult::Sent(_) => {}
        other => panic!("connect not sent: {other:?}"),
    }
}

/// Answers each request by its route.
fn respond_by_route(h: &Harness, f: impl Fn(&str) -> Reply + Send + 'static) {
    h.messenger.set_responder(move |req| f(req.message.route().unwrap_or_default()));
}

fn call_log_permission(world: &World) -> bool {
    world.h.app.phone_record().unwrap().permission_or(permission::CALL_LOGS, false)
}

#[tokio::test(start_paused = true)]
async fn without_the_consent_the_connect_turns_call_logs_off_and_sends_no_sequence_number() {
    let world = world();
    connect(&world.h).await;

    let payload = connect_payload(&world.h);
    assert_eq!(payload.object("configuration").unwrap().get("CallLogs"), Some(&Value::Bool(false)));
    assert_eq!(payload.object("sequenceNumbers").unwrap().get("call_logs"), None);
}

#[tokio::test(start_paused = true)]
async fn with_the_consent_the_connect_sends_the_current_sequence_number_and_stores_the_expected_one() {
    let world = world();
    world.grant();
    respond_by_route(&world.h, |route| match route {
        "/connect/v1" => Reply::Now(response_with(
            ValueSet::new()
                .with("result", 0i32)
                .with("call_logs", 5i64)
                .with("permissions", ValueSet::new().with("calls", false).with("call_logs", true)),
        )),
        _ => Reply::Now(result_response(0)),
    });
    connect(&world.h).await;

    let payload = connect_payload(&world.h);
    assert_eq!(payload.object("configuration").unwrap().get("CallLogs"), Some(&Value::Bool(true)));
    assert_eq!(payload.object("sequenceNumbers").unwrap().get("call_logs"), Some(&Value::Int64(-1)));

    assert!(!world.calling.status(&phone()).unwrap().up_to_date, "expected 5 not stored");
    assert!(call_log_permission(&world), "permissions.call_logs not applied");
    assert!(
        !world.h.app.phone_record().unwrap().permission_or(permission::CALLS, true),
        "permissions.calls not applied"
    );

    send(&world.h, metadata(&[], ValueSet::new().with("sequenceNumber", 5i64))).await;
    assert!(world.calling.status(&phone()).unwrap().up_to_date, "current 5 not stored");
}

#[tokio::test(start_paused = true)]
async fn the_empty_history_refresh_sends_calling_once_then_a_contacts_sync() {
    let world = world();
    world.h.app.update_record_for(&phone(), |r| r.set_permission(permission::CALL_LOGS, true));
    assert_eq!(world.calling.refresh_if_empty().await.unwrap(), None, "sent without the consent");

    world.calling.set_pc_consent(&phone(), true).unwrap();
    assert!(eventually(|| world.h.messenger.requests_on("/calling").len() == 1).await, "the consent sent nothing");
    assert_eq!(world.calling.refresh_if_empty().await.unwrap(), None, "sent twice");

    let req = &world.h.messenger.requests_on("/calling")[0];
    let vs = payload(req);
    let cv = vs.str("correlationVector").unwrap().to_string();
    assert_eq!(cv, req.trace.correlation_id);
    assert_eq!(vs.str("contentType"), Some("callLogs"));
    assert_eq!(vs.string_array("relatedCorrelationIds"), Some(&[cv][..]));
    assert_eq!(vs.string_array("subcontentTypes"), Some(&["callLogs".to_string()][..]));
    assert_eq!(vs.get("isuserinitiated"), Some(&Value::Bool(false)));
    assert_eq!(*world.contacts.syncs.lock().unwrap(), vec![false], "no contacts sync followed");
}

#[tokio::test(start_paused = true)]
async fn no_refresh_without_contacts_capabilities_or_with_a_stored_history() {
    let without = world();
    without.h.app.update_record_for(&phone(), |r| r.contacts_capabilities = None);
    without.h.app.update_record_for(&phone(), |r| r.set_permission(permission::CALL_LOGS, true));
    without.calling.set_pc_consent(&phone(), true).unwrap();
    tokio::task::yield_now().await;
    assert!(without.h.messenger.requests_on("/calling").is_empty(), "/calling sent without contacts capabilities");

    let stored = world();
    send(&stored.h, items(&[(1, "+15550100", 1, 1_000, 10)], ValueSet::new())).await;
    stored.grant();
    assert_eq!(stored.calling.refresh_if_empty().await.unwrap(), None, "sent with a stored history");
    assert!(stored.h.messenger.requests_on("/calling").is_empty(), "/calling sent with a stored history");
}

#[tokio::test(start_paused = true)]
async fn needs_permission_clears_the_call_log_permission() {
    let world = world();
    world.h.app.update_record_for(&phone(), |r| r.set_permission(permission::CALL_LOGS, true));
    respond_by_route(&world.h, |route| match route {
        "/calling" => Reply::Now(result_response(7)),
        _ => Reply::Now(result_response(0)),
    });
    world.calling.set_pc_consent(&phone(), true).unwrap();

    assert!(eventually(|| !call_log_permission(&world)).await, "the permission is still held");
    assert_eq!(world.calling.status(&phone()).unwrap().access, CallsAccess::DevicePermissionRequired);
}

#[tokio::test(start_paused = true)]
async fn an_accepted_permission_prompt_grants_the_call_log_and_refreshes() {
    let world = world();
    world.calling.set_pc_consent(&phone(), true).unwrap();
    respond_by_route(&world.h, |route| match route {
        "/permission" => Reply::Now(result_response(4)),
        _ => Reply::Now(result_response(0)),
    });

    let result = world.calling.request_device_permission().await.unwrap();

    assert_eq!(result.outcome, AppOutcome::Accepted);
    assert_eq!(result.refresh, Some(AppOutcome::Success));
    assert!(call_log_permission(&world), "the permission is not granted");
    let req = &world.h.messenger.requests_on("/permission")[0];
    assert_eq!(payload(req).get("permissionType"), Some(&Value::Int32(256)));
    assert_eq!(payload(req).get("silent"), Some(&Value::Bool(false)));
    assert_eq!(world.h.messenger.requests_on("/calling").len(), 1);
}

#[tokio::test(start_paused = true)]
async fn a_denied_permission_prompt_sends_no_refresh() {
    let world = world();
    world.calling.set_pc_consent(&phone(), true).unwrap();
    respond_by_route(&world.h, |_| Reply::Now(result_response(5)));

    let result = world.calling.request_device_permission().await.unwrap();

    assert_eq!(result.refresh, None);
    assert!(world.h.messenger.requests_on("/calling").is_empty(), "/calling sent after a denial");
    assert!(!call_log_permission(&world), "the permission is granted");
}

#[tokio::test(start_paused = true)]
async fn the_access_follows_the_capabilities_the_consent_and_the_permissions() {
    use phonelink_engine::app::consts::capability;
    let world = world();
    let access = |w: &World| w.calling.status(&phone()).unwrap().access;
    assert_eq!(access(&world), CallsAccess::PcConsentRequired);

    world.calling.set_pc_consent(&phone(), true).unwrap();
    assert_eq!(access(&world), CallsAccess::DevicePermissionRequired);

    world.h.app.update_record_for(&phone(), |r| {
        r.attributes_os_version = Some("14".into());
        r.set_permission(permission::POST_NOTIFICATIONS, false);
    });
    assert_eq!(access(&world), CallsAccess::PostNotificationRequired);
    world.h.app.update_record_for(&phone(), |r| r.set_permission(permission::AUTO_LAUNCH, true));
    assert_eq!(access(&world), CallsAccess::DevicePermissionRequired);

    world.h.app.update_record_for(&phone(), |r| r.capability_bits &= !capability::CALL_LOGS);
    assert_eq!(access(&world), CallsAccess::CallLogsUnsupported);
    world.h.app.update_record_for(&phone(), |r| r.capability_bits &= !capability::CALLS);
    assert_eq!(access(&world), CallsAccess::Unsupported);
}

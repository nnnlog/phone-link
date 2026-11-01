use std::sync::atomic::AtomicBool;

use super::*;
use phonelink_core::edge::standins::FixedReachability;
use phonelink_core::seams::TokenError;
use phonelink_core::tap::null_tap;

struct NoTokens;

#[async_trait]
impl TokenSource for NoTokens {
    async fn services_token(&self, _env: Environment, _force_refresh: bool) -> Result<String, TokenError> {
        Err(TokenError::Other("no tokens in this test".into()))
    }
}

fn manager() -> Arc<RelayManager> {
    let ident = ClientIdentification {
        logical_device_id: "bG9naWNhbC1kZXZpY2UtaWQtZm9yLXRlc3RzLTMyYnl0ZXM=".into(),
        app_version: "1.26082.74.0".into(),
        app_id: "MicrosoftWindows.CrossDevice_cw5n1h2txyewy".into(),
        session_id: "0f8fad5b-d9cb-469f-a165-70867728950e".into(),
        ring_name: "Public".into(),
        os: "Windows".into(),
        os_version: "10.0.26100".into(),
    };
    RelayManager::new(
        Arc::new(EngineConfig::default()),
        Arc::new(ident),
        Arc::new(NoTokens),
        Arc::new(FixedReachability(AtomicBool::new(true))),
        null_tap(),
    )
}

#[tokio::test]
async fn a_close_its_caller_stops_waiting_for_still_completes() {
    let relay = manager();
    let env = Environment::Prod;
    let phone = DcgClient::new("phone", env);
    let connection = relay.get_or_create(&phone, TraceContext::new_root(Vec::new()));

    relay.shared.keys.lock_or_panic().insert(ConnectionKey {
        partner: phone.id.clone(),
        env,
        connection_session_id: None,
    });

    // The close has to wait for the lifecycle lock; its caller gives up meanwhile.
    let lifecycle = relay.slot(env).lifecycle.lock().await;
    tokio::select! {
        biased;
        _ = HubControl::close(&*relay, env) => panic!("the close cannot finish while the lock is held"),
        _ = tokio::task::yield_now() => {}
    }
    drop(lifecycle);

    tokio::time::timeout(Duration::from_secs(5), async {
        while relay.connection(env).is_some() {
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("the abandoned close finished on its own");

    assert!(relay.shared.keys.lock_or_panic().iter().all(|k| k.env != env));
    assert_eq!(connection.state(), HubState::Disconnected);

    // The next open of the environment creates a new connection.
    let next = relay.get_or_create(&phone, TraceContext::new_root(Vec::new()));
    assert!(!Arc::ptr_eq(&connection, &next));
}

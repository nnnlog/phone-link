use super::*;
use phonelink_core::types::Environment;

fn item(extra: &str) -> NotificationItem {
    NotificationItem::parse(&format!(
        r#"{{"key":"k","packageName":"com.a","postTime":1760781600123,"title":"T","text":"x","appName":"A"{extra}}}"#
    ))
    .unwrap()
}

fn input() -> DecisionInput {
    DecisionInput {
        is_new: true,
        duplicate: (false, false),
        pc_permission: true,
        device_permission: true,
        group_alerts_allowed: false,
        muted_all: false,
        app_muted: false,
        app_flags: AppFlags::ALL,
        key_window: false,
        now_ms: 1_760_781_600_123,
        recency_ms: 300_000,
    }
}

#[test]
fn decision_rules() {
    let important = item(r#","importance":4"#);
    assert_eq!(decide(&important, &input()), Ok(()));
    assert_eq!(decide(&item(""), &input()), Err(NotShown::NotImportant), "absent importance is 3");
    assert_eq!(decide(&item(r#","importance":5,"notificationClass":2"#), &input()), Err(NotShown::MediaStyle));
    assert_eq!(decide(&item(r#","importance":5,"notificationClass":1"#), &input()), Err(NotShown::Messaging));

    let once = item(r#","importance":4,"flags":8"#);
    assert_eq!(decide(&once, &DecisionInput { is_new: false, ..input() }), Err(NotShown::AlertOnce));
    assert_eq!(decide(&once, &input()), Ok(()));

    let multi = item(r#","importance":4,"flags":8,"template":"$InboxStyle","textLines":["a"]"#);
    assert_eq!(decide(&multi, &DecisionInput { is_new: false, ..input() }), Ok(()));

    assert_eq!(decide(&important, &DecisionInput { duplicate: (true, false), ..input() }), Ok(()));
    assert_eq!(decide(&important, &DecisionInput { duplicate: (true, true), ..input() }), Err(NotShown::Duplicate));

    assert_eq!(
        decide(&important, &DecisionInput { device_permission: false, ..input() }),
        Err(NotShown::PermissionNotGranted)
    );

    assert_eq!(decide(&important, &DecisionInput { muted_all: true, ..input() }), Err(NotShown::MutedAll));
    assert_eq!(decide(&important, &DecisionInput { app_muted: true, ..input() }), Err(NotShown::AppMuted));
    assert_eq!(
        decide(&important, &DecisionInput { app_flags: AppFlags::NONE, ..input() }),
        Err(NotShown::AppSettingDisabled)
    );
    assert_eq!(decide(&important, &DecisionInput { app_flags: AppFlags::BANNER, ..input() }), Ok(()));
    assert_eq!(decide(&important, &DecisionInput { key_window: true, ..input() }), Err(NotShown::KeyWindow));

    let late = DecisionInput { now_ms: 1_760_781_600_123 + 300_001, ..input() };
    assert_eq!(decide(&important, &late), Err(NotShown::NotRecent));

    // Group messages: the Messages consent, and the feed flag alone.
    let group = item(r#","importance":4,"isReadOnlyGroupMessage":true"#);
    assert_eq!(decide(&group, &input()), Err(NotShown::PermissionNotGranted));
    let allowed = DecisionInput { group_alerts_allowed: true, ..input() };
    assert_eq!(decide(&group, &allowed), Ok(()));
    assert_eq!(
        decide(&group, &DecisionInput { app_flags: AppFlags::BANNER, ..allowed }),
        Err(NotShown::AppSettingDisabled)
    );

    assert!(popup_allowed(&group, AppFlags::FEED));
    assert!(!popup_allowed(&important, AppFlags::FEED));
}

#[test]
fn dedupe_window() {
    let mut d = Dedupe::default();
    let t0 = SystemTime::UNIX_EPOCH + Duration::from_secs(1_000);
    let w = Duration::from_secs(20);
    let a = item("");

    assert_eq!(d.check(&a, t0, w), (false, false));
    assert_eq!(d.check(&a, t0 + Duration::from_secs(19), w), (true, true));

    // Re-recorded at 19 s; 20 s after that it is gone.
    assert_eq!(d.check(&a, t0 + Duration::from_secs(39), w), (false, false));

    let other_ms =
        NotificationItem::parse(r#"{"key":"k","packageName":"com.a","postTime":1760781600124,"title":"T","text":"x"}"#)
            .unwrap();
    assert_eq!(d.check(&other_ms, t0 + Duration::from_secs(40), w), (true, false));
}

#[test]
fn throttling_counts_per_package_case_insensitively() {
    let cfg =
        ThrottleConfig { threshold: 5, monitoring_period: Duration::from_secs(10), cooldown: Duration::from_secs(180) };
    let mut t = Throttle::default();
    let t0 = SystemTime::UNIX_EPOCH + Duration::from_secs(1_000);

    for n in 0..4 {
        let p = if n % 2 == 0 { "com.A" } else { "com.a" };
        assert_eq!(t.process(p, t0 + Duration::from_secs(n), &cfg), ThrottleDecision::ShowNormally);
    }

    assert_eq!(t.process("com.a", t0 + Duration::from_secs(4), &cfg), ThrottleDecision::ShowSilentlyWithNotice);
    assert_eq!(t.process("com.a", t0 + Duration::from_secs(100), &cfg), ThrottleDecision::ShowSilently);
    assert_eq!(t.process("com.b", t0 + Duration::from_secs(100), &cfg), ThrottleDecision::ShowNormally);

    // The cooldown ends 180 s after it started; counting starts over.
    assert_eq!(t.process("com.a", t0 + Duration::from_secs(184), &cfg), ThrottleDecision::ShowNormally);

    // Alerts older than the monitoring period no longer count.
    let mut t = Throttle::default();
    for n in 0..4 {
        t.process("p", t0 + Duration::from_secs(n * 4), &cfg);
    }
    assert_eq!(t.process("p", t0 + Duration::from_secs(16), &cfg), ThrottleDecision::ShowNormally);
}

#[test]
fn content() {
    let phone = DcgClient::new("p1", Environment::Prod);
    let i = item(
        r#","importance":4,"notificationActions":[{"actionName":"Reply","isActionInlineReply":true,"actionIndex":1}]"#,
    );

    let n = banner(&phone, &i, true);
    assert_eq!(n.id, "notifications/p1.Prod/k");
    assert_eq!((n.title.as_str(), n.subtitle.as_deref(), n.body.as_str()), ("A", Some("T"), "x"));
    assert_eq!(n.thread_id.as_deref(), Some("com.a"));
    assert_eq!(n.actions.len(), 1);
    assert_eq!(n.actions[0].id, "reply/1/0");
    assert_eq!(n.actions[0].text_input.as_ref().unwrap().placeholder, "Reply");
    assert_eq!(n.user_info.get("importance").map(String::as_str), Some("4"));

    let p = paused_notice(&phone, "A", "com.a", Duration::from_secs(180), true);
    assert_eq!(p.id, "notifications/p1.Prod:paused/com.a");
    assert!(p.body.contains("3 minutes"));
}

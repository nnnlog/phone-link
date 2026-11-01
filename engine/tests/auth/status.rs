//! The error response and status mapping and the auth headers of every DCG call, through the REST
//! client against the fake DCG server.

use crate::support::auth_world;

use std::sync::Arc;

use auth_world::fake_dcg::error_body;
use auth_world::harness::Harness;
use phonelink_engine::auth::dcg_rest::{DcgRest, RestError, ServiceRequestStatus, TransportCause, WakeBody};
use phonelink_engine::codec::json::OrderedStringMap;
use phonelink_engine::config::Endpoints;
use phonelink_engine::edge::standins::FixedReachability;
use phonelink_engine::identification::ClientIdentification;
use phonelink_engine::tap::Tap;
use phonelink_engine::types::{Environment, TraceContext};

fn rest_for(h: &Harness, endpoints: Endpoints, reachability: Arc<FixedReachability>) -> DcgRest {
    let ident = Arc::new(ClientIdentification::load_or_create(&h.config, &*h.store).unwrap());
    let tap: Tap = h.tap.clone();
    DcgRest::new(DcgRest::http_client().unwrap(), endpoints, "FirstPartyAppsRepo", ident, reachability, tap)
        .with_policy(h.config.auth.rest.clone())
}

fn rest(h: &Harness) -> DcgRest {
    rest_for(h, h.config.endpoints.clone(), h.reachability.clone())
}

fn trace() -> TraceContext {
    TraceContext::new_root(Vec::new())
}

#[tokio::test]
async fn every_row_of_the_status_table() {
    use ServiceRequestStatus as S;
    let h = Harness::new().await;
    let rest = rest(&h);
    let html = "<html>Forbidden</html>";

    let rows: Vec<(u16, String, S)> = vec![
        (400, error_body(Some(10), "UnsupportedMarket"), S::UnsupportedMarketError),
        (400, error_body(Some(2), "InvalidRequest"), S::BadRequest),
        (400, String::new(), S::BadRequest),
        (401, error_body(Some(1), "InvalidAuthHeader"), S::MsaUnauthorizedRequest),
        (401, error_body(Some(4), "StrongAuth"), S::StrongAuthUnauthorizedRequest),
        (401, error_body(Some(3), "Other"), S::UnauthorizedRequest),
        (401, String::new(), S::UnauthorizedRequest),
        (403, error_body(Some(7), "LtwDisabled"), S::LtwDisabled),
        (403, error_body(Some(13), "Unlinked"), S::UnlinkedDevice),
        (403, error_body(Some(5), "Other"), S::ForbiddenDcgRequest),
        (403, error_body(None, "Uncategorized"), S::ForbiddenDcgRequest),
        (403, String::new(), S::ForbiddenDcgRequest),
        (403, html.to_string(), S::ForbiddenDcgRequest),
        (404, String::new(), S::NotFoundError),
        (429, String::new(), S::TooManyRequests),
        (500, String::new(), S::ServiceError),
        (503, error_body(Some(1), "Busy"), S::ServiceError),
        (409, String::new(), S::ClientError),
        (418, error_body(Some(13), "Teapot"), S::ClientError),
    ];

    for (http, body, expected) in rows {
        h.dcg.script("GetAccountCert", http, &body);
        let err = rest.account_cert(Environment::Prod, "msa", &trace()).await.unwrap_err();
        let RestError::Status(failure) = &err else { panic!("{http}: {err:?}") };
        assert_eq!(failure.status, expected, "HTTP {http} body {body:?}");
        assert_eq!(failure.http_status, http);
        assert_eq!(failure.body, body);
        assert_eq!(err.status(), Some(expected));
    }

    // Behavioural predicates.
    h.dcg.script("SignIn", 400, &error_body(Some(2), "InvalidNonce"));
    let e = rest.sign_in(Environment::Prod, "msa", "jwt", &trace()).await.unwrap_err();
    assert!(e.is_invalid_nonce() && !e.is_invalid_certificate());
    let RestError::Status(f) = &e else { unreachable!() };
    let detail = f.error.as_ref().unwrap();
    assert_eq!(
        (detail.category_code, detail.code.as_deref(), detail.message.as_deref()),
        (Some(2), Some("InvalidNonce"), Some("scripted"))
    );
    assert_eq!(detail.details, Some(serde_json::json!([])));

    h.dcg.script("SignIn", 400, &error_body(Some(2), "InvalidCertificate"));
    assert!(rest.sign_in(Environment::Prod, "msa", "jwt", &trace()).await.unwrap_err().is_invalid_certificate());
    h.dcg.script("SignIn", 403, &error_body(Some(13), "x"));
    assert!(rest.sign_in(Environment::Prod, "msa", "jwt", &trace()).await.unwrap_err().is_unlinked());
    h.dcg.script("SignIn", 403, "");
    assert!(rest.sign_in(Environment::Prod, "msa", "jwt", &trace()).await.unwrap_err().is_forbidden_uncategorized());
    h.dcg.script("SignIn", 401, &error_body(Some(4), "x"));
    assert!(rest.sign_in(Environment::Prod, "msa", "jwt", &trace()).await.unwrap_err().is_unauthorized());

    // InvalidNonce is a BadRequest code only.
    h.dcg.script("SignIn", 500, &error_body(Some(2), "InvalidNonce"));
    assert!(!rest.sign_in(Environment::Prod, "msa", "jwt", &trace()).await.unwrap_err().is_invalid_nonce());
}

#[tokio::test]
async fn transport_failures_map_by_their_cause_not_by_reachability() {
    let h = Harness::new().await;
    // A port nothing listens on: the connection cannot be established, an internet failure whatever
    // the path monitor reports.
    let closed = {
        let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        l.local_addr().unwrap()
    };

    let mut endpoints = h.config.endpoints.clone();
    endpoints.prod_base = format!("http://{closed}/");
    let reachability = Arc::new(FixedReachability::online());
    let rest = rest_for(&h, endpoints, reachability.clone());

    for online in [true, false] {
        reachability.set(online);
        let e = rest.account_cert(Environment::Prod, "msa", &trace()).await.unwrap_err();
        assert!(matches!(e, RestError::Transport { cause: TransportCause::Internet, .. }), "{e:?}");
        assert_eq!(e.status(), Some(ServiceRequestStatus::InternetError));
        assert!(e.is_internet());
    }
}

#[tokio::test]
async fn unexpected_success_bodies_fail_loudly() {
    let h = Harness::new().await;
    let rest = rest(&h);

    h.dcg.script("GetAccountCert", 200, "not json");
    assert!(matches!(
        rest.account_cert(Environment::Prod, "m", &trace()).await,
        Err(RestError::InvalidResponse { .. })
    ));

    h.dcg.script("GenerateNonce", 200, r#"{"epochTimeStamp":1}"#);
    assert!(
        matches!(
            rest.generate_nonce(Environment::Prod, "m", "d", &trace()).await,
            Err(RestError::InvalidResponse { .. })
        ),
        "a missing nonce"
    );

    for body in [
        r#"{"accessToken":"","deviceId":"d","epochExpirationTime":1}"#,
        r#"{"deviceId":"d","epochExpirationTime":1}"#,
        r#"{"accessToken":"t","epochExpirationTime":1}"#,
        r#"{"accessToken":"t","deviceId":"d"}"#,
    ] {
        h.dcg.script("SignIn", 200, body);
        assert!(
            matches!(
                rest.sign_in(Environment::Prod, "m", "jwt", &trace()).await,
                Err(RestError::InvalidResponse { .. })
            ),
            "{body}"
        );
    }

    h.dcg.script("SignIn", 200, r#"{"accessToken":"t","deviceId":"d","epochExpirationTime":5,"keyValidRemainingDays":-3,"tenantId":"x","extra":true}"#);
    let token = rest.sign_in(Environment::Prod, "m", "jwt", &trace()).await.unwrap();
    assert_eq!((token.access_token.as_str(), token.device_id.as_str(), token.epoch_expiration_time), ("t", "d", 5));
    assert_eq!(token.key_valid_remaining_days, Some(-3));

    // Empty tokens never leave the process.
    let before = h.dcg.requests().len();
    assert!(matches!(rest.device_info_list(Environment::Prod, "", &trace()).await, Err(RestError::Local { .. })));
    assert!(matches!(
        rest.unlink_device(Environment::Prod, "m", "", "p", &trace()).await,
        Err(RestError::Local { .. })
    ));
    assert_eq!(h.dcg.requests().len(), before);
}

#[tokio::test]
async fn auth_headers_per_call() {
    let h = Harness::new().await;
    let rest = rest(&h);

    // A services token the fake accepts on the Dcg-Token calls.
    h.dcg.with(|s| s.tokens.insert("svc".into(), "me".into()));
    rest.device_info_list(Environment::Prod, "msa", &trace()).await.unwrap();
    rest.account_cert(Environment::Prod, "msa", &trace()).await.unwrap();
    rest.unlink_device(Environment::Prod, "msa", "svc", "peer-1", &trace()).await.unwrap();

    let mut data = OrderedStringMap::default();
    data.push("DCG-Environment", "Prod");
    let body = WakeBody {
        device_id: "peer-1".into(),
        ttl: 20,
        priority: "normal".into(),
        collapse_key: "YPPWake".into(),
        data,
    };
    rest.wake(Environment::Prod, "svc", &body, &trace()).await.unwrap();

    let reqs = h.dcg.requests();
    let lines: Vec<String> = reqs.iter().map(|r| r.line()).collect();
    assert_eq!(
        lines,
        [
            "GET /DeviceAuthProxy/GetDeviceInfoList?api-version=1.5.0&filter=&top=20",
            "GET /DeviceAuthProxy/GetAccountCert?api-version=1.0.0",
            "POST /DeviceAuthProxy/UnlinkDevice?api-version=1.5.0&device-id=peer-1",
            "POST /Dispatcher/Wake?api-version=1.5.0",
        ]
    );

    let get = |op: &str| reqs.iter().find(|r| r.op() == op).unwrap().clone();
    for op in ["GetDeviceInfoList", "GetAccountCert"] {
        let r = get(op);
        assert_eq!(r.header("Authorization"), Some("Bearer msa"));
        assert_eq!(r.header("Authorization-Type"), Some("MSA"));
        assert_eq!(r.header("Dcg-Token"), None, "{op}: MSA only");
        assert_eq!(r.header("UserIdentityToken"), None);
    }

    let unlink = get("UnlinkDevice");
    assert_eq!(unlink.header("Authorization"), Some("Bearer msa"));
    assert_eq!(unlink.header("Dcg-Token"), Some("svc"));
    assert_eq!(unlink.body, "", "no body");

    let wake = get("Wake");
    assert_eq!(wake.header("Authorization"), Some("Bearer svc"), "services-token bearer");
    assert_eq!(wake.header("Authorization-Type"), None, "no MSA bearer, so no Authorization-Type");
    assert_eq!(wake.header("Dcg-Token"), None);
    assert_eq!(wake.header("AuthorizationPortal"), Some("FirstPartyAppsRepo"));
    assert_eq!(
        wake.body,
        r#"{"deviceId":"peer-1","ttl":20,"priority":"normal","collapseKey":"YPPWake","data":{"DCG-Environment":"Prod"}}"#
    );

    for r in &reqs {
        assert!(r.header("traceparent").is_some() && r.header("DCG-LogicalDeviceId").is_some(), "{}", r.path);
    }
}

#[tokio::test]
async fn pop_device_key_is_sanitized() {
    let h = Harness::new().await;
    let _provisioned = h.provisioned().await;
    let rest = rest(&h);

    let token = h.dcg.with(|s| s.tokens.keys().next().unwrap().clone());
    let body: phonelink_engine::auth::dcg_rest::DeviceInfoBody =
        serde_json::from_str(&h.dcg.of("EnrollDevice")[0].body).unwrap();

    let key = format!("PartnerDeviceKey~{}", "Ab9".repeat(21) + "x");
    rest.enroll_device(Environment::Prod, "msa", &token, &key, &body, &trace()).await.unwrap();
    rest.enroll_device(Environment::Prod, "msa", &token, "PartnerDeviceKey~bad", &body, &trace()).await.unwrap();

    let enrolls = h.dcg.of("EnrollDevice");
    assert_eq!(enrolls[1].query("pop-device-key").as_deref(), Some(key.as_str()));
    assert_eq!(enrolls[2].query("pop-device-key").as_deref(), Some(""), "an ill-formed key is blanked");
}

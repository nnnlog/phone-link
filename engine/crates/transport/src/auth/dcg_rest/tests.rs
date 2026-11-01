use super::*;
use phonelink_core::codec::json::OrderedStringMap;
use phonelink_core::net;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;

/// What a one-connection test server does once it accepted.
#[derive(Clone, Copy)]
enum Server {
    /// Reads the request, then closes with RST.
    Reset,
    /// Answers bytes that are no TLS record.
    NotTls,
    /// Reads the request and never answers.
    Silent,
    /// Answers a head announcing 10 body bytes, sends 3 and closes.
    BrokenBody,
}

async fn serve(server: Server) -> std::net::SocketAddr {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();

    tokio::spawn(async move {
        let (mut socket, _) = listener.accept().await.unwrap();
        let mut buf = [0u8; 4096];

        match server {
            Server::NotTls => {
                let _ = socket.write_all(b"HTTP/1.1 200 OK\r\n\r\n").await;
            }
            Server::Reset => {
                let _ = socket.read(&mut buf).await;
                socket.set_zero_linger().unwrap();
            }
            Server::Silent => {
                let _ = socket.read(&mut buf).await;
                std::future::pending::<()>().await;
            }
            Server::BrokenBody => {
                let _ = socket.read(&mut buf).await;
                let _ = socket.write_all(b"HTTP/1.1 200 OK\r\ncontent-length: 10\r\n\r\nabc").await;
            }
        }
    });

    addr
}

async fn failed_get(url: String, timeout: Duration) -> reqwest::Error {
    net::http_client(timeout).unwrap().get(url).send().await.unwrap_err()
}

/// Transport failures by cause: every failure to establish the connection and every internet
/// socket cause is `InternetError`, a timeout `Cancelled`, anything else no status.
#[tokio::test]
async fn transport_failures_are_classified_by_cause() {
    let long = Duration::from_secs(10);

    // The host name does not resolve.
    let e = failed_get("http://dcg.invalid/Dispatcher/Wake".into(), long).await;
    assert_eq!(TransportCause::of(&e), TransportCause::Internet, "{e:?}");

    // Refused.
    let closed = TcpListener::bind("127.0.0.1:0").await.unwrap().local_addr().unwrap();
    let e = failed_get(format!("http://{closed}/"), long).await;
    assert_eq!(TransportCause::of(&e), TransportCause::Internet, "{e:?}");

    // The TLS handshake fails.
    let addr = serve(Server::NotTls).await;
    let e = failed_get(format!("https://{addr}/"), long).await;
    assert_eq!(TransportCause::of(&e), TransportCause::Internet, "{e:?}");

    // Reset after the request went out.
    let addr = serve(Server::Reset).await;
    let e = failed_get(format!("http://{addr}/"), long).await;
    assert!(!e.is_connect(), "connected first: {e:?}");
    assert_eq!(TransportCause::of(&e), TransportCause::Internet, "{e:?}");

    // No answer within the timeout.
    let addr = serve(Server::Silent).await;
    let e = failed_get(format!("http://{addr}/"), Duration::from_millis(300)).await;
    assert_eq!(TransportCause::of(&e), TransportCause::TimedOut, "{e:?}");

    // A reply that breaks off without a network cause.
    let addr = serve(Server::BrokenBody).await;
    let reply = net::http_client(long).unwrap().get(format!("http://{addr}/")).send().await.unwrap();
    let e = reply.text().await.unwrap_err();
    assert_eq!(TransportCause::of(&e), TransportCause::Other, "{e:?}");
}

#[test]
fn transport_causes_map_to_internet_error_cancelled_or_no_status() {
    let failure = |cause| RestError::Transport { call: "Wake", cause, detail: String::new() };
    assert_eq!(failure(TransportCause::Internet).status(), Some(ServiceRequestStatus::InternetError));
    assert!(failure(TransportCause::Internet).is_internet());
    assert_eq!(failure(TransportCause::TimedOut).status(), Some(ServiceRequestStatus::Cancelled));
    assert!(!failure(TransportCause::TimedOut).is_internet());
    assert_eq!(failure(TransportCause::Other).status(), None);
}

#[test]
fn status_table() {
    use ServiceRequestStatus as S;
    assert_eq!(map_http_status(400, Some(10)), S::UnsupportedMarketError);
    assert_eq!(map_http_status(400, Some(3)), S::BadRequest);
    assert_eq!(map_http_status(400, None), S::BadRequest);
    assert_eq!(map_http_status(401, Some(1)), S::MsaUnauthorizedRequest);
    assert_eq!(map_http_status(401, Some(4)), S::StrongAuthUnauthorizedRequest);
    assert_eq!(map_http_status(401, Some(2)), S::UnauthorizedRequest);
    assert_eq!(map_http_status(401, None), S::UnauthorizedRequest);
    assert_eq!(map_http_status(403, Some(7)), S::LtwDisabled);
    assert_eq!(map_http_status(403, Some(13)), S::UnlinkedDevice);
    assert_eq!(map_http_status(403, Some(99)), S::ForbiddenDcgRequest);
    assert_eq!(map_http_status(403, None), S::ForbiddenDcgRequest);
    assert_eq!(map_http_status(404, None), S::NotFoundError);
    assert_eq!(map_http_status(429, None), S::TooManyRequests);
    assert_eq!(map_http_status(500, None), S::ServiceError);
    assert_eq!(map_http_status(503, Some(1)), S::ServiceError);
    assert_eq!(map_http_status(409, None), S::ClientError);
    assert_eq!(map_http_status(302, None), S::ClientError);
}

#[test]
fn pop_device_key_shape() {
    let good = format!("PartnerDeviceKey~{}", "a1".repeat(32));

    assert_eq!(sanitize_pop_device_key(&good), good);
    assert_eq!(sanitize_pop_device_key("PartnerDeviceKey~short"), "");
    assert_eq!(sanitize_pop_device_key(&format!("PartnerDeviceKey~{}", "-".repeat(64))), "");
    assert_eq!(sanitize_pop_device_key(""), "");
}

#[test]
fn wake_body_keeps_order() {
    let mut data = OrderedStringMap::default();
    data.push("DCG-Environment", "Prod");
    data.push("IgnoreDeviceDisabledStatus", "False");

    let body =
        WakeBody { device_id: "d".into(), ttl: 20, priority: "normal".into(), collapse_key: "YPPWake".into(), data };

    assert_eq!(
        serde_json::to_string(&body).unwrap(),
        r#"{"deviceId":"d","ttl":20,"priority":"normal","collapseKey":"YPPWake","data":{"DCG-Environment":"Prod","IgnoreDeviceDisabledStatus":"False"}}"#
    );
}

#[test]
fn secrets_are_redacted_in_bodies() {
    let v = tap_body(r#"{"accessToken":"abc","deviceId":"x","nested":[{"refresh_token":"r"}]}"#);

    assert_eq!(v["accessToken"], "<redacted 3 bytes>");
    assert_eq!(v["deviceId"], "x");
    assert_eq!(v["nested"][0]["refresh_token"], "<redacted 1 bytes>");
    assert_eq!(query_value("a b/c"), "a%20b%2Fc");
}

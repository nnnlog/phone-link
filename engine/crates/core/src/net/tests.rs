use super::*;

#[derive(Debug, thiserror::Error)]
#[error("wrapper")]
struct Wrapper(#[source] Box<dyn Error + Send + Sync>);

#[test]
fn socket_causes_count_anywhere_in_the_error_chain() {
    for kind in [
        io::ErrorKind::NetworkDown,
        io::ErrorKind::NetworkUnreachable,
        io::ErrorKind::HostUnreachable,
        io::ErrorKind::ConnectionAborted,
        io::ErrorKind::ConnectionReset,
    ] {
        assert!(is_internet_cause(&io::Error::from(kind)), "{kind:?}");
        assert!(is_internet_cause(&Wrapper(Box::new(io::Error::from(kind)))), "{kind:?} wrapped");
    }

    for kind in [io::ErrorKind::ConnectionRefused, io::ErrorKind::TimedOut, io::ErrorKind::Other] {
        assert!(!is_internet_cause(&io::Error::from(kind)), "{kind:?}");
    }

    let dns = NameResolutionFailed { host: "h.invalid".into(), source: io::Error::other("not known") };
    assert!(is_internet_cause(&Wrapper(Box::new(dns))));

    // Inside an `io::Error`, whose `source` skips the wrapped error.
    let dns = NameResolutionFailed { host: "h.invalid".into(), source: io::Error::other("not known") };
    assert!(is_internet_cause(&io::Error::other(dns)));
    assert!(!is_internet_cause(&io::Error::other(Wrapper(Box::new(io::Error::other("tls"))))));
}

#[tokio::test]
async fn a_name_that_does_not_resolve_is_a_typed_internet_cause() {
    let err = lookup("relayhub.invalid", 443).await.unwrap_err();
    assert_eq!(err.host, "relayhub.invalid");
    assert!(is_internet_cause(&err));
    assert_eq!(lookup("127.0.0.1", 9).await.unwrap(), vec!["127.0.0.1:9".parse().unwrap()]);

    let http = http_client(Duration::from_secs(10)).unwrap();
    let err = http.get("http://login.invalid/token").send().await.unwrap_err();
    assert!(is_internet_cause(&err), "{err:?}");
    assert!(request_unreached(&err));
}

#[tokio::test]
async fn a_refused_connection_is_unreached_but_no_internet_cause() {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    drop(listener);
    let http = http_client(Duration::from_secs(10)).unwrap();
    let err = http.get(format!("http://{addr}/token")).send().await.unwrap_err();
    assert!(!is_internet_cause(&err), "{err:?}");
    assert!(request_unreached(&err), "a connect failure");
}

#[tokio::test]
async fn a_reset_connection_is_an_internet_cause() {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();

    tokio::spawn(async move {
        let (mut socket, _) = listener.accept().await.unwrap();
        let mut buf = [0u8; 1024];
        let _ = tokio::io::AsyncReadExt::read(&mut socket, &mut buf).await;
        // Closing with a zero linger sends RST instead of FIN.
        socket.set_zero_linger().unwrap();
        drop(socket);
    });

    let http = http_client(Duration::from_secs(10)).unwrap();
    let err = http.get(format!("http://{addr}/token")).send().await.unwrap_err();

    assert!(is_internet_cause(&err), "{err:?}");
    assert!(request_unreached(&err));
}

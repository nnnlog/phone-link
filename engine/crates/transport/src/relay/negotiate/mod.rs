//! Negotiate and the transport URL, as the ASP.NET Core `SignalR` client library does it by default:
//! `POST <hub>negotiate?negotiateVersion=1` with the connection headers and the bearer token, an
//! Azure-style `{ url, accessToken }` redirect followed with its `accessToken` replacing the bearer,
//! and the WebSocket URL `<hub with ws/wss scheme>?id=<connectionToken>`.

use std::time::Duration;

use reqwest::Url;
use reqwest::header::{AUTHORIZATION, HeaderMap, HeaderName, HeaderValue};
use serde::Deserialize;

use super::RelayShared;
use super::connection::StartError;
use phonelink_core::tap::{Direction, TapEvent, TapLayer, redact_secret_header};
use phonelink_core::types::Environment;

/// Bound on negotiate redirects. Assumption (the protocol names none): the ASP.NET Core
/// client's limit of 100; past it the start fails loudly.
pub(crate) const MAX_REDIRECTS: usize = 100;

/// Bound on one negotiate request and on the WebSocket upgrade. Assumption (the protocol names none):
/// 100 s.
pub(crate) const HTTP_TIMEOUT: Duration = Duration::from_secs(100);

/// The outcome of negotiate: where the transport connects and with which bearer.
#[derive(Debug, Clone)]
pub(crate) struct Negotiated {
    /// `<hub with ws/wss scheme>?id=<connectionToken>` of the last (redirected) hub URL.
    pub transport_url: Url,
    /// The services token, or the `accessToken` of the last redirect that carried one.
    pub bearer: String,
    pub connection_id: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct NegotiateResponse {
    #[serde(default)]
    url: Option<String>,
    #[serde(default)]
    access_token: Option<String>,
    #[serde(default)]
    connection_id: Option<String>,
    #[serde(default)]
    connection_token: Option<String>,
    #[serde(default)]
    negotiate_version: Option<i64>,
    #[serde(default)]
    available_transports: Option<Vec<AvailableTransport>>,
    #[serde(default)]
    error: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct AvailableTransport {
    transport: String,
    #[serde(default)]
    transfer_formats: Vec<String>,
}

/// `<hub>negotiate?negotiateVersion=1`, keeping any query the hub URL already has.
pub(crate) fn negotiate_url(hub: &Url) -> Url {
    let mut url = hub.clone();
    let mut path = url.path().to_string();
    if !path.ends_with('/') {
        path.push('/');
    }
    path.push_str("negotiate");
    url.set_path(&path);
    url.query_pairs_mut().append_pair("negotiateVersion", "1");
    url
}

/// `<hub with ws/wss scheme>?id=<connectionToken>`, keeping any query the hub URL already has.
pub(crate) fn transport_url(hub: &Url, connection_token: &str) -> Result<Url, StartError> {
    let mut url = hub.clone();
    let scheme = match url.scheme() {
        "http" | "ws" => "ws",
        "https" | "wss" => "wss",
        other => return Err(StartError::Protocol(format!("hub URL scheme {other:?} is not http(s)"))),
    };

    url.set_scheme(scheme)
        .map_err(|()| StartError::Protocol(format!("cannot switch the hub URL to the {scheme} scheme")))?;
    url.query_pairs_mut().append_pair("id", connection_token);
    Ok(url)
}

fn header_map(headers: &[(&'static str, String)], bearer: &str) -> Result<HeaderMap, StartError> {
    let mut map = HeaderMap::new();
    for (name, value) in headers {
        let name = HeaderName::from_bytes(name.as_bytes())
            .map_err(|e| StartError::Protocol(format!("header name {name:?}: {e}")))?;
        let value =
            HeaderValue::from_str(value).map_err(|e| StartError::Protocol(format!("header {name} value: {e}")))?;
        map.insert(name, value);
    }

    let bearer = HeaderValue::from_str(&format!("Bearer {bearer}"))
        .map_err(|e| StartError::Protocol(format!("bearer token is not a header value: {e}")))?;
    map.insert(AUTHORIZATION, bearer);
    Ok(map)
}

fn headers_json(map: &HeaderMap) -> serde_json::Value {
    serde_json::Value::Array(
        map.iter()
            .map(|(name, value)| {
                let value = String::from_utf8_lossy(value.as_bytes());
                serde_json::json!([name.as_str(), redact_secret_header(name.as_str(), &value)])
            })
            .collect(),
    )
}

/// The negotiate body for the tap, with a redirect's `accessToken` (a bearer) redacted.
fn body_json(body: &[u8]) -> serde_json::Value {
    match serde_json::from_slice::<serde_json::Value>(body) {
        Ok(mut json) => {
            if let Some(token) = json.get_mut("accessToken") {
                let len = token.as_str().map(str::len);
                if let Some(len) = len {
                    *token = serde_json::Value::String(format!("<redacted {len} bytes>"));
                }
            }
            json
        }
        Err(_) => serde_json::Value::String(String::from_utf8_lossy(body).into_owned()),
    }
}

async fn post(
    shared: &RelayShared,
    env: Environment,
    url: &Url,
    headers: &[(&'static str, String)],
    bearer: &str,
) -> Result<NegotiateResponse, StartError> {
    let map = header_map(headers, bearer)?;
    shared.tap.emit(TapEvent::new(
        TapLayer::Rest,
        Direction::Out,
        Some(env.name().to_string()),
        serde_json::json!({ "method": "POST", "url": url.as_str(), "headers": headers_json(&map) }),
        None,
    ));

    let response = shared
        .http
        .post(url.clone())
        .headers(map)
        // An empty POST: the hub answers 411 Length Required without the header, and an empty
        // body alone does not make the HTTP client send it.
        .header(reqwest::header::CONTENT_LENGTH, "0")
        .send()
        .await
        .map_err(|e| StartError::transport(format!("negotiate: {e}"), &e))?;

    let status = response.status();
    let body = response.bytes().await.map_err(|e| StartError::transport(format!("negotiate body: {e}"), &e))?;
    shared.tap.emit(TapEvent::new(
        TapLayer::Rest,
        Direction::In,
        Some(env.name().to_string()),
        serde_json::json!({ "url": url.as_str(), "status": status.as_u16(), "body": body_json(&body) }),
        None,
    ));

    if !status.is_success() {
        return Err(StartError::Http { stage: "negotiate", status: status.as_u16() });
    }
    serde_json::from_slice(&body)
        .map_err(|e| StartError::Protocol(format!("negotiate response is not the expected JSON: {e}")))
}

/// Runs negotiate against `hub_url`, following redirects, and returns the transport URL and bearer.
pub(crate) async fn negotiate(
    shared: &RelayShared,
    env: Environment,
    hub_url: &str,
    headers: &[(&'static str, String)],
    bearer: String,
) -> Result<Negotiated, StartError> {
    let mut hub = Url::parse(hub_url).map_err(|e| StartError::Protocol(format!("hub URL: {e}")))?;
    let mut bearer = bearer;
    let mut redirects = 0usize;

    loop {
        let url = negotiate_url(&hub);
        let response = post(shared, env, &url, headers, &bearer).await?;

        if let Some(error) = response.error {
            return Err(StartError::Protocol(format!("negotiate answered an error: {error}")));
        }

        if let Some(next) = response.url {
            redirects += 1;
            if redirects > MAX_REDIRECTS {
                return Err(StartError::Protocol(format!("negotiate redirected more than {MAX_REDIRECTS} times")));
            }
            hub = Url::parse(&next).map_err(|e| StartError::Protocol(format!("redirect URL: {e}")))?;
            if let Some(token) = response.access_token {
                bearer = token;
            }
            tracing::debug!(env = %env, redirect = %hub, "negotiate redirected to another hub");
            continue;
        }

        let transports = response.available_transports.unwrap_or_default();
        // Temporary diagnostic: what the server answered, so a failed hub start shows whether the
        // relay redirected, which negotiate version it speaks, and what it offered.
        tracing::warn!(
            env = %env,
            redirected = response.url.is_some(),
            negotiate_version = response.negotiate_version.unwrap_or(0),
            has_connection_token = response.connection_token.is_some(),
            has_url = response.url.is_some(),
            transports = ?transports.iter().map(|t| t.transport.as_str()).collect::<Vec<_>>(),
            "diagnostic: negotiate answered"
        );
        // Raw values to a plain file (the log's redaction hides them).
        {
            use std::io::Write;
            let _ = std::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open("/tmp/phone-link-relay.log")
                .and_then(|mut f| {
                    writeln!(
                        f,
                        "negotiate env={env} redirected={} version={} has_url={} has_token={} has_id={} transports={:?}",
                        response.url.is_some(),
                        response.negotiate_version.unwrap_or(0),
                        response.url.is_some(),
                        response.connection_token.is_some(),
                        response.connection_id.is_some(),
                        transports.iter().map(|t| t.transport.as_str()).collect::<Vec<_>>()
                    )
                });
        }
        let websockets =
            transports.iter().any(|t| t.transport == "WebSockets" && t.transfer_formats.iter().any(|f| f == "Binary"));
        if !websockets {
            return Err(StartError::Protocol(format!(
                "the hub offers no WebSockets transport with the Binary transfer format: {:?}",
                transports.iter().map(|t| &t.transport).collect::<Vec<_>>()
            )));
        }

        // negotiateVersion ≥ 1 answers a connectionToken; a version-0 server identifies the
        // connection by its connectionId (SignalR transport protocol).
        let token = if response.negotiate_version.unwrap_or(0) >= 1 {
            response
                .connection_token
                .ok_or_else(|| StartError::Protocol("negotiate response has no connectionToken".into()))?
        } else {
            response
                .connection_id
                .clone()
                .ok_or_else(|| StartError::Protocol("version-0 negotiate response has no connectionId".into()))?
        };

        let negotiated =
            Negotiated { transport_url: transport_url(&hub, &token)?, bearer, connection_id: response.connection_id };
        {
            use std::io::Write;
            let _ = std::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open("/tmp/phone-link-relay.log")
                .and_then(|mut f| writeln!(f, "transport_url={}", negotiated.transport_url));
        }
        return Ok(negotiated);
    }
}

#[cfg(test)]
mod tests;

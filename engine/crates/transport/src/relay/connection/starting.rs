//! Starting a session: negotiate, upgrade, handshake.

use super::{
    ActivityStream, Established, HANDSHAKE_TIMEOUT, HubConnection, ReceiveClock, StartError, WsStream, connect_tcp,
    ws_header,
};
use crate::relay::negotiate;
use crate::relay::negotiate::{HTTP_TIMEOUT, Negotiated};
use futures_util::{SinkExt, StreamExt};
use phonelink_core::codec::hub;
use phonelink_core::tap::{Direction, TapEvent, TapLayer, redact_secret_header};
use std::sync::Arc;
use tokio::time::Instant;
use tokio_tungstenite::tungstenite;
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use tokio_tungstenite::tungstenite::http::header::AUTHORIZATION;
use tokio_tungstenite::tungstenite::protocol::WebSocketConfig;
use tokio_util::sync::CancellationToken;

impl HubConnection {
    pub(super) fn start_breaker_open(&self) -> bool {
        self.inner().start_breaker_until.is_some_and(|until| Instant::now() < until)
    }

    /// The start retry policy: up to `1 + start_retry_attempts` starts `start_retry_sleep` apart,
    /// filtered by [`StartError::is_retryable`]; every attempt passes the 429 breaker. A start that
    /// failed with HTTP 401 invalidates the cached services token once, so the next attempt's token
    /// request auto-refreshes — no forced refresh flag leaves this path.
    pub(super) async fn start_with_retry(&self, stop: &CancellationToken) -> Result<Established, StartError> {
        let cfg = &self.shared.config.relay;
        let env = self.params.env;
        let mut retries_done: u32 = 0;

        loop {
            match self.start_through_breaker(false, stop).await {
                Ok(established) => return Ok(established),
                Err(error) if error.is_retryable() && retries_done < cfg.start_retry_attempts => {
                    // A 401 means the server rejected our cached token: drop it so the next start's
                    // token request refreshes automatically.
                    if retries_done == 0 && error.is_401() {
                        self.shared.tokens.invalidate_services_token(env).await;
                    }
                    retries_done += 1;
                    tracing::warn!(env = %env, %error, retry = retries_done, "hub start failed; retrying");
                    tokio::select! {
                        _ = stop.cancelled() => return Err(StartError::Cancelled),
                        _ = tokio::time::sleep(cfg.start_retry_sleep) => {}
                    }
                }
                Err(error) => return Err(error),
            }
        }
    }

    /// One start through the 429 breaker: refused while it is open, closing it on success and
    /// opening it on HTTP 429.
    async fn start_through_breaker(
        &self,
        force_refresh: bool,
        stop: &CancellationToken,
    ) -> Result<Established, StartError> {
        let env = self.params.env;
        let result = if self.start_breaker_open() {
            Err(StartError::BrokenCircuit)
        } else {
            self.start_once(force_refresh, stop).await
        };

        match result {
            Ok(established) => {
                self.inner().start_breaker_until = None;
                Ok(established)
            }
            Err(error) => {
                if error.is_429() {
                    // The start's error, logged by its caller, names the 429; later starts are refused
                    // with `BrokenCircuit` until the breaker closes.
                    tracing::debug!(env = %env, delay_ms = self.breaker_delay.as_millis(), "start circuit breaker opened");
                    self.inner().start_breaker_until = Some(Instant::now() + self.breaker_delay);
                }
                Err(error)
            }
        }
    }

    /// One start, cancellable by `stop`.
    pub(super) async fn start_once(
        &self,
        force_refresh: bool,
        stop: &CancellationToken,
    ) -> Result<Established, StartError> {
        tokio::select! {
            _ = stop.cancelled() => Err(StartError::Cancelled),
            result = self.start_inner(force_refresh) => result,
        }
    }

    pub(super) async fn start_inner(&self, force_refresh: bool) -> Result<Established, StartError> {
        let env = self.params.env;
        let token = self.shared.tokens.services_token(env, force_refresh).await.map_err(StartError::Token)?;
        if token.is_empty() {
            return Err(StartError::EmptyToken);
        }

        let headers = self.connection_headers();
        let hub_url = self.shared.config.endpoints.hub_url(env);
        let negotiated = negotiate::negotiate(&self.shared, env, &hub_url, &headers, token).await?;

        let (mut ws, received) = self.connect_transport(&negotiated, &headers).await?;
        let leftover = self.handshake(&mut ws).await?;
        Ok(Established { ws, leftover, received })
    }

    /// The WebSocket upgrade with the connection headers and `Authorization: Bearer …` as a header,
    /// over a TCP stream whose received bytes are clocked for the server timeout.
    pub(super) async fn connect_transport(
        &self,
        negotiated: &Negotiated,
        headers: &[(&'static str, String)],
    ) -> Result<(WsStream, Arc<ReceiveClock>), StartError> {
        let env = self.params.env;
        let mut request = negotiated
            .transport_url
            .as_str()
            .into_client_request()
            .map_err(|e| StartError::Protocol(format!("transport URL: {e}")))?;

        // Temporary diagnostic: the exact WebSocket target (query values redacted), so a 404 on
        // upgrade can be traced to the host/path being requested.
        {
            let uri = request.uri().to_string();
            {
                use std::io::Write;
                let _ = std::fs::OpenOptions::new()
                    .create(true)
                    .append(true)
                    .open("/tmp/phone-link-relay.log")
                    .and_then(|mut f| writeln!(f, "upgrade uri={uri}"));
            }
            let mut redacted_uri = uri;
            if let Some(pos) = redacted_uri.find('?') {
                let (head, query) = redacted_uri.split_at(pos);
                let redacted = query
                    .split('&')
                    .filter(|p| !p.is_empty())
                    .map(|p| p.split('=').next().unwrap_or(p).to_string() + "=<redacted>")
                    .collect::<Vec<_>>()
                    .join("&");
                redacted_uri = format!("{head}?{redacted}");
            }
            tracing::warn!(env = %env, uri = %redacted_uri, "diagnostic: relay upgrade request");
        }

        {
            let map = request.headers_mut();
            for (name, value) in headers {
                let (name, value) = ws_header(name, value)?;
                map.insert(name, value);
            }
            let (_, bearer) = ws_header(AUTHORIZATION.as_str(), &format!("Bearer {}", negotiated.bearer))?;
            map.insert(AUTHORIZATION, bearer);
        }

        let tapped_headers: Vec<serde_json::Value> = request
            .headers()
            .iter()
            .map(|(k, v)| {
                let v = String::from_utf8_lossy(v.as_bytes());
                serde_json::json!([k.as_str(), redact_secret_header(k.as_str(), &v)])
            })
            .collect();
        self.shared.tap.emit(TapEvent::new(
            TapLayer::Rest,
            Direction::Out,
            Some(env.name().to_string()),
            serde_json::json!({ "method": "GET", "url": negotiated.transport_url.as_str(), "headers": tapped_headers, "connectionId": negotiated.connection_id }),
            None,
        ));

        // No message or frame size limit: a DCG fragment may be as large as the fragment size (512 MiB),
        // and the SignalR client sets no hard limit either.
        let config = WebSocketConfig::default().max_message_size(None).max_frame_size(None);
        let received = ReceiveClock::new();
        let upgrade = async {
            let tcp = connect_tcp(&request).await?;
            let stream = ActivityStream { inner: tcp, received: received.clone() };
            // TLS (rustls with the webpki roots) for `wss`, as `connect_async` sets it up.
            tokio_tungstenite::client_async_tls_with_config(request, stream, Some(config), None).await.map_err(|e| {
                match e {
                    tungstenite::Error::Http(response) => {
                        StartError::Http { stage: "WebSocket upgrade", status: response.status().as_u16() }
                    }
                    other => StartError::transport(format!("WebSocket upgrade: {other}"), &other),
                }
            })
        };

        let (ws, response) = tokio::time::timeout(HTTP_TIMEOUT, upgrade)
            .await
            .map_err(|_| StartError::Transport("the WebSocket upgrade timed out".into()))??;
        tracing::debug!(env = %env, status = %response.status(), "WebSocket upgraded");
        Ok((ws, received))
    }

    /// Sends `{"protocol":"messagepack","version":2}` + `0x1E` as a binary message and reads the
    /// answer from text or binary messages; bytes after the separator are hub frames.
    pub(super) async fn handshake(&self, ws: &mut WsStream) -> Result<Vec<u8>, StartError> {
        let env = self.params.env;
        self.shared.tap.emit(TapEvent::new(
            TapLayer::HubMessage,
            Direction::Out,
            Some(env.name().to_string()),
            serde_json::json!({ "handshake": String::from_utf8_lossy(&hub::HANDSHAKE_REQUEST[..hub::HANDSHAKE_REQUEST.len() - 1]) }),
            Some(hub::HANDSHAKE_REQUEST),
        ));

        ws.send(Message::Binary(hub::HANDSHAKE_REQUEST.to_vec().into()))
            .await
            .map_err(|e| StartError::transport(format!("sending the handshake: {e}"), &e))?;

        let deadline = Instant::now() + HANDSHAKE_TIMEOUT;
        let mut buf = Vec::new();

        loop {
            let next = tokio::time::timeout_at(deadline, ws.next())
                .await
                .map_err(|_| StartError::Protocol("no handshake answer within the handshake timeout".into()))?;

            match next {
                Some(Ok(Message::Binary(bytes))) => buf.extend_from_slice(&bytes),
                Some(Ok(Message::Text(text))) => buf.extend_from_slice(text.as_bytes()),
                Some(Ok(Message::Close(frame))) => {
                    return Err(StartError::Transport(format!("WebSocket closed during the handshake: {frame:?}")));
                }
                Some(Ok(_)) => continue,
                Some(Err(e)) => return Err(StartError::transport(format!("reading the handshake: {e}"), &e)),
                None => return Err(StartError::Transport("WebSocket ended during the handshake".into())),
            }

            match hub::parse_handshake_response(&buf) {
                Ok(None) => continue,
                Ok(Some((response, used))) => {
                    self.shared.tap.emit(TapEvent::new(
                        TapLayer::HubMessage,
                        Direction::In,
                        Some(env.name().to_string()),
                        serde_json::json!({ "handshake": String::from_utf8_lossy(&buf[..used.saturating_sub(1)]) }),
                        Some(&buf[..used]),
                    ));

                    if let Some(error) = response.error {
                        return Err(StartError::HandshakeRejected(error));
                    }
                    return Ok(buf.split_off(used));
                }
                Err(e) => return Err(StartError::Protocol(e.to_string())),
            }
        }
    }
}

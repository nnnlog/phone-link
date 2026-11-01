//! The DCG REST calls of the control plane.
//!
//! It covers URLs with `?api-version=…` first, the seven `DCG-*` identification headers, the trace
//! headers, `AuthorizationPortal`, the per-call auth headers, the JSON bodies exactly as Phone Link
//! on Windows sends them, and the mapping of error responses to statuses — a request without a
//! usable reply is classified by [`TransportCause`].
//!
//! Every call goes
//! through the resilience policy of [`super::rest_policy`] and emits one REST [`TapEvent`] listing its
//! sends.
//! `/Registration/Register` is not implemented (the engine is PC-initiated only), nor
//! `DeleteDeviceInfo` only on the CLI's explicit request (Phone Link on Windows does not send it).

use std::sync::Arc;
use std::time::Duration;

use serde::Serialize;
use serde::de::DeserializeOwned;

use super::rest_policy::{self, PolicyError, RestPolicy, ServiceClient};
use phonelink_core::config::{DcgRestConfig, Endpoints};
use phonelink_core::constants::http_header as h;
use phonelink_core::edge::Reachability;
use phonelink_core::identification::ClientIdentification;
use phonelink_core::tap::{Direction, Tap, TapEvent, TapLayer, redact_secret_header};
use phonelink_core::types::{Environment, TraceContext};

mod devices;
mod identity;
mod pairing;

mod bodies;
mod status;

use bodies::{
    AccountCertResponse, CertificateJwtBody, DeviceIdBody, EnvironmentResponse, NonceResponse, RawEnrollResponse,
    RawTokenResponse, RotateKeysBody,
};
pub use bodies::{DeviceInfoBody, DeviceMetadata, EnrollDeviceResponse, TokenResponse, WakeBody};
#[cfg(feature = "test-support")]
pub use status::ErrorDetail;
use status::error_chain;
pub use status::{
    PolicyFailure, RestError, ServiceFailure, ServiceRequestStatus, TransportCause, map_http_status, parse_error_body,
};

/// Timeout of one HTTP request of the control plane's shared HTTP client, from its start to the end
/// of the reply body.
///
/// The MSA token requests have no other bound; a DCG REST call's attempts are cut
/// earlier by its resilience policy, so for them it bounds reading the reply body, which the policy
/// does not cover (Phone Link on Windows sets no bound there).
pub const REQUEST_TIMEOUT: Duration = Duration::from_secs(100);

// ------------------------------------------------------------------------------------------------
// The client
// ------------------------------------------------------------------------------------------------

/// The auth headers of one call.
#[derive(Clone, Copy)]
pub enum DcgAuth<'a> {
    /// `Authorization: Bearer <MSA>` + `Authorization-Type: MSA`.
    Msa(&'a str),
    /// The four `/Auth/*` calls: MSA + `UserIdentityType: MSA` + `UserIdentityToken: <MSA>`.
    MsaUserIdentity(&'a str),
    /// MSA + `Dcg-Token: <services token>` (`EnrollDevice`, `SetDeviceInfo`, `UnlinkDevice`).
    MsaWithDcgToken { msa: &'a str, services: &'a str },
    /// `Authorization: Bearer <services token>` (`Dispatcher`).
    Services(&'a str),
}

/// The `EnrollDevice` / `SetDeviceInfo` `PoP` device key: must match
/// `^PartnerDeviceKey~[a-zA-Z0-9]{64}$` or it is blanked.
pub fn sanitize_pop_device_key(key: &str) -> &str {
    match key.strip_prefix("PartnerDeviceKey~") {
        Some(rest) if rest.len() == 64 && rest.bytes().all(|b| b.is_ascii_alphanumeric()) => key,
        _ => "",
    }
}

/// Percent-encodes a query value (RFC 3986 unreserved characters pass).
fn query_value(v: &str) -> String {
    let mut out = String::with_capacity(v.len());
    for b in v.bytes() {
        if b.is_ascii_alphanumeric() || matches!(b, b'-' | b'.' | b'_' | b'~') {
            out.push(b as char);
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }

    out
}

/// Secret-bearing JSON keys replaced in tap events (bearer credentials only).
/// Bearer tokens, and `CloudRelay`'s TURN `credential`.
const SECRET_JSON_KEYS: [&str; 7] =
    ["accessToken", "access_token", "refresh_token", "id_token", "device_code", "refreshToken", "credential"];

/// Replaces bearer credentials inside a JSON value (tap events only).
pub fn redact_json_secrets(value: &mut serde_json::Value) {
    match value {
        serde_json::Value::Object(map) => {
            for (k, v) in map.iter_mut() {
                if SECRET_JSON_KEYS.contains(&k.as_str()) {
                    if let serde_json::Value::String(s) = v {
                        *v = serde_json::Value::String(format!("<redacted {} bytes>", s.len()));
                    }
                } else {
                    redact_json_secrets(v);
                }
            }
        }
        serde_json::Value::Array(items) => items.iter_mut().for_each(redact_json_secrets),
        _ => {}
    }
}

/// A body for a tap event: parsed JSON with secrets redacted, else the raw text.
pub fn tap_body(text: &str) -> serde_json::Value {
    if text.is_empty() {
        return serde_json::Value::Null;
    }

    match serde_json::from_str::<serde_json::Value>(text) {
        Ok(mut v) => {
            redact_json_secrets(&mut v);
            v
        }
        Err(_) => serde_json::Value::String(text.to_string()),
    }
}

/// The DCG REST client.
///
/// One `reqwest::Client`, the process's identification headers, the `AuthorizationPortal` of the
/// configured scope family, and the resilience policy, whose concurrency counts and circuits its
/// clones share.
///
/// `reachability` is only logged with a transport failure — it does not classify one. A failed
/// call's lines here are `debug`: the error carries the same text, and its caller logs or shows it.
#[derive(Clone)]
pub struct DcgRest {
    http: reqwest::Client,
    endpoints: Endpoints,
    portal: &'static str,
    ident: Arc<ClientIdentification>,
    reachability: Arc<dyn Reachability>,
    tap: Tap,
    policy: Arc<RestPolicy>,
}

struct Reply {
    status: u16,
    body: String,
}

impl DcgRest {
    pub fn new(
        http: reqwest::Client,
        endpoints: Endpoints,
        portal: &'static str,
        ident: Arc<ClientIdentification>,
        reachability: Arc<dyn Reachability>,
        tap: Tap,
    ) -> Self {
        let policy = Arc::new(RestPolicy::new(DcgRestConfig::default()));
        Self { http, endpoints, portal, ident, reachability, tap, policy }
    }

    /// The client with its own resilience policy of `config` (the default policy is the production one).
    pub fn with_policy(mut self, config: DcgRestConfig) -> Self {
        self.policy = Arc::new(RestPolicy::new(config));
        self
    }

    pub fn policy(&self) -> &RestPolicy {
        &self.policy
    }

    /// The shared HTTP client of the control plane, DCG REST and MSA alike (rustls, request
    /// timeout, typed name-resolution failures — [`phonelink_core::net::http_client`]).
    pub fn http_client() -> Result<reqwest::Client, reqwest::Error> {
        phonelink_core::net::http_client(REQUEST_TIMEOUT)
    }

    fn url(base: &str, path_and_query: &str, call: &'static str) -> Result<String, RestError> {
        if !base.ends_with('/') {
            return Err(RestError::Local { call, detail: format!("base URL {base:?} must end in '/'") });
        }
        Ok(format!("{base}{path_and_query}"))
    }

    fn headers(&self, auth: DcgAuth<'_>, trace: &TraceContext) -> Vec<(&'static str, String)> {
        let mut headers: Vec<(&'static str, String)> =
            self.ident.headers().iter().map(|(k, v)| (*k, v.to_string())).collect();

        let state = trace.tracestate();
        headers.push((h::TRACEPARENT, trace.traceparent()));
        headers.push((h::TRACESTATE, state.clone()));
        headers.push((h::DCG_TRACE_STATE, state));
        headers.push((h::AUTHORIZATION_PORTAL, self.portal.to_string()));

        match auth {
            DcgAuth::Msa(msa) => {
                headers.push((h::AUTHORIZATION, format!("Bearer {msa}")));
                headers.push((h::AUTHORIZATION_TYPE, h::MSA.to_string()));
            }
            DcgAuth::MsaUserIdentity(msa) => {
                headers.push((h::AUTHORIZATION, format!("Bearer {msa}")));
                headers.push((h::AUTHORIZATION_TYPE, h::MSA.to_string()));
                headers.push((h::USER_IDENTITY_TYPE, h::MSA.to_string()));
                headers.push((h::USER_IDENTITY_TOKEN, msa.to_string()));
            }
            DcgAuth::MsaWithDcgToken { msa, services } => {
                headers.push((h::AUTHORIZATION, format!("Bearer {msa}")));
                headers.push((h::AUTHORIZATION_TYPE, h::MSA.to_string()));
                headers.push((h::DCG_TOKEN, services.to_string()));
            }
            DcgAuth::Services(services) => {
                headers.push((h::AUTHORIZATION, format!("Bearer {services}")));
            }
        }

        headers
    }

    fn check_tokens(auth: DcgAuth<'_>, call: &'static str) -> Result<(), RestError> {
        let empty = match auth {
            DcgAuth::Msa(t) | DcgAuth::MsaUserIdentity(t) | DcgAuth::Services(t) => t.is_empty(),
            DcgAuth::MsaWithDcgToken { msa, services } => msa.is_empty() || services.is_empty(),
        };

        if empty {
            return Err(RestError::Local { call, detail: "empty token: never an anonymous request".into() });
        }
        Ok(())
    }

    /// A request that failed after it was built, classified by [`TransportCause`]; a request that
    /// could not be built is [`RestError::Local`] (nothing was sent).
    fn transport_failure(&self, call: &'static str, error: &reqwest::Error, context: &str) -> RestError {
        let detail = format!("{context}{}", error_chain(error));
        if error.is_builder() {
            return RestError::Local { call, detail };
        }

        let cause = TransportCause::of(error);
        let internet_available = self.reachability.is_internet_available();
        tracing::debug!(call, ?cause, internet_available, error = %detail, "no usable reply");
        RestError::Transport { call, cause, detail }
    }

    /// How the policy ended a call without a reply, as the call's error.
    fn policy_failure(&self, call: &'static str, error: PolicyError) -> RestError {
        let failure = match error {
            PolicyError::NoReply(e) => return self.transport_failure(call, &e, ""),
            PolicyError::TimedOut(bound) => {
                let detail = match bound {
                    rest_policy::Bound::Total(d) => format!("no reply within the call's {d:?}"),
                    rest_policy::Bound::Attempt(d) => format!("no reply within the last attempt's {d:?}"),
                };
                tracing::debug!(call, %detail, "timed out");
                return RestError::Transport { call, cause: TransportCause::TimedOut, detail };
            }
            PolicyError::BrokenCircuit { retry_after } => PolicyFailure::BrokenCircuit { retry_after },
            PolicyError::ConcurrencyLimit { limit } => PolicyFailure::ConcurrencyLimit { limit },
            PolicyError::ResendsExhausted { status, sends } => PolicyFailure::ResendsExhausted { status, sends },
            PolicyError::NegativeRetryAfter { value } => PolicyFailure::NegativeRetryAfter { value },
        };

        tracing::debug!(call, %failure, "ended by the resilience policy");
        RestError::Policy { call, failure }
    }

    #[expect(clippy::too_many_arguments, reason = "independent inputs of one operation; not yet grouped")]
    async fn send(
        &self,
        call: &'static str,
        client: ServiceClient,
        method: reqwest::Method,
        env_label: &str,
        url: String,
        auth: DcgAuth<'_>,
        trace: &TraceContext,
        body: Option<String>,
    ) -> Result<Reply, RestError> {
        self.send_with(call, client, method, env_label, url, auth, trace, Vec::new(), body).await
    }

    /// [`DcgRest::send`] with `extra` headers after the common ones.
    #[expect(clippy::too_many_arguments, reason = "independent inputs of one operation; not yet grouped")]
    async fn send_with(
        &self,
        call: &'static str,
        client: ServiceClient,
        method: reqwest::Method,
        env_label: &str,
        url: String,
        auth: DcgAuth<'_>,
        trace: &TraceContext,
        extra: Vec<(&'static str, String)>,
        body: Option<String>,
    ) -> Result<Reply, RestError> {
        Self::check_tokens(auth, call)?;

        let mut headers = self.headers(auth, trace);
        headers.extend(extra);
        if body.is_some() {
            headers.push(("Content-Type", "application/json".to_string()));
        } else if method == reqwest::Method::POST {
            // The HTTP client sends no length for an empty body, and the service answers a body-less
            // POST without one `411 Length Required`.
            headers.push(("Content-Length", "0".to_string()));
        }

        let request = |resend: Option<u32>| {
            let mut req = self.http.request(method.clone(), &url);
            for (k, v) in &headers {
                req = req.header(*k, v);
            }
            if let Some(n) = resend {
                req = req.header(rest_policy::RETRY_ATTEMPT, n.to_string());
            }
            req.body(body.clone().unwrap_or_default()).send()
        };

        let (result, steps) = self.policy.execute(client, request).await;
        let (outcome, status, resp_text, error) = match result {
            Err(e) => {
                let failure = self.policy_failure(call, e);
                let detail = failure.to_string();
                (Err(failure), None, None, Some(detail))
            }
            Ok(resp) => {
                let status = resp.status().as_u16();
                match resp.text().await {
                    Err(e) => {
                        let failure = self.transport_failure(call, &e, "reading the body: ");
                        let detail = failure.to_string();
                        (Err(failure), Some(status), None, Some(detail))
                    }
                    Ok(text) => {
                        let outcome = if (200..300).contains(&status) {
                            Ok(Reply { status, body: text.clone() })
                        } else {
                            let error = parse_error_body(&text);
                            let mapped = map_http_status(status, error.as_ref().and_then(|e| e.category_code));
                            Err(RestError::Status(Box::new(ServiceFailure {
                                call,
                                http_status: status,
                                status: mapped,
                                error,
                                body: text.clone(),
                            })))
                        };
                        (outcome, Some(status), Some(text), None)
                    }
                }
            }
        };

        let summary = serde_json::json!({
            "call": call,
            "method": method.as_str(),
            "url": url,
            "request_headers": headers
                .iter()
                .map(|(k, v)| serde_json::json!([k, redact_secret_header(k, v)]))
                .collect::<Vec<_>>(),
            "request_body": body.as_deref().map(tap_body).unwrap_or(serde_json::Value::Null),
            "sends": steps.iter().map(rest_policy::Step::to_json).collect::<Vec<_>>(),
            "status": status,
            "response_body": resp_text.as_deref().map(tap_body).unwrap_or(serde_json::Value::Null),
            "error": error,
        });
        self.tap.emit(TapEvent::new(TapLayer::Rest, Direction::Out, Some(env_label.to_string()), summary, None));
        outcome
    }

    fn parse<T: DeserializeOwned>(call: &'static str, reply: &Reply) -> Result<T, RestError> {
        serde_json::from_str(&reply.body)
            .map_err(|e| RestError::InvalidResponse { call, detail: format!("HTTP {}: {e}", reply.status) })
    }

    fn json<T: Serialize>(call: &'static str, body: &T) -> Result<String, RestError> {
        serde_json::to_string(body).map_err(|e| RestError::Local { call, detail: format!("body: {e}") })
    }

    fn base(&self, env: Environment) -> &str {
        self.endpoints.base(env)
    }
}

#[cfg(test)]
mod tests;

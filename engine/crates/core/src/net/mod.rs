//! The HTTP client every REST caller shares, and which transport failures the network caused.
//!
//! That decides what makes a failure's root cause "lack of internet connectivity" (the relay's
//! `OpenStatus` `InternetError`), what the engine's MSA token provider reports as unreachable, and
//! which DCG REST transport failures are `InternetError`
//! (`auth::dcg_rest::TransportCause`).
//!
//! The HTTP clients of the control plane and of the relay resolve host names through
//! [`TypedResolver`], and the relay's WebSocket connect through [`lookup`], so a name that did not
//! resolve stays recognizable as [`NameResolutionFailed`] in an error's cause chain;
//! [`is_internet_cause`] walks that chain.

use std::error::Error;
use std::io;
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

/// A host name did not resolve.
#[derive(Debug, thiserror::Error)]
#[error("resolving {host}: {source}")]
pub struct NameResolutionFailed {
    pub host: String,
    #[source]
    pub source: io::Error,
}

/// The addresses of `host:port` from the system resolver (`tokio::net::lookup_host`, which also
/// accepts an IP literal without a lookup).
pub async fn lookup(host: &str, port: u16) -> Result<Vec<SocketAddr>, NameResolutionFailed> {
    match tokio::net::lookup_host((host, port)).await {
        Ok(addrs) => Ok(addrs.collect()),
        Err(source) => Err(NameResolutionFailed { host: host.to_string(), source }),
    }
}

/// Resolves as reqwest's default resolver does — the system resolver, off the async threads — but
/// reports a failure as [`NameResolutionFailed`].
#[derive(Debug, Default, Clone, Copy)]
pub struct TypedResolver;

impl reqwest::dns::Resolve for TypedResolver {
    fn resolve(&self, name: reqwest::dns::Name) -> reqwest::dns::Resolving {
        let host = name.as_str().to_string();
        Box::pin(async move {
            let addrs = lookup(&host, 0).await?;
            Ok(Box::new(addrs.into_iter()) as reqwest::dns::Addrs)
        })
    }
}

/// An HTTP client (rustls) with `timeout` per request that resolves names through
/// [`TypedResolver`].
pub fn http_client(timeout: Duration) -> Result<reqwest::Client, reqwest::Error> {
    reqwest::Client::builder().timeout(timeout).dns_resolver(Arc::new(TypedResolver)).build()
}

/// The socket errors that count as a network cause: network down or unreachable, host unreachable,
/// connection aborted or reset.
fn is_internet_kind(kind: io::ErrorKind) -> bool {
    matches!(
        kind,
        io::ErrorKind::NetworkDown
            | io::ErrorKind::NetworkUnreachable
            | io::ErrorKind::HostUnreachable
            | io::ErrorKind::ConnectionAborted
            | io::ErrorKind::ConnectionReset
    )
}

/// `true` when `error` or one of its causes shows that the network is the problem.
///
/// A host name that did not resolve, the network down or unreachable, the host unreachable, or the
/// connection aborted or reset.
///
/// A refused connection, a TLS failure, a timeout or an HTTP status is not one.
pub fn is_internet_cause(error: &(dyn Error + 'static)) -> bool {
    let mut pending: Vec<&(dyn Error + 'static)> = vec![error];
    while let Some(e) = pending.pop() {
        if e.is::<NameResolutionFailed>() {
            return true;
        }

        if let Some(io) = e.downcast_ref::<io::Error>() {
            if is_internet_kind(io.kind()) {
                return true;
            }
            // `io::Error::source` skips a wrapped error and returns that error's own source.
            if let Some(inner) = io.get_ref() {
                pending.push(inner);
                continue;
            }
        }

        if let Some(source) = e.source() {
            pending.push(source);
        }
    }
    false
}

#[cfg(feature = "test-support")]
/// The engine's MSA token request got no HTTP response because of the network.
///
/// It could not be sent (name resolution, TCP connect, TLS handshake or proxy — reqwest's connect
/// errors), it timed out, or its connection failed for one of the [`is_internet_cause`] causes.
///
/// A request that could not be
/// built is a local error, never this.
pub fn request_unreached(error: &reqwest::Error) -> bool {
    !error.is_builder() && (error.is_connect() || error.is_timeout() || is_internet_cause(error))
}

#[cfg(test)]
mod tests;

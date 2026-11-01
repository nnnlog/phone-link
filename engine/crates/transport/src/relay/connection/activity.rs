//! Receive activity counted on the transport's bytes.

use super::StartError;
use phonelink_core::net;
use std::io;
use std::pin::Pin;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::task::{Context, Poll};
use std::time::Duration;
use tokio::io::{AsyncRead, AsyncWrite, ReadBuf};
use tokio::net::TcpStream;
use tokio::time::Instant;
use tokio_tungstenite::tungstenite::handshake::client::Request;

/// When the transport last received bytes. [`ActivityStream`] updates it beneath TLS and WebSocket
/// framing, so the server timeout (the ASP.NET Core `SignalR` client ends the connection when
/// "nothing is received for `ServerTimeout`") sees the bytes of a message that is still arriving, not
/// only completed messages.
pub(super) struct ReceiveClock {
    origin: Instant,
    /// Nanoseconds after `origin` of the last read that returned bytes (0: none yet).
    last_nanos: AtomicU64,
}

impl ReceiveClock {
    pub(super) fn new() -> Arc<Self> {
        Arc::new(Self { origin: Instant::now(), last_nanos: AtomicU64::new(0) })
    }

    fn touch(&self) {
        let nanos = u64::try_from(self.origin.elapsed().as_nanos()).unwrap_or(u64::MAX);
        self.last_nanos.fetch_max(nanos, Ordering::Relaxed);
    }

    /// The last read that returned bytes (the clock's creation when there was none).
    pub(super) fn last(&self) -> Instant {
        self.origin + Duration::from_nanos(self.last_nanos.load(Ordering::Relaxed))
    }
}

/// The TCP stream of a hub WebSocket, recording on a [`ReceiveClock`] every read that returns bytes.
pub(super) struct ActivityStream<S> {
    pub(super) inner: S,
    pub(super) received: Arc<ReceiveClock>,
}

impl<S: AsyncRead + Unpin> AsyncRead for ActivityStream<S> {
    fn poll_read(mut self: Pin<&mut Self>, cx: &mut Context<'_>, buf: &mut ReadBuf<'_>) -> Poll<io::Result<()>> {
        let before = buf.filled().len();
        let polled = Pin::new(&mut self.inner).poll_read(cx, buf);
        if matches!(polled, Poll::Ready(Ok(()))) && buf.filled().len() > before {
            self.received.touch();
        }
        polled
    }
}

impl<S: AsyncWrite + Unpin> AsyncWrite for ActivityStream<S> {
    fn poll_write(mut self: Pin<&mut Self>, cx: &mut Context<'_>, buf: &[u8]) -> Poll<io::Result<usize>> {
        Pin::new(&mut self.inner).poll_write(cx, buf)
    }

    fn poll_write_vectored(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        bufs: &[io::IoSlice<'_>],
    ) -> Poll<io::Result<usize>> {
        Pin::new(&mut self.inner).poll_write_vectored(cx, bufs)
    }

    fn is_write_vectored(&self) -> bool {
        self.inner.is_write_vectored()
    }

    fn poll_flush(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Pin::new(&mut self.inner).poll_flush(cx)
    }

    fn poll_shutdown(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Pin::new(&mut self.inner).poll_shutdown(cx)
    }
}

/// The TCP connection of the WebSocket upgrade, made as `tokio_tungstenite::connect_async` makes it:
/// the URI's host (IPv6 brackets stripped) and port (`ws` 80, `wss` 443), Nagle disabled. The host
/// is resolved first ([`net::lookup`]), so a name that does not resolve is a network failure.
pub(super) async fn connect_tcp(request: &Request) -> Result<TcpStream, StartError> {
    let uri = request.uri();
    let host = uri.host().ok_or_else(|| StartError::Protocol("the transport URL has no host".to_owned()))?;
    let host = host.strip_prefix('[').and_then(|h| h.strip_suffix(']')).unwrap_or(host);

    let port = uri
        .port_u16()
        .or(match uri.scheme_str() {
            Some("wss") => Some(443),
            Some("ws") => Some(80),
            _ => None,
        })
        .ok_or_else(|| StartError::Protocol("the transport URL is not ws or wss".to_owned()))?;

    let addrs =
        net::lookup(host, port).await.map_err(|e| StartError::transport(format!("WebSocket upgrade: {e}"), &e))?;
    let tcp = TcpStream::connect(addrs.as_slice())
        .await
        .map_err(|e| StartError::transport(format!("WebSocket upgrade: connecting to {host}:{port}: {e}"), &e))?;
    tcp.set_nodelay(true).map_err(|e| StartError::Transport(format!("WebSocket upgrade: TCP_NODELAY: {e}")))?;
    Ok(tcp)
}

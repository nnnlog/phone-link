//! The session task: reading, writing and the timeouts.

use super::{
    CLOSE_FRAME_TIMEOUT, Established, HubConnection, ReceiveClock, Session, SessionEnd, WsStream, effective_timeouts,
};
use futures_util::stream::{SplitSink, SplitStream};
use futures_util::{SinkExt, StreamExt};
use phonelink_core::codec::hub;
use phonelink_core::codec::hub::HubMessage;
use phonelink_core::tap::{Direction, TapEvent, TapLayer};
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::{mpsc, oneshot};
use tokio::time::Instant;
use tokio_tungstenite::tungstenite::Message;
use tokio_util::sync::CancellationToken;

pub(super) async fn run_session(
    conn: Arc<HubConnection>,
    session: Arc<Session>,
    established: Established,
    outbound: mpsc::UnboundedReceiver<Vec<u8>>,
) {
    let end = session_loop(&conn, &session, established, outbound).await;
    session.close_pending();
    if end != SessionEnd::Stopped {
        conn.on_session_end(&session, end).await;
    }
}

/// Runs one session: the reader on this task, the writer on its own, so a write that waits for a
/// full socket never stops reading (hub pings, heartbeats and completions keep being processed).
pub(super) async fn session_loop(
    conn: &Arc<HubConnection>,
    session: &Arc<Session>,
    established: Established,
    outbound: mpsc::UnboundedReceiver<Vec<u8>>,
) -> SessionEnd {
    let (keep_alive, server_timeout) = effective_timeouts(&conn.shared.config.relay);
    let Established { ws, leftover, received } = established;
    let (sink, stream) = ws.split();
    let reader_done = CancellationToken::new();
    let (writer_end_tx, mut writer_end) = oneshot::channel();

    let writer = tokio::spawn({
        let conn = conn.clone();
        let local_stop = session.cancel.clone();
        let reader_done = reader_done.clone();
        async move {
            let end = write_frames(conn, sink, outbound, keep_alive, local_stop, reader_done).await;
            // The reader may have ended the session already.
            let _ = writer_end_tx.send(end);
        }
    });

    let end = read_frames(conn, session, stream, leftover, &received, server_timeout, &mut writer_end).await;
    reader_done.cancel();

    // The writer stops at once (a local stop first sends the bounded close frame); the socket closes
    // when both halves are gone.
    if let Err(error) = writer.await {
        tracing::error!(env = %conn.params.env, session = session.id, %error, "hub WebSocket writer task failed");
    }
    end
}

/// Reads and dispatches hub frames until the session ends. The server timeout runs from the last
/// byte the socket delivered (or the session start): a message whose bytes are still arriving keeps
/// the connection, a socket that delivers nothing for `server_timeout` ends it.
pub(super) async fn read_frames(
    conn: &Arc<HubConnection>,
    session: &Arc<Session>,
    mut stream: SplitStream<WsStream>,
    mut buf: Vec<u8>,
    received: &ReceiveClock,
    server_timeout: Duration,
    writer_end: &mut oneshot::Receiver<SessionEnd>,
) -> SessionEnd {
    let started = Instant::now();
    let timeout_at = || received.last().max(started) + server_timeout;

    if let Some(end) = conn.process_buffer(session, &mut buf) {
        return end;
    }

    loop {
        let deadline = timeout_at();
        // Unbiased: neither a burst of inbound frames nor the timer starves the other.
        tokio::select! {
            _ = session.cancel.cancelled() => return SessionEnd::Stopped,
            end = &mut *writer_end => {
                return end.unwrap_or_else(|_| SessionEnd::Lost("the WebSocket writer ended".into()));
            }
            message = stream.next() => {
                match message {
                    Some(Ok(Message::Binary(bytes))) => buf.extend_from_slice(&bytes),
                    Some(Ok(Message::Text(text))) => buf.extend_from_slice(text.as_bytes()),
                    Some(Ok(Message::Close(frame))) => {
                        return SessionEnd::Lost(format!("the server closed the WebSocket: {frame:?}"));
                    }
                    // WebSocket ping/pong frames are answered by tungstenite.
                    Some(Ok(_)) => continue,
                    Some(Err(e)) => return SessionEnd::Lost(format!("reading the WebSocket: {e}")),
                    None => return SessionEnd::Lost("the WebSocket ended".into()),
                }

                if let Some(end) = conn.process_buffer(session, &mut buf) {
                    return end;
                }
            }
            _ = tokio::time::sleep_until(deadline) => {
                // Bytes that arrived after `deadline` was computed move it.
                if timeout_at() <= Instant::now() {
                    return SessionEnd::Lost(format!("nothing received from the hub for {server_timeout:?}"));
                }
            }
        }
    }
}

/// Writes the queued invocation frames and a client `Ping` whenever nothing was sent for
/// `keep_alive`. A local stop sends the WebSocket close frame (bounded); once the reader ended the
/// session the writer just stops, also in the middle of a write.
pub(super) async fn write_frames(
    conn: Arc<HubConnection>,
    mut sink: SplitSink<WsStream, Message>,
    mut outbound: mpsc::UnboundedReceiver<Vec<u8>>,
    keep_alive: Duration,
    local_stop: CancellationToken,
    reader_done: CancellationToken,
) -> SessionEnd {
    let env = conn.params.env;
    let mut last_sent = Instant::now();

    loop {
        let message = tokio::select! {
            biased;
            _ = local_stop.cancelled() => return send_close_frame(&mut sink).await,
            _ = reader_done.cancelled() => return SessionEnd::Stopped,
            frame = outbound.recv() => match frame {
                Some(frame) => Message::Binary(frame.into()),
                // The session (the sender's owner) is gone.
                None => return SessionEnd::Stopped,
            },
            _ = tokio::time::sleep_until(last_sent + keep_alive) => {
                let frame = hub::encode_frame(&HubMessage::Ping);
                conn.shared.tap.emit(TapEvent::new(
                    TapLayer::HubMessage,
                    Direction::Out,
                    Some(env.name().to_string()),
                    serde_json::json!([6]),
                    Some(&frame),
                ));
                Message::Binary(frame.into())
            }
        };

        tokio::select! {
            biased;
            _ = local_stop.cancelled() => return send_close_frame(&mut sink).await,
            _ = reader_done.cancelled() => return SessionEnd::Stopped,
            sent = sink.send(message) => {
                if let Err(e) = sent {
                    return SessionEnd::Lost(format!("writing to the WebSocket: {e}"));
                }
            }
        }

        last_sent = Instant::now();
    }
}

/// The WebSocket close frame of a locally stopped session. A stalled socket must not keep the
/// stopped session's task alive.
pub(super) async fn send_close_frame(sink: &mut SplitSink<WsStream, Message>) -> SessionEnd {
    let _ = tokio::time::timeout(CLOSE_FRAME_TIMEOUT, sink.send(Message::Close(None))).await;
    SessionEnd::Stopped
}

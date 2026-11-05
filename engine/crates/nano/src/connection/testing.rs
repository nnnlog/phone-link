//! A connection without sockets, for the unit tests of the layers above it.

use tokio::sync::{broadcast, mpsc, watch};

use super::{CloseReason, Command, ConnectionType, NanoConnection};
use crate::channel::RawChannel;
use crate::metrics;

/// An open connection that hands its owner `channels` as the phone's and closes (`UserEnded`) when
/// told to; the receiver turns `true` once a close was asked for.
pub(crate) fn offline(channels: Vec<RawChannel>) -> (NanoConnection, watch::Receiver<bool>) {
    let (cmd, mut commands) = mpsc::unbounded_channel();
    let (closed_tx, closed) = watch::channel(None);
    let (close_asked_tx, close_asked) = watch::channel(false);
    let (health, _) = broadcast::channel(4);

    tokio::spawn(async move {
        let mut channels = channels;
        // The owner's receiver of the phone's channels stays open until the connection closes.
        let mut forward = None;
        while let Some(command) = commands.recv().await {
            match command {
                Command::Forward(tx) => {
                    for c in channels.drain(..) {
                        let _ = tx.send(c);
                    }
                    forward = Some(tx);
                }
                Command::Close | Command::CloseLost { .. } => {
                    let _ = close_asked_tx.send(true);
                    let _ = closed_tx.send(Some(CloseReason::UserEnded));
                    drop(forward);
                    return;
                }
                _ => {}
            }
        }
    });

    let connection =
        NanoConnection { kind: ConnectionType::IceSrtp, cmd, health, closed, metrics: metrics::register() };
    (connection, close_asked)
}

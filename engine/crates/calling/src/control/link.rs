//! One RFCOMM channel to the phone's Hands-Free Audio Gateway and the AT dialogue on it.
//!
//! A reader on the blocking pool reads the channel, splits and parses the AG's lines and hands them
//! out: a final result and the information lines of the outstanding command complete that command,
//! everything else goes to the link's events. One command is outstanding at a time; a command
//! without a final result in time fails, and three such failures in a row close the channel.

use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::{Arc, Mutex, Weak};
use std::time::Duration;

use phonelink_bluetooth::{PhoneBluetooth, RfcommError};
use phonelink_core::sync::MutexExt;
use tokio::runtime::Handle;
use tokio::sync::{mpsc, oneshot};
use tracing::{debug, info, warn};

use super::at::{self, Final, Line, LineSplitter};

/// Commands in a row that may go unanswered before the channel is closed.
const UNANSWERED_LIMIT: u32 = 3;

/// What the link reports besides command answers.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum LinkEvent {
    /// A line no outstanding command asked for.
    Unsolicited(Line),
    /// The channel closed; no event follows.
    Closed,
}

/// Why a command got no final result.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub(crate) enum LinkError {
    #[error("the channel is closed")]
    Closed,
    #[error("the phone did not answer {0} in time")]
    TimedOut(String),
    #[error("writing to the channel failed: {0}")]
    Write(String),
}

/// A command's final result and the information lines that came before it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Response {
    pub(crate) result: Final,
    pub(crate) info: Vec<Line>,
}

/// The outstanding command.
struct Pending {
    prefix: Option<&'static str>,
    info: Vec<Line>,
    done: oneshot::Sender<Response>,
}

/// The open channel; dropping it closes the channel.
pub(crate) struct Link {
    edge: Arc<dyn PhoneBluetooth>,
    channel: u64,
    runtime: Handle,
    timeout: Duration,
    slot: tokio::sync::Mutex<()>,
    pending: Mutex<Option<Pending>>,
    unanswered: AtomicU32,
    closed: AtomicBool,
}

impl Link {
    /// Opens the channel to `service_uuid` on `address` (blocking, at most `open_timeout`) and starts
    /// its reader, which polls the channel every `read_poll`.
    pub(crate) fn open(
        edge: Arc<dyn PhoneBluetooth>,
        address: &str,
        service_uuid: &str,
        open_timeout: Duration,
        read_poll: Duration,
        command_timeout: Duration,
        runtime: &Handle,
    ) -> Result<(Arc<Link>, mpsc::UnboundedReceiver<LinkEvent>), RfcommError> {
        let channel = edge.rfcomm_open(
            address.to_string(),
            service_uuid.to_string(),
            phonelink_core::time::duration_ms(open_timeout),
        )?;
        info!("hands-free channel opened");

        let link = Arc::new(Link {
            edge,
            channel,
            runtime: runtime.clone(),
            timeout: command_timeout,
            slot: tokio::sync::Mutex::new(()),
            pending: Mutex::new(None),
            unanswered: AtomicU32::new(0),
            closed: AtomicBool::new(false),
        });

        let (events, received) = mpsc::unbounded_channel();
        let (reader, edge) = (Arc::downgrade(&link), link.edge.clone());
        runtime.spawn_blocking(move || read_loop(&reader, edge.as_ref(), channel, &events, read_poll));
        Ok((link, received))
    }

    /// Sends `command` and waits for its final result, collecting the information lines that start
    /// with `prefix`.
    pub(crate) async fn command(&self, command: &str, prefix: Option<&'static str>) -> Result<Response, LinkError> {
        let _slot = self.slot.lock().await;
        if self.closed.load(Ordering::SeqCst) {
            return Err(LinkError::Closed);
        }

        let (done, answer) = oneshot::channel();
        *self.pending.lock_or_recover() = Some(Pending { prefix, info: Vec::new(), done });
        let name = at::command_name(command);
        debug!(command = name, "hands-free command sent");

        let edge = self.edge.clone();
        let (channel, bytes) = (self.channel, format!("{command}\r").into_bytes());
        let written = self.runtime.spawn_blocking(move || edge.rfcomm_write(channel, bytes)).await;
        if let Err(e) = written.map_err(|e| e.to_string()).and_then(|r| r.map_err(|e| e.to_string())) {
            self.pending.lock_or_recover().take();
            return Err(LinkError::Write(e));
        }

        match tokio::time::timeout(self.timeout, answer).await {
            Ok(Ok(response)) => {
                self.unanswered.store(0, Ordering::SeqCst);
                Ok(response)
            }
            Ok(Err(_)) => Err(LinkError::Closed),
            Err(_) => {
                self.pending.lock_or_recover().take();
                if self.unanswered.fetch_add(1, Ordering::SeqCst) + 1 >= UNANSWERED_LIMIT {
                    warn!(command = name, "hands-free commands went unanswered; the channel is closed");
                    self.close();
                }
                Err(LinkError::TimedOut(name.to_string()))
            }
        }
    }

    /// Closes the channel; the reader then reports it closed. Closing twice does nothing.
    pub(crate) fn close(&self) {
        if !self.closed.swap(true, Ordering::SeqCst) {
            self.edge.rfcomm_close(self.channel);
        }
    }

    pub(crate) fn is_closed(&self) -> bool {
        self.closed.load(Ordering::SeqCst)
    }

    fn dispatch(&self, line: Line, events: &mpsc::UnboundedSender<LinkEvent>) {
        let mut pending = self.pending.lock_or_recover();
        if let Some(p) = pending.as_mut() {
            if let Line::Final(result) = line {
                if let Some(p) = pending.take() {
                    // A command that timed out meanwhile no longer listens.
                    let _ = p.done.send(Response { result, info: p.info });
                }
                return;
            }
            if p.prefix.is_some() && line.info_prefix() == p.prefix {
                p.info.push(line);
                return;
            }
        }
        drop(pending);

        if let Line::Other(text) = &line {
            debug!(kind = at::line_name(text), "hands-free line ignored");
        }
        // No receiver means the controller already dropped the link.
        let _ = events.send(LinkEvent::Unsolicited(line));
    }
}

impl Drop for Link {
    fn drop(&mut self) {
        self.close();
    }
}

/// Reads the channel until it closes or the link is dropped, handing each line to the link.
fn read_loop(
    link: &Weak<Link>,
    edge: &dyn PhoneBluetooth,
    channel: u64,
    events: &mpsc::UnboundedSender<LinkEvent>,
    poll: Duration,
) {
    let mut splitter = LineSplitter::default();
    let poll_ms = phonelink_core::time::duration_ms(poll);
    loop {
        let read = edge.rfcomm_read(channel, poll_ms);
        let Some(link) = link.upgrade() else { return };
        match read {
            Ok(bytes) if bytes.is_empty() => break,
            Ok(bytes) => {
                for line in splitter.push(&bytes) {
                    link.dispatch(at::parse(&line), events);
                }
            }
            Err(RfcommError::TimedOut) if !link.is_closed() => {}
            Err(RfcommError::TimedOut) => break,
            Err(e) => {
                warn!(error = %e, "hands-free channel read failed");
                break;
            }
        }
    }

    info!("hands-free channel closed");
    if let Some(link) = link.upgrade() {
        link.close();
        // The outstanding command, if any, ends with the channel.
        link.pending.lock_or_recover().take();
    }
    // No receiver means the controller already dropped the link.
    let _ = events.send(LinkEvent::Closed);
}

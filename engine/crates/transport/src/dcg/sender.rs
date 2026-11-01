//! The per-partner stop-and-wait sender, its per-partner state, and the constants of this layer.
//!
//! Per target partner: one priority queue (priority ascending, then `MessageId` ascending) and one
//! sender loop with exactly one fragment in flight. The loop takes an operation, takes its next
//! fragment and — when fragments remain — re-enqueues the operation before sending, so messages to
//! one partner interleave by priority and `MessageId`. A fragment try opens the hub connection,
//! checks (or wakes) the partner and invokes the hub; a `Failure` or `NotConnected` from the hub
//! repeats all three, twice, without delay.

use std::cmp::Ordering;
use std::collections::BinaryHeap;
use std::sync::{Arc, Mutex, Weak};

use tokio::sync::{Notify, oneshot};
use tokio_util::sync::CancellationToken;

use super::DcgTransport;
use super::ids::TargetIds;
use phonelink_core::codec::dcg_packet::{Ack, AckOutcome, Fragment, VERSION};
use phonelink_core::codec::hub::MultiplexPacket;
use phonelink_core::sync::MutexExt;
use phonelink_core::tap::Direction;
use phonelink_core::types::{
    ConnectReason, DcgClient, DcgSendStatus, HubSendResult, MessageType, OpenStatus, Opened, PartnerDisconnectReason,
    SendOptions, TraceContext,
};

/// One message on its way: the fragments of its serialized envelope and its policy.
pub(super) struct SendOp {
    message_id: i32,
    priority: u32,
    message_type: MessageType,
    options: SendOptions,
    connection_session_id: Option<String>,
    trace: TraceContext,
    fragments: Vec<Vec<u8>>,
    cancel: CancellationToken,
    state: Mutex<OpState>,
}

struct OpState {
    next_fragment: usize,
    /// `None` once the message has a result.
    result: Option<oneshot::Sender<DcgSendStatus>>,
}

impl SendOp {
    #[expect(clippy::too_many_arguments, reason = "independent inputs of one operation; not yet grouped")]
    pub(super) fn new(
        message_id: i32,
        priority: u32,
        message_type: MessageType,
        options: SendOptions,
        connection_session_id: Option<String>,
        trace: TraceContext,
        fragments: Vec<Vec<u8>>,
        cancel: CancellationToken,
        result: oneshot::Sender<DcgSendStatus>,
    ) -> Self {
        assert!(!fragments.is_empty(), "a message has at least one fragment");

        Self {
            message_id,
            priority,
            message_type,
            options,
            connection_session_id,
            trace,
            fragments,
            cancel,
            state: Mutex::new(OpState { next_fragment: 0, result: Some(result) }),
        }
    }

    fn state(&self) -> std::sync::MutexGuard<'_, OpState> {
        self.state.lock_or_panic()
    }

    fn is_finished(&self) -> bool {
        self.state().result.is_none()
    }

    fn take_next_fragment(&self) -> usize {
        let mut state = self.state();
        let index = state.next_fragment;
        state.next_fragment += 1;
        index
    }

    /// The message result; the first failure wins and later fragments are dropped.
    fn finish(&self, status: DcgSendStatus) {
        if let Some(tx) = self.state().result.take() {
            let _ = tx.send(status);
        }
    }
}

/// Queue order: priority ascending, then `MessageId` ascending (`BinaryHeap` is a max-heap, so the
/// comparison is reversed).
struct Queued(Arc<SendOp>);

impl Queued {
    fn key(&self) -> (u32, i32) {
        (self.0.priority, self.0.message_id)
    }
}

impl PartialEq for Queued {
    fn eq(&self, other: &Self) -> bool {
        self.key() == other.key()
    }
}

impl Eq for Queued {}

impl PartialOrd for Queued {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for Queued {
    fn cmp(&self, other: &Self) -> Ordering {
        other.key().cmp(&self.key())
    }
}

/// An ack as the receiver hands it to the sender loop, tagged with the trust check's result.
pub(super) struct ReceivedAck {
    pub ack: Ack,
    pub trusted: bool,
}

struct AckSlot {
    sequence_number: i32,
    tx: Option<oneshot::Sender<ReceivedAck>>,
}

/// Everything this transport keeps per target partner.
pub(super) struct Target {
    pub client: DcgClient,
    /// The target's `SessionId` and `SequenceNumber` counter, which belong to the process
    /// ([`super::DcgIdentifiers`]).
    pub ids: Arc<TargetIds>,
    queue: Mutex<BinaryHeap<Queued>>,
    notify: Notify,
    in_flight: Mutex<Option<AckSlot>>,
    /// Consecutive messages that ended `TimedOut` (the engine's partner-disconnect breaker).
    consecutive_timeouts: Mutex<u32>,
}

impl Target {
    pub(super) fn new(client: DcgClient, ids: Arc<TargetIds>) -> Self {
        Self {
            client,
            ids,
            queue: Mutex::new(BinaryHeap::new()),
            notify: Notify::new(),
            in_flight: Mutex::new(None),
            consecutive_timeouts: Mutex::new(0),
        }
    }

    pub(super) fn push(&self, op: Arc<SendOp>) {
        self.queue.lock_or_panic().push(Queued(op));
        self.notify.notify_one();
    }

    async fn next_op(&self) -> Arc<SendOp> {
        loop {
            let next = self.queue.lock_or_panic().pop();
            if let Some(Queued(op)) = next {
                return op;
            }
            self.notify.notified().await;
        }
    }

    /// Opens the ack slot of the fragment in flight; acks are matched by `SequenceNumber` only.
    fn expect_ack(&self, sequence_number: i32) -> AckWait<'_> {
        let (tx, rx) = oneshot::channel();
        *self.in_flight.lock_or_panic() = Some(AckSlot { sequence_number, tx: Some(tx) });
        AckWait { target: self, sequence_number, rx }
    }

    /// An ack from this partner (same `dcgClientId` and environment): delivered when its
    /// `SequenceNumber` is the one in flight; `SN − 1` is logged as late; anything else is ignored.
    pub(super) fn deliver_ack(&self, ack: Ack, trusted: bool) {
        let mut slot = self.in_flight.lock_or_panic();
        let sequence_number = ack.sequence_number;

        match slot.as_mut() {
            Some(waiting) if waiting.sequence_number == sequence_number => match waiting.tx.take() {
                Some(tx) => {
                    let _ = tx.send(ReceivedAck { ack, trusted });
                }
                None => tracing::debug!(partner = %self.client, sequence_number, "duplicate ack ignored"),
            },
            other => self.log_ignored_ack(sequence_number, other.map(|w| w.sequence_number)),
        }
    }

    /// Logs an ack that matches no fragment in flight: late (`SN − 1`), for another sequence number,
    /// or with nothing in flight (`waiting` is the in-flight sequence number).
    fn log_ignored_ack(&self, sequence_number: i32, waiting: Option<i32>) {
        match waiting {
            Some(waiting) if waiting.wrapping_sub(1) == sequence_number => {
                tracing::debug!(partner = %self.client, sequence_number, waiting, "late ack (SequenceNumber − 1) ignored");
            }
            Some(waiting) => {
                tracing::debug!(partner = %self.client, sequence_number, waiting, "ack for a SequenceNumber not in flight ignored");
            }
            None => {
                tracing::debug!(partner = %self.client, sequence_number, "ack with no fragment in flight ignored");
            }
        }
    }
}

/// The ack wait of one fragment; closes the slot when the fragment completes.
struct AckWait<'a> {
    target: &'a Target,
    sequence_number: i32,
    rx: oneshot::Receiver<ReceivedAck>,
}

impl Drop for AckWait<'_> {
    fn drop(&mut self) {
        let mut slot = self.target.in_flight.lock_or_panic();
        if slot.as_ref().is_some_and(|s| s.sequence_number == self.sequence_number) {
            *slot = None;
        }
    }
}

/// How a fragment completed successfully.
enum FragmentDone {
    /// The partner acknowledged it.
    Acknowledged,
    /// `SkipPartnerAck`: the hub send succeeded and no ack was awaited.
    Sent,
}

/// The ack outcome: what an ack means for the waiting fragment.
fn ack_result(received: ReceivedAck) -> Result<FragmentDone, DcgSendStatus> {
    if !received.trusted {
        return Err(DcgSendStatus::LostTrustWithPartner);
    }
    match received.ack.outcome() {
        AckOutcome::Acknowledged => Ok(FragmentDone::Acknowledged),
        AckOutcome::RemoteTransportTooOld => Err(DcgSendStatus::RemoteTransportTooOld),
        AckOutcome::RemoteTransportTooNew => Err(DcgSendStatus::RemoteTransportTooNew),
        AckOutcome::RemoteTransportError { error_no } => {
            // The status the sender gets carries no ErrorNo.
            tracing::debug!(error_no, "failed ack");
            Err(DcgSendStatus::RemoteTransportError)
        }
    }
}

/// The sender loop of one target; ends with the transport.
pub(super) async fn sender_loop(transport: Weak<DcgTransport>, target: Arc<Target>, shutdown: CancellationToken) {
    loop {
        let op = tokio::select! {
            _ = shutdown.cancelled() => return,
            op = target.next_op() => op,
        };
        let Some(transport) = transport.upgrade() else { return };
        transport.process_op(&target, op).await;
    }
}

impl DcgTransport {
    /// Takes the next fragment of `op` and carries it through sequence number, attempts, ack wait and
    /// the message result.
    async fn process_op(&self, target: &Arc<Target>, op: Arc<SendOp>) {
        if op.is_finished() {
            return;
        }
        if op.cancel.is_cancelled() {
            op.finish(DcgSendStatus::Canceled);
            return;
        }

        let index = op.take_next_fragment();
        let count = op.fragments.len();
        let last = index + 1 == count;
        if !last {
            target.push(op.clone());
        }

        let sequence_number = target.ids.next_sequence_number();
        match self.send_fragment(target, &op, index, sequence_number).await {
            Ok(done) => {
                if matches!(done, FragmentDone::Acknowledged) {
                    *target.consecutive_timeouts.lock_or_panic() = 0;
                }
                if last {
                    op.finish(DcgSendStatus::Success);
                }
            }
            Err(status) => {
                // The status goes back to the sender, which logs the failure; this is its detail.
                tracing::debug!(partner = %target.client, message_id = op.message_id, fragment_id = index + 1, sequence_number, ?status, "message not delivered");
                if status == DcgSendStatus::TimedOut {
                    self.count_ack_timeout(target);
                }
                op.finish(status);
            }
        }
    }

    /// The engine's partner-disconnect breaker (differs from Phone Link on Windows):
    /// `partner_disconnect_ack_timeouts` consecutive `TimedOut` messages remove the partner with
    /// `TooManyAckTimeouts` (0 disables); any acknowledged fragment resets the count. The relay logs
    /// the removal with its reason.
    fn count_ack_timeout(&self, target: &Target) {
        let threshold = self.config.dcg.partner_disconnect_ack_timeouts;
        let mut count = target.consecutive_timeouts.lock_or_panic();
        *count += 1;

        if threshold == 0 || *count < threshold {
            return;
        }
        *count = 0;
        drop(count);
        self.hub.remove_partner(&target.client, PartnerDisconnectReason::TooManyAckTimeouts);
    }

    /// One fragment: up to `1 + ack_retry_count` attempts with the same `SequenceNumber`; each
    /// attempt is a whole try (open, partner check or wake, hub send with its retries).
    async fn send_fragment(
        &self,
        target: &Target,
        op: &SendOp,
        index: usize,
        sequence_number: i32,
    ) -> Result<FragmentDone, DcgSendStatus> {
        let fragment = Fragment {
            version: VERSION,
            session_id: target.ids.session_id.clone(),
            sequence_number,
            message_id: op.message_id,
            message_type_raw: Some(
                i32::try_from(op.message_type.wire_value()).expect("MessageType values are 0 and 1"),
            ),
            fragment_id: i32::try_from(index + 1).expect("FragmentId fits an int32"),
            fragment_count: i32::try_from(op.fragments.len()).expect("FragmentCount fits an int32"),
            raw: op.fragments[index].clone(),
        };

        let packet = fragment.to_packet();
        let mut ack = target.expect_ack(sequence_number);
        let attempts = 1 + self.config.dcg.ack_retry_count;

        for attempt in 1..=attempts {
            self.attempt(target, op, &packet).await?;
            if op.options.skip_partner_ack {
                return Ok(FragmentDone::Sent);
            }

            tokio::select! {
                _ = op.cancel.cancelled() => return Err(DcgSendStatus::Canceled),
                received = tokio::time::timeout(self.config.dcg.ack_timeout, &mut ack.rx) => match received {
                    Ok(Ok(received)) => return ack_result(received),
                    Ok(Err(_)) => {
                        tracing::error!(partner = %target.client, sequence_number, "ack slot of the fragment in flight replaced");
                        debug_assert!(false, "the ack slot of the fragment in flight was replaced");
                        return Err(DcgSendStatus::Canceled);
                    }
                    // The last attempt's timeout is the message's `TimedOut`, which its sender logs.
                    Err(_) if attempt < attempts => {
                        tracing::warn!(partner = %target.client, sequence_number, attempt, attempts, "fragment not acked within the ack timeout; sending it again");
                    }
                    Err(_) => {}
                },
            }
        }

        Err(DcgSendStatus::TimedOut)
    }

    /// One attempt: each try gets and opens the hub connection serving the target, checks that the hub
    /// reports the partner connected (or wakes it), then invokes the hub. A hub `Failure` or
    /// `NotConnected` repeats the whole try `hub_send_retries` times without delay — so a connection
    /// that dropped after the partner check is opened again and the partner checked again; any other
    /// failure, an open failure or a failed partner check or wake ends the fragment at once.
    async fn attempt(&self, target: &Target, op: &SendOp, packet: &MultiplexPacket) -> Result<(), DcgSendStatus> {
        let tries = 1 + self.config.dcg.hub_send_retries;
        let mut try_number = 0;

        loop {
            try_number += 1;
            self.open_connection(target, op).await?;
            self.ensure_partner_connected(target, op).await?;

            match self.hub_send(target, op, packet).await? {
                HubSendResult::Success => return Ok(()),
                result @ (HubSendResult::Failure | HubSendResult::NotConnected) if try_number < tries => {
                    tracing::warn!(partner = %target.client, ?result, try_number, "hub send of a fragment failed; trying again");
                }
                result => return Err(DcgSendStatus::HubSendFailed { result, cause: None }),
            }
        }
    }

    /// The attempt's open: an open connection is used as it is; a failed open ends the fragment. The open
    /// runs in its own task, as every hub open of the engine does, so a cancelled send stops waiting
    /// for it without abandoning it midway.
    async fn open_connection(&self, target: &Target, op: &SendOp) -> Result<(), DcgSendStatus> {
        let opener = self.opener.clone();
        let client = target.client.clone();
        let trace = op.trace.clone();
        let open = tokio::spawn(async move { opener.open_for(&client, ConnectReason::SendFragment, &trace).await });

        let opened = tokio::select! {
            _ = op.cancel.cancelled() => return Err(DcgSendStatus::Canceled),
            joined = open => joined.unwrap_or_else(|e| Opened { status: OpenStatus::Failure, cause: Some(format!("the hub open task failed: {e}")) }),
        };

        if opened.status == OpenStatus::Success {
            return Ok(());
        }
        Err(DcgSendStatus::open_failed(opened))
    }

    /// The attempt's partner check: the partner must be reported connected, or be woken (unless *skip
    /// wake*) — by the per-partner connection, with the message's own options and the default wake
    /// TTL ([`phonelink_core::seams::PartnerConnector`]).
    async fn ensure_partner_connected(&self, target: &Target, op: &SendOp) -> Result<(), DcgSendStatus> {
        if self.hub.is_partner_connected(&target.client) {
            return Ok(());
        }
        if op.options.skip_wake_if_not_connected {
            return Err(DcgSendStatus::PartnerNotConnected);
        }

        let Some(connector) = self.connector() else {
            // Without a connector a send fails `PartnerNotConnected` by design; its sender logs it.
            tracing::debug!(partner = %target.client, "partner not connected and no partner connector is set");
            return Err(DcgSendStatus::PartnerNotConnected);
        };

        tokio::select! {
            _ = op.cancel.cancelled() => Err(DcgSendStatus::Canceled),
            result = connector.connect_for_send(
                &target.client,
                op.options.ignore_device_disabled,
                self.config.platform.connection_timeout,
                &op.trace,
            ) => result,
        }
    }

    /// The attempt's hub invocation; its result goes to [`DcgTransport::attempt`].
    async fn hub_send(
        &self,
        target: &Target,
        op: &SendOp,
        packet: &MultiplexPacket,
    ) -> Result<HubSendResult, DcgSendStatus> {
        self.tap_packet(Direction::Out, &target.client, packet, op.connection_session_id.as_deref());
        tokio::select! {
            _ = op.cancel.cancelled() => Err(DcgSendStatus::Canceled),
            result = self.hub.send_packet(&target.client, &op.trace, packet.clone(), op.connection_session_id.clone()) => Ok(result),
        }
    }
}

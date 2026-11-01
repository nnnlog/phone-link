//! STUN and TURN transactions: sending, retransmitting and failing them.

use super::{
    CHECK_EXTRA_RETRIES, CLIENT_CONTEXT, IceAgent, IceEvent, PairState, Purpose, REQUESTED_TRANSPORT_UDP,
    TURN_LIFETIME, Transaction, Transmit, TurnState,
};
use crate::ice::candidate::{STUN_RETRY_COUNT, STUN_TIMEOUT_MS};
use crate::ice::stun;
use crate::ice::stun::{Class, Message, TransactionId, attr, method};
use std::net::SocketAddr;
use std::time::{Duration, Instant};
use tracing::{debug, warn};

impl IceAgent {
    pub(super) fn rto(&self) -> Duration {
        match self.rtt {
            Some(rtt) => rtt + (rtt * 2).clamp(Duration::from_millis(50), Duration::from_millis(500)),
            None => Duration::from_millis(u64::from(STUN_TIMEOUT_MS)),
        }
    }

    #[expect(clippy::too_many_arguments, reason = "independent inputs of one operation; not yet grouped")]
    pub(super) fn start_transaction(
        &mut self,
        now: Instant,
        purpose: Purpose,
        base: usize,
        to: SocketAddr,
        message: &Message,
        key: Option<&[u8]>,
        via_turn: Option<usize>,
    ) {
        let extra = if matches!(purpose, Purpose::Check { .. }) { CHECK_EXTRA_RETRIES } else { 0 };
        let data = message.encode(key, false);
        let rto = self.rto();

        let t = Transaction {
            purpose,
            base,
            to,
            data,
            sent: 0,
            max_sends: 1 + STUN_RETRY_COUNT + extra,
            rto,
            due: now,
            first_sent: now,
            via_turn,
        };

        self.transactions.insert(message.transaction, t);
        self.send_due(now);
    }

    pub(super) fn wrap_for_turn(&self, turn: usize, peer: SocketAddr, data: &[u8]) -> (usize, SocketAddr, Vec<u8>) {
        let t = &self.turns[turn];
        let wrapped = match t.channels.get(&peer) {
            Some(ch) => stun::channel_data(*ch, data),
            None => {
                let tx: TransactionId = rand::random();
                Message::new(Class::Indication, method::SEND, tx)
                    .with(attr::XOR_PEER_ADDRESS, stun::encode_xor_address(peer, &tx))
                    .with(attr::DATA, data.to_vec())
                    .encode(None, false)
            }
        };
        (t.base, t.server.address, wrapped)
    }

    pub(super) fn send_due(&mut self, now: Instant) {
        let mut failed = Vec::new();
        let ids: Vec<TransactionId> =
            self.transactions.iter().filter(|(_, t)| t.due <= now).map(|(id, _)| *id).collect();

        for id in ids {
            let t = self.transactions.get_mut(&id).expect("listed");
            if t.sent >= t.max_sends {
                failed.push(id);
                continue;
            }

            if t.sent > 0 {
                t.rto *= 2;
            } else {
                t.first_sent = now;
            }
            t.sent += 1;
            t.due = now + t.rto;

            let (base, to, data, via) = (t.base, t.to, t.data.clone(), t.via_turn);
            let transmit = match via {
                Some(turn) => {
                    let (base, server, wrapped) = self.wrap_for_turn(turn, to, &data);
                    Transmit { base, to: server, data: wrapped }
                }
                None => Transmit { base, to, data },
            };
            self.out.push_back(transmit);
        }

        for id in failed {
            let t = self.transactions.remove(&id).expect("listed");
            self.on_transaction_failed(now, t.purpose, t.to);
        }
    }

    pub(super) fn on_transaction_failed(&mut self, now: Instant, purpose: Purpose, to: SocketAddr) {
        match purpose {
            Purpose::Srflx { .. } => {
                debug!(server = %to, "STUN server did not answer");
                self.events.push_back(IceEvent::ServerTimedOut(to));
            }
            Purpose::Allocate { turn } | Purpose::Refresh { turn } => {
                warn!(server = %to, "TURN server did not answer; no relayed candidate from it");
                self.turns[turn].state = TurnState::Failed;
                self.events.push_back(IceEvent::ServerTimedOut(to));
            }
            Purpose::Permission { pair, .. } | Purpose::ChannelBind { pair, .. } | Purpose::Check { pair, .. } => {
                self.pairs[pair].state = PairState::Failed;
            }
        }

        self.check_gathering();
        self.check_failed(now);
    }

    pub(super) fn check_gathering(&mut self) {
        if self.gathering_done {
            return;
        }

        let pending =
            self.transactions.values().any(|t| matches!(t.purpose, Purpose::Srflx { .. } | Purpose::Allocate { .. }));
        if !pending {
            self.gathering_done = true;
            debug!(candidate_count = self.local.len(), "ICE gathering complete");
            self.events.push_back(IceEvent::GatheringComplete);
        }
    }

    pub(super) fn check_failed(&mut self, _now: Instant) {
        if self.failed || self.selected.is_some() || self.remote.is_none() {
            return;
        }

        let all_failed = !self.pairs.is_empty() && self.pairs.iter().all(|p| p.state == PairState::Failed);
        if all_failed || (self.pairs.is_empty() && self.gathering_done) {
            // The attempt fails with the counts in its reason, and whoever handles that failure
            // logs it.
            self.failed = true;
            self.events.push_back(IceEvent::Failed { pair_count: self.pairs.len(), candidate_count: self.local.len() });
        }
    }

    pub(super) fn send_allocate(&mut self, now: Instant, turn: usize) {
        let t = &self.turns[turn];
        let family = if self.bases[t.base].address.is_ipv4() { 1u8 } else { 2 };

        let mut m = Message::request(method::ALLOCATE)
            .with(attr::SOFTWARE, stun::SOFTWARE)
            .with(attr::REQUESTED_TRANSPORT, REQUESTED_TRANSPORT_UDP.to_vec())
            .with(attr::LIFETIME, TURN_LIFETIME.to_be_bytes().to_vec());
        if t.dont_fragment {
            m.add(attr::DONT_FRAGMENT, Vec::new());
        }
        m.add(attr::REQUESTED_ADDRESS_FAMILY, vec![family, 0, 0, 0]);
        m.add(attr::MS_CLIENT_CONTEXT, CLIENT_CONTEXT.to_vec());

        let key = self.authenticate(turn, &mut m);
        let (base, server) = (t.base, t.server.address);
        self.start_transaction(now, Purpose::Allocate { turn }, base, server, &m, key.as_ref().map(|k| &k[..]), None);
    }

    /// Adds the long-term credential attributes once a realm is known; returns the integrity key.
    pub(super) fn authenticate(&self, turn: usize, m: &mut Message) -> Option<[u8; 16]> {
        let t = &self.turns[turn];
        let key = t.key()?;
        m.add(attr::USERNAME, t.server.username.clone().into_bytes());
        m.add(attr::REALM, t.realm.clone().unwrap_or_default().into_bytes());
        m.add(attr::NONCE, t.nonce.clone().unwrap_or_default());
        Some(key)
    }

    pub(super) fn send_turn_request(&mut self, now: Instant, turn: usize, purpose: Purpose, mut m: Message) {
        m.add(attr::SOFTWARE, stun::SOFTWARE);
        let key = self.authenticate(turn, &mut m);
        let (base, server) = (self.turns[turn].base, self.turns[turn].server.address);
        self.start_transaction(now, purpose, base, server, &m, key.as_ref().map(|k| &k[..]), None);
    }
}

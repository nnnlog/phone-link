//! Candidate pairs and their connectivity checks.

use super::{IceAgent, MAX_PAIRS, Pair, PairState, Purpose, pair_priority};
use crate::ice::candidate::{AddressType, Candidate, CandidateType, SessionDescription};
use crate::ice::stun;
use crate::ice::stun::{Class, Message, TransactionId, attr, method};
use std::net::SocketAddr;
use std::time::Instant;
use tracing::debug;

impl IceAgent {
    /// Takes the phone's description: pairs are formed and checks start.
    pub fn set_remote(&mut self, now: Instant, remote: SessionDescription) {
        for c in &remote.candidates {
            debug!(
                address = %c.transport_text,
                base = %c.base_address,
                kind = ?c.kind,
                ipv6 = c.ipv6,
                address_type = ?c.address_type,
                raw_transport = %c.transport,
                "phone candidate"
            );
        }

        self.remote_password = remote.password.clone().into_bytes();
        self.remote_ufrag = remote.username.clone();
        self.remote_candidates = remote.candidates.clone();
        self.remote = Some(remote);

        for (base, source, priority) in std::mem::take(&mut self.early_sources) {
            self.learn_peer_reflexive(base, source, priority);
        }

        self.form_pairs();
        self.next_check = now;
        self.check_failed(now);
    }

    pub(super) fn pairable(local: &Candidate, remote: &Candidate) -> bool {
        local.transport == remote.transport
            && local.is_ipv6() == remote.is_ipv6()
            && (!local.is_ipv6()
                || (local.address_type == AddressType::LinkLocal) == (remote.address_type == AddressType::LinkLocal))
    }

    pub(super) fn form_pairs(&mut self) {
        for (li, l) in self.local.iter().enumerate() {
            // Server-reflexive candidates send from their base, which the host pair covers.
            if l.kind == CandidateType::ServerReflexive || (self.relay_only && l.kind != CandidateType::Relayed) {
                continue;
            }

            for (ri, r) in self.remote_candidates.iter().enumerate() {
                if !Self::pairable(l, r) || self.pairs.iter().any(|p| p.local == li && p.remote == ri) {
                    continue;
                }

                self.pairs.push(Pair {
                    local: li,
                    remote: ri,
                    priority: pair_priority(l.priority, r.priority),
                    state: PairState::Waiting,
                    nominating: false,
                    nominated: false,
                });
            }
        }

        // Keep the 100 highest; indexes of in-flight transactions stay valid because pairs are only
        // appended while no checks run, and truncation happens before any check.
        if self.transactions.values().all(|t| {
            !matches!(t.purpose, Purpose::Check { .. } | Purpose::Permission { .. } | Purpose::ChannelBind { .. })
        }) {
            self.pairs.sort_by_key(|p| std::cmp::Reverse(p.priority));
            self.pairs.truncate(MAX_PAIRS);
        }
    }

    pub(super) fn learn_peer_reflexive(&mut self, base: usize, source: SocketAddr, priority: u32) -> Option<usize> {
        if let Some(i) = self.remote_candidates.iter().position(|c| c.transport_address == source) {
            return Some(i);
        }

        if self.remote.is_none() {
            self.early_sources.push((base, source, priority));
            return None;
        }

        let mut c = Candidate::local(CandidateType::PeerReflexive, source, source, None, 0);
        c.priority = priority;
        c.foundation = u32::MAX - self.remote_candidates.len() as u32;
        self.remote_candidates.push(c);

        let ri = self.remote_candidates.len() - 1;
        let li = self.local_route.iter().position(|(b, turn)| *b == base && turn.is_none())?;
        if Self::pairable(&self.local[li], &self.remote_candidates[ri]) && self.pairs.len() < MAX_PAIRS {
            self.pairs.push(Pair {
                local: li,
                remote: ri,
                priority: pair_priority(self.local[li].priority, priority),
                state: PairState::Waiting,
                nominating: false,
                nominated: false,
            });
        }

        Some(ri)
    }

    pub(super) fn send_check(&mut self, now: Instant, pair: usize, nominate: bool) {
        let p = &self.pairs[pair];
        let local = &self.local[p.local];
        let remote = self.remote_candidates[p.remote].transport_address;
        let (base, turn) = self.local_route[p.local];

        if let Some(t) = turn {
            if !self.turns[t].permissions.contains(&remote.ip()) {
                self.pairs[pair].state = PairState::InProgress;
                let tx: TransactionId = rand::random();
                let mut permission_peer = remote;
                permission_peer.set_port(9);
                let m = Message::new(Class::Request, method::CREATE_PERMISSION, tx)
                    .with(attr::XOR_PEER_ADDRESS, stun::encode_xor_address(permission_peer, &tx));
                self.send_turn_request(now, t, Purpose::Permission { turn: t, peer: remote, pair }, m);
                return;
            }
        }

        let username = format!("{}:{}", self.remote_ufrag, self.credentials.ufrag);
        let mut m = Message::request(method::BINDING)
            .with(attr::USERNAME, username.into_bytes())
            .with(attr::ICE_CONTROLLING, self.credentials.tie_breaker.to_vec())
            .with(attr::PRIORITY, local.priority.to_be_bytes().to_vec());
        if nominate {
            m.add(attr::USE_CANDIDATE, Vec::new());
        }
        m.add(attr::TRANSACTION_TRANSMIT_COUNTER, stun::transmit_counter(1, 0));
        m.add(attr::SOFTWARE, stun::SOFTWARE);

        let key = self.remote_password.clone();
        self.pairs[pair].state =
            if self.pairs[pair].state == PairState::Succeeded { PairState::Succeeded } else { PairState::InProgress };
        if nominate {
            self.pairs[pair].nominating = true;
        }
        self.start_transaction(now, Purpose::Check { pair, nominate }, base, remote, &m, Some(&key), turn);
    }
}

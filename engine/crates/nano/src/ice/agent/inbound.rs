//! Timers and inbound datagrams: what the agent does when either arrives.

use super::{IceAgent, PairState, Purpose, TURN_LIFETIME, Transmit, TurnState};
use crate::ice::candidate::PACING_MS;
use crate::ice::stun;
use crate::ice::stun::{Class, Message, attr, method};
use std::net::SocketAddr;
use std::time::{Duration, Instant};
use tracing::debug;

impl IceAgent {
    pub fn poll_timeout(&self) -> Option<Instant> {
        let mut t = self.transactions.values().map(|t| t.due).min();
        if self.remote.is_some() && self.selected.is_none() && !self.failed && self.has_waiting() {
            t = Some(t.map_or(self.next_check, |x| x.min(self.next_check)));
        }

        for turn in &self.turns {
            if let Some(r) = turn.refresh_at {
                t = Some(t.map_or(r, |x| x.min(r)));
            }
        }

        t
    }

    pub(super) fn has_waiting(&self) -> bool {
        !self.triggered.is_empty() || self.pairs.iter().any(|p| p.state == PairState::Waiting)
    }

    pub fn handle_timeout(&mut self, now: Instant) {
        self.send_due(now);

        for i in 0..self.turns.len() {
            if self.turns[i].refresh_at.is_some_and(|r| r <= now) && self.turns[i].state == TurnState::Allocated {
                self.turns[i].refresh_at = None;
                let m = Message::request(method::REFRESH).with(attr::LIFETIME, TURN_LIFETIME.to_be_bytes().to_vec());
                self.send_turn_request(now, i, Purpose::Refresh { turn: i }, m);
            }
        }

        if self.remote.is_some() && self.selected.is_none() && !self.failed && now >= self.next_check {
            let next =
                self.triggered.pop_front().or_else(|| self.pairs.iter().position(|p| p.state == PairState::Waiting));
            if let Some(pair) = next {
                self.send_check(now, pair, false);
                self.next_check = now + Duration::from_millis(u64::from(PACING_MS));
            }
        }
    }

    /// One datagram from base `base`. Returns the application data (SRTP) of the selected pair; STUN
    /// and TURN traffic is consumed.
    pub fn handle_datagram(&mut self, now: Instant, base: usize, from: SocketAddr, data: &[u8]) -> Option<Vec<u8>> {
        // Datagrams from a TURN server: ChannelData or a Data indication carry a peer's datagram.
        if let Some(turn) = self.turns.iter().position(|t| t.base == base && t.server.address == from) {
            if let Some((channel, inner)) = stun::parse_channel_data(data) {
                let peer = self.turns[turn].channels.iter().find(|(_, c)| **c == channel).map(|(p, _)| *p)?;
                return self.relayed_datagram(now, turn, peer, inner.to_vec());
            }

            if let Ok(d) = Message::decode(data) {
                if d.message.class == Class::Indication && d.message.method == method::DATA {
                    let peer = d.message.xor_address(attr::XOR_PEER_ADDRESS)?;
                    let inner = d.message.get(attr::DATA)?.to_vec();
                    return self.relayed_datagram(now, turn, peer, inner);
                }
                self.on_stun(now, base, None, from, d);
                return None;
            }
            return None;
        }

        if stun::is_stun(data) {
            match Message::decode(data) {
                Ok(d) => self.on_stun(now, base, None, from, d),
                Err(e) => debug!(error = %e, source = %from, "STUN datagram dropped"),
            }
            return None;
        }

        self.application(base, None, from, data.to_vec())
    }

    pub(super) fn relayed_datagram(
        &mut self,
        now: Instant,
        turn: usize,
        peer: SocketAddr,
        inner: Vec<u8>,
    ) -> Option<Vec<u8>> {
        let base = self.turns[turn].base;

        if stun::is_stun(&inner) {
            if let Ok(d) = Message::decode(&inner) {
                self.on_stun(now, base, Some(turn), peer, d);
            }
            return None;
        }
        self.application(base, Some(turn), peer, inner)
    }

    pub(super) fn application(
        &self,
        base: usize,
        turn: Option<usize>,
        from: SocketAddr,
        data: Vec<u8>,
    ) -> Option<Vec<u8>> {
        let pair = &self.pairs[self.selected?];
        let (pbase, pturn) = self.local_route[pair.local];

        if pbase == base && pturn == turn && self.remote_candidates[pair.remote].transport_address == from {
            Some(data)
        } else {
            None
        }
    }

    pub(super) fn on_stun(
        &mut self,
        now: Instant,
        base: usize,
        turn: Option<usize>,
        from: SocketAddr,
        d: stun::Decoded,
    ) {
        match d.message.class {
            Class::Request if d.message.method == method::BINDING => self.on_check_request(now, base, turn, from, d),
            Class::Success | Class::Error => {
                let Some(t) = self.transactions.remove(&d.message.transaction) else {
                    return;
                };
                self.on_response(now, t, from, d);
            }
            _ => {}
        }
    }

    pub(super) fn on_check_request(
        &mut self,
        now: Instant,
        base: usize,
        turn: Option<usize>,
        from: SocketAddr,
        d: stun::Decoded,
    ) {
        if d.verify_integrity(self.credentials.password.as_bytes()).is_err() {
            debug!(source = %from, "connectivity check with bad integrity; dropped");
            return;
        }

        let m = &d.message;
        let mut response = Message::new(Class::Success, method::BINDING, m.transaction)
            .with(attr::XOR_MAPPED_ADDRESS, stun::encode_xor_address(from, &m.transaction))
            .with(attr::MAPPED_ADDRESS, stun::encode_address(from));
        if let Some(counter) = m.get(attr::TRANSACTION_TRANSMIT_COUNTER) {
            response.add(attr::TRANSACTION_TRANSMIT_COUNTER, counter.to_vec());
        }

        let bytes = response.encode(Some(self.credentials.password.as_bytes()), false);
        match turn {
            Some(t) => {
                let (b, server, wrapped) = self.wrap_for_turn(t, from, &bytes);
                self.out.push_back(Transmit { base: b, to: server, data: wrapped });
            }
            None => self.out.push_back(Transmit { base, to: from, data: bytes }),
        }

        if turn.is_some() || self.selected.is_some() {
            return;
        }

        let priority = m.u32_attribute(attr::PRIORITY).unwrap_or(0);
        if let Some(ri) = self.learn_peer_reflexive(base, from, priority) {
            let pair = self.pairs.iter().position(|p| p.remote == ri && self.local_route[p.local] == (base, None));
            if let Some(pair) = pair {
                if self.pairs[pair].state == PairState::Waiting || self.pairs[pair].state == PairState::Failed {
                    self.pairs[pair].state = PairState::Waiting;
                    self.triggered.push_back(pair);
                    self.next_check = self.next_check.min(now);
                }
            }
        }
    }
}

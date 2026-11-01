//! Responses to the agent's requests: checks, allocations, permissions and refreshes.

use super::{
    IceAgent, IceEvent, MAX_REDIRECTS, PairState, Purpose, TURN_LIFETIME, TURN_REFRESH_CAP, Transaction, Transmit,
    TurnState,
};
use crate::ice::candidate::{Candidate, CandidateType, classify, local_preference};
use crate::ice::stun;
use crate::ice::stun::{Class, Message, TransactionId, attr, method};
use std::net::SocketAddr;
use std::time::{Duration, Instant};
use tracing::{debug, info, warn};

impl IceAgent {
    pub(super) fn on_response(&mut self, now: Instant, t: Transaction, from: SocketAddr, d: stun::Decoded) {
        let m = &d.message;
        if t.sent == 1 {
            self.rtt = Some(now.saturating_duration_since(t.first_sent));
        }

        match t.purpose {
            Purpose::Srflx { base, server } => self.on_srflx_response(base, server, m),
            Purpose::Allocate { turn } => self.on_allocate_response(now, turn, m),
            Purpose::Refresh { turn } => self.on_refresh_response(now, turn, m),
            Purpose::Permission { turn, peer, pair } => self.on_permission_response(now, turn, peer, pair, m),
            Purpose::ChannelBind { turn, peer, channel, pair } => {
                self.on_channel_bind_response(now, turn, peer, channel, pair, m);
            }
            Purpose::Check { pair, nominate } => {
                if !self.on_check_response(now, t, from, &d, pair, nominate) {
                    return;
                }
            }
        }

        self.check_failed(now);
    }

    /// A server-reflexive binding response: a mapped address other than the base's becomes a local
    /// candidate; gathering may then be complete.
    fn on_srflx_response(&mut self, base: usize, server: SocketAddr, m: &Message) {
        if m.class == Class::Success {
            if let Some(mapped) = m.mapped_address() {
                let b = self.bases[base];
                if mapped != b.address {
                    self.add_local(
                        Candidate::local(
                            CandidateType::ServerReflexive,
                            mapped,
                            b.address,
                            Some(server),
                            local_preference(b.interface_index),
                        ),
                        base,
                        None,
                    );
                }
            }
        }

        self.check_gathering();
    }

    /// A TURN refresh response: the next refresh scheduled (a zero lifetime ends the allocation), or
    /// the refresh retried with a new nonce.
    fn on_refresh_response(&mut self, now: Instant, turn: usize, m: &Message) {
        if m.class == Class::Success {
            let lifetime = m.u32_attribute(attr::LIFETIME).unwrap_or(TURN_LIFETIME);
            debug!(lifetime, "TURN allocation refreshed");
            if lifetime == 0 {
                self.turns[turn].state = TurnState::Failed;
            } else {
                self.schedule_refresh(now, turn, lifetime);
            }
        } else if self.take_new_nonce(turn, m) {
            let r = Message::request(method::REFRESH).with(attr::LIFETIME, TURN_LIFETIME.to_be_bytes().to_vec());
            self.send_turn_request(now, turn, Purpose::Refresh { turn }, r);
        }
    }

    /// A TURN permission response: the pair waits for its check (again, after a new nonce) or fails.
    fn on_permission_response(&mut self, now: Instant, turn: usize, peer: SocketAddr, pair: usize, m: &Message) {
        if m.class == Class::Success {
            debug!(%peer, "TURN permission created");
            self.turns[turn].permissions.insert(peer.ip());
            self.pairs[pair].state = PairState::Waiting;
            self.triggered.push_front(pair);
            self.next_check = self.next_check.min(now);
        } else if m.error_code() == Some(403) {
            self.pairs[pair].state = PairState::Failed;
        } else if self.take_new_nonce(turn, m) {
            self.pairs[pair].state = PairState::Waiting;
            self.triggered.push_front(pair);
        } else {
            self.pairs[pair].state = PairState::Failed;
        }
    }

    /// A TURN channel-bind response: the bound channel completes the pair's check, a new nonce
    /// retries the bind, anything else fails the pair.
    fn on_channel_bind_response(
        &mut self,
        now: Instant,
        turn: usize,
        peer: SocketAddr,
        channel: u16,
        pair: usize,
        m: &Message,
    ) {
        self.turns[turn].pending_channels.remove(&peer);
        if m.class == Class::Success {
            debug!(%peer, channel, "TURN channel bound");
            self.turns[turn].channels.insert(peer, channel);
            self.check_succeeded(now, pair, self.pairs[pair].nominating);
        } else if self.take_new_nonce(turn, m) {
            self.bind_channel(now, turn, peer, pair);
        } else {
            self.pairs[pair].state = PairState::Failed;
        }
    }

    /// A connectivity-check response; `false` when it was not verified (or came from another
    /// address) and its transaction is kept waiting for the real one.
    fn on_check_response(
        &mut self,
        now: Instant,
        t: Transaction,
        from: SocketAddr,
        d: &stun::Decoded,
        pair: usize,
        nominate: bool,
    ) -> bool {
        let m = &d.message;
        if m.class != Class::Success {
            if m.error_code() == Some(487) {
                debug!("role conflict from the phone; the PC stays controlling");
            }
            self.pairs[pair].state = PairState::Failed;
        } else if d.verify_integrity(&self.remote_password).is_err() || from != t.to {
            debug!("check response not verified or from another address; dropped");
            self.transactions.insert(m.transaction, t);
            return false;
        } else {
            let (_, turn) = self.local_route[self.pairs[pair].local];
            let peer = self.remote_candidates[self.pairs[pair].remote].transport_address;

            match turn {
                Some(tn) if !self.turns[tn].channels.contains_key(&peer) => {
                    self.pairs[pair].nominating |= nominate;
                    self.bind_channel(now, tn, peer, pair);
                }
                _ => self.check_succeeded(now, pair, nominate),
            }
        }

        true
    }

    pub(super) fn bind_channel(&mut self, now: Instant, turn: usize, peer: SocketAddr, pair: usize) {
        if !self.turns[turn].pending_channels.insert(peer) {
            return;
        }

        let channel = self.turns[turn].next_channel;
        self.turns[turn].next_channel += 1;

        let tx: TransactionId = rand::random();
        let m = Message::new(Class::Request, method::CHANNEL_BIND, tx)
            .with(attr::CHANNEL_NUMBER, [channel.to_be_bytes().to_vec(), vec![0, 0]].concat())
            .with(attr::XOR_PEER_ADDRESS, stun::encode_xor_address(peer, &tx));
        self.send_turn_request(now, turn, Purpose::ChannelBind { turn, peer, channel, pair }, m);
    }

    pub(super) fn check_succeeded(&mut self, now: Instant, pair: usize, nominated: bool) {
        self.pairs[pair].state = PairState::Succeeded;

        if nominated {
            if self.selected.is_none() {
                self.pairs[pair].nominated = true;
                self.selected = Some(pair);

                let p = &self.pairs[pair];
                let local = self.local[p.local].transport_address;
                let remote = self.remote_candidates[p.remote].transport_address;
                info!(%local, %remote, kind = ?self.local[p.local].kind, "ICE pair selected");
                self.events.push_back(IceEvent::Selected { local, remote });
                self.transactions.retain(|_, t| !matches!(t.purpose, Purpose::Check { .. }));
            }
            return;
        }

        if self.selected.is_none() && !self.pairs.iter().any(|p| p.nominating) {
            self.send_check(now, pair, true);
        }
    }

    pub(super) fn take_new_nonce(&mut self, turn: usize, m: &Message) -> bool {
        match m.error_code() {
            Some(401 | 438) => {
                let t = &mut self.turns[turn];
                if let Some(realm) = m.text(attr::REALM) {
                    t.realm = Some(realm);
                }
                if let Some(nonce) = m.get(attr::NONCE) {
                    t.nonce = Some(nonce.to_vec());
                }
                t.realm.is_some() && t.nonce.is_some()
            }
            _ => false,
        }
    }

    pub(super) fn on_allocate_response(&mut self, now: Instant, turn: usize, m: &Message) {
        if m.class == Class::Success {
            let t = &mut self.turns[turn];
            t.state = TurnState::Allocated;
            t.relayed = m.xor_address(attr::XOR_RELAYED_ADDRESS);
            t.mapped = m.xor_address(attr::XOR_MAPPED_ADDRESS);
            let lifetime = m.u32_attribute(attr::LIFETIME).unwrap_or(TURN_LIFETIME);
            let (base, server, relayed, mapped) = (t.base, t.server.address, t.relayed, t.mapped);
            self.schedule_refresh(now, turn, lifetime);

            if let Some(relayed) = relayed {
                let b = self.bases[base];
                let mut c = Candidate::local(
                    CandidateType::Relayed,
                    relayed,
                    relayed,
                    Some(server),
                    local_preference(b.interface_index),
                );
                c.nat_address = mapped;
                c.relay_standard = Some("0".into());
                c.address_type = classify(relayed.ip());
                self.add_local(c, base, Some(turn));
            }

            self.check_gathering();
            return;
        }

        let code = m.error_code();
        let retry = match code {
            Some(401) if !self.turns[turn].challenged => {
                self.turns[turn].challenged = true;
                self.take_new_nonce(turn, m)
            }
            Some(438) => self.take_new_nonce(turn, m),
            Some(420) if self.turns[turn].dont_fragment => {
                self.turns[turn].dont_fragment = false;
                true
            }
            Some(300) => self.redirect(turn, m),
            _ => false,
        };

        if retry {
            self.send_allocate(now, turn);
        } else {
            warn!(?code, "TURN allocation failed; no relayed candidate from it");
            self.turns[turn].state = TurnState::Failed;
            self.check_gathering();
        }
    }

    /// Follows a 300's ALTERNATE-SERVER of the TURN server's address family: the allocation starts
    /// again there, without the old server's realm and nonce.
    pub(super) fn redirect(&mut self, turn: usize, m: &Message) -> bool {
        let t = &mut self.turns[turn];
        let Some(alternate) = m.get(attr::ALTERNATE_SERVER).and_then(stun::decode_address) else {
            return false;
        };
        if t.redirects >= MAX_REDIRECTS || alternate.is_ipv4() != t.server.address.is_ipv4() {
            return false;
        }

        debug!(from = %t.server.address, to = %alternate, "TURN allocation redirected");
        t.redirects += 1;
        t.server.address = alternate;
        t.realm = None;
        t.nonce = None;
        t.challenged = false;
        true
    }

    pub(super) fn schedule_refresh(&mut self, now: Instant, turn: usize, lifetime: u32) {
        let lifetime = Duration::from_secs(u64::from(lifetime));
        let next = lifetime.saturating_sub(Duration::from_secs(540)).max(lifetime / 2).min(TURN_REFRESH_CAP);
        self.turns[turn].refresh_at = Some(now + next);
    }

    /// Wraps an application datagram for the selected pair.
    pub fn send_data(&self, data: Vec<u8>) -> Option<Transmit> {
        let pair = &self.pairs[self.selected?];
        let (base, turn) = self.local_route[pair.local];
        let remote = self.remote_candidates[pair.remote].transport_address;

        Some(match turn {
            Some(t) => {
                let (b, server, wrapped) = self.wrap_for_turn(t, remote, &data);
                Transmit { base: b, to: server, data: wrapped }
            }
            None => Transmit { base, to: remote, data },
        })
    }

    #[cfg(feature = "test-support")]
    /// Whether the selected pair goes through a TURN relay.
    pub fn selected_is_relayed(&self) -> bool {
        self.selected.is_some_and(|p| self.local_route[self.pairs[p].local].1.is_some())
    }
}

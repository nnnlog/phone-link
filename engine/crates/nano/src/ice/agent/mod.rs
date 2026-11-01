//! The controlling ICE agent (sans-IO).
//!
//! It covers host, server-reflexive and relayed candidates; connectivity checks with regular
//! nomination; TURN allocations, permissions and channel bindings; and, once a pair is selected,
//! the demultiplexing of STUN from SRTP on it.
//!
//! The agent owns no socket. Bases are numbered; the connection reads datagrams from each base's
//! socket into [`IceAgent::handle_datagram`] and sends what [`IceAgent::poll_transmit`] returns.

use std::collections::{HashMap, HashSet, VecDeque};
use std::net::{IpAddr, SocketAddr};
use std::time::{Duration, Instant};

use base64::Engine as _;

use super::candidate::{Candidate, CandidateType, SessionDescription, assign_foundations, local_preference};
use super::stun::{self, Message, TransactionId, method};

mod checks;
mod inbound;
mod responses;
mod transactions;

/// Pairs the agent keeps at most.
const MAX_PAIRS: usize = 100;
/// Extra retransmissions a connectivity check gets.
const CHECK_EXTRA_RETRIES: u32 = 3;
/// TURN allocation lifetime requested.
const TURN_LIFETIME: u32 = 3600;
/// The longest wait between TURN refreshes.
const TURN_REFRESH_CAP: Duration = Duration::from_secs(40);
/// An all-zero GUID as text, carried by every Allocate.
const CLIENT_CONTEXT: &[u8] = b"00000000-0000-0000-0000-000000000000";
/// `REQUESTED-TRANSPORT` UDP.
const REQUESTED_TRANSPORT_UDP: [u8; 4] = [0x11, 0, 0, 0];
const FIRST_CHANNEL: u16 = 0x4000;
/// 300 (Try Alternate) redirects an allocation follows at most.
const MAX_REDIRECTS: u32 = 3;

/// One local socket the agent gathers on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Base {
    pub address: SocketAddr,
    pub interface_index: u32,
}

/// The TURN server and credentials of an attempt.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TurnServer {
    pub address: SocketAddr,
    pub username: String,
    pub password: String,
}

/// What the agent gathers with.
#[derive(Debug, Clone)]
pub struct IceConfig {
    pub stun: Vec<SocketAddr>,
    pub turn: Option<TurnServer>,
    /// Diagnostics: offer and pair only relayed candidates, so every path goes through the relay.
    /// Phone Link on Windows has no such mode.
    pub relay_only: bool,
}

/// One datagram to send from base `base`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Transmit {
    pub base: usize,
    pub to: SocketAddr,
    pub data: Vec<u8>,
}

/// What the agent reports.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum IceEvent {
    /// Every host, STUN and TURN preparation finished: the offer can go out.
    GatheringComplete,
    /// A pair was nominated: application data flows on it.
    Selected { local: SocketAddr, remote: SocketAddr },
    /// Every pair failed: the attempt fails with these counts in its reason.
    Failed { pair_count: usize, candidate_count: usize },
    /// A STUN or TURN request timed out (no candidate from that server).
    ServerTimedOut(SocketAddr),
}

/// ICE credentials and tie-breaker, fresh per attempt.
#[derive(Debug, Clone)]
pub struct Credentials {
    pub ufrag: String,
    pub password: String,
    pub tie_breaker: [u8; 8],
}

impl Credentials {
    pub fn random() -> Self {
        let b64 = base64::prelude::BASE64_STANDARD;
        Self {
            ufrag: b64.encode(rand::random::<[u8; 9]>()),
            password: b64.encode(rand::random::<[u8; 33]>()),
            tie_breaker: rand::random(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PairState {
    Waiting,
    InProgress,
    Succeeded,
    Failed,
}

#[derive(Debug, Clone)]
struct Pair {
    local: usize,
    remote: usize,
    priority: u64,
    state: PairState,
    nominating: bool,
    nominated: bool,
}

/// What a transaction is for.
#[derive(Debug, Clone)]
enum Purpose {
    Srflx { base: usize, server: SocketAddr },
    Allocate { turn: usize },
    Refresh { turn: usize },
    Permission { turn: usize, peer: SocketAddr, pair: usize },
    ChannelBind { turn: usize, peer: SocketAddr, channel: u16, pair: usize },
    Check { pair: usize, nominate: bool },
}

struct Transaction {
    purpose: Purpose,
    base: usize,
    to: SocketAddr,
    data: Vec<u8>,
    sent: u32,
    max_sends: u32,
    rto: Duration,
    due: Instant,
    first_sent: Instant,
    /// Sent through this TURN allocation (Send indication / `ChannelData`).
    via_turn: Option<usize>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TurnState {
    Allocating,
    Allocated,
    Failed,
}

struct TurnAllocation {
    base: usize,
    server: TurnServer,
    state: TurnState,
    realm: Option<String>,
    nonce: Option<Vec<u8>>,
    dont_fragment: bool,
    relayed: Option<SocketAddr>,
    mapped: Option<SocketAddr>,
    refresh_at: Option<Instant>,
    permissions: HashSet<IpAddr>,
    channels: HashMap<SocketAddr, u16>,
    pending_channels: HashSet<SocketAddr>,
    next_channel: u16,
    /// A 401 was answered with credentials; a second one fails the allocation.
    challenged: bool,
    /// 300 redirects followed so far.
    redirects: u32,
}

impl TurnAllocation {
    fn key(&self) -> Option<[u8; 16]> {
        self.realm.as_ref().map(|r| stun::long_term_key(&self.server.username, r, &self.server.password))
    }
}

/// The controlling agent.
pub struct IceAgent {
    pub credentials: Credentials,
    relay_only: bool,
    bases: Vec<Base>,
    local: Vec<Candidate>,
    /// For each local candidate: its sending base and, for relayed ones, the TURN allocation.
    local_route: Vec<(usize, Option<usize>)>,
    turns: Vec<TurnAllocation>,
    remote: Option<SessionDescription>,
    remote_candidates: Vec<Candidate>,
    remote_password: Vec<u8>,
    remote_ufrag: String,
    pairs: Vec<Pair>,
    transactions: HashMap<TransactionId, Transaction>,
    triggered: VecDeque<usize>,
    selected: Option<usize>,
    gathering_done: bool,
    failed: bool,
    rtt: Option<Duration>,
    next_check: Instant,
    out: VecDeque<Transmit>,
    events: VecDeque<IceEvent>,
    early_sources: Vec<(usize, SocketAddr, u32)>,
}

fn pair_priority(controlling: u32, controlled: u32) -> u64 {
    let (g, d) = (u64::from(controlling), u64::from(controlled));
    (1 << 32) * g.min(d) + 2 * g.max(d) + u64::from(g > d)
}

impl IceAgent {
    /// Creates the agent over `bases` and starts gathering.
    pub fn new(now: Instant, bases: Vec<Base>, config: IceConfig) -> Self {
        let mut agent = Self {
            credentials: Credentials::random(),
            relay_only: config.relay_only,
            bases,
            local: Vec::new(),
            local_route: Vec::new(),
            turns: Vec::new(),
            remote: None,
            remote_candidates: Vec::new(),
            remote_password: Vec::new(),
            remote_ufrag: String::new(),
            pairs: Vec::new(),
            transactions: HashMap::new(),
            triggered: VecDeque::new(),
            selected: None,
            gathering_done: false,
            failed: false,
            rtt: None,
            next_check: now,
            out: VecDeque::new(),
            events: VecDeque::new(),
            early_sources: Vec::new(),
        };

        for (i, base) in agent.bases.clone().iter().enumerate() {
            agent.add_local(
                Candidate::local(
                    CandidateType::Host,
                    base.address,
                    base.address,
                    None,
                    local_preference(base.interface_index),
                ),
                i,
                None,
            );

            for server in config.stun.iter().filter(|s| s.is_ipv4() == base.address.is_ipv4()) {
                let request = Message::request(method::BINDING);
                agent.start_transaction(
                    now,
                    Purpose::Srflx { base: i, server: *server },
                    i,
                    *server,
                    &request,
                    None,
                    None,
                );
            }
        }

        if let Some(turn) = config.turn {
            for (i, base) in agent.bases.clone().iter().enumerate() {
                if base.address.is_ipv4() != turn.address.is_ipv4() {
                    continue;
                }

                agent.turns.push(TurnAllocation {
                    base: i,
                    server: turn.clone(),
                    state: TurnState::Allocating,
                    realm: None,
                    nonce: None,
                    dont_fragment: true,
                    relayed: None,
                    mapped: None,
                    refresh_at: None,
                    permissions: HashSet::new(),
                    channels: HashMap::new(),
                    pending_channels: HashSet::new(),
                    next_channel: FIRST_CHANNEL,
                    challenged: false,
                    redirects: 0,
                });
                let t = agent.turns.len() - 1;
                agent.send_allocate(now, t);
            }
        }

        agent.check_gathering();
        agent
    }

    fn add_local(&mut self, candidate: Candidate, base: usize, turn: Option<usize>) {
        if self.local.iter().any(|c| c.transport_address == candidate.transport_address && c.kind == candidate.kind) {
            return;
        }

        self.local.push(candidate);
        self.local_route.push((base, turn));
        assign_foundations(&mut self.local);
    }

    /// The PC's session description (complete once gathering is).
    pub fn local_description(&self) -> SessionDescription {
        let candidates =
            self.local.iter().filter(|c| !self.relay_only || c.kind == CandidateType::Relayed).cloned().collect();
        SessionDescription::local(self.credentials.ufrag.clone(), self.credentials.password.clone(), candidates)
    }

    pub fn gathering_complete(&self) -> bool {
        self.gathering_done
    }

    pub fn poll_event(&mut self) -> Option<IceEvent> {
        self.events.pop_front()
    }

    pub fn poll_transmit(&mut self) -> Option<Transmit> {
        self.out.pop_front()
    }
}

#[cfg(test)]
mod tests;

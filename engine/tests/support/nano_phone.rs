//! A scripted phone for the Nano tests: the server side of the Nano path on 127.0.0.1 — an `IpSrtp`
//! listener or a controlled ICE responder, SRTP, the server halves of the MTU probe, keepalive and
//! rate-control handshakes, a rate controller built from the engine's receiver and sender, and a
//! mux that creates channels the test then drives through the engine's own channel types.

use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use phonelink_engine::nano::channel::{Announcement, ChannelError, ChannelLink, RawChannel, RawInbound};
use phonelink_engine::nano::contract::{MasterKey, to_hex};
use phonelink_engine::nano::ice::candidate::{Candidate, CandidateType, SessionDescription};
use phonelink_engine::nano::ice::stun::{self, Class, Message, attr, method};
use phonelink_engine::nano::srtp::{RtpPacket, SrtpSession};
use phonelink_engine::nano::transport::SendMode;
use phonelink_engine::nano::transport::mux::{Control, SequencerRx, SequencerTx};
use phonelink_engine::nano::transport::rc_header::RcHeader;
use phonelink_engine::nano::transport::receiver::Receiver;
use phonelink_engine::nano::transport::sender::{Outgoing, Sender};
use serde_json::{Value, json};
use tokio::net::UdpSocket;
use tokio::sync::mpsc;

const PT_PROBE: u8 = 0x66;
const PT_KEEPALIVE: u8 = 0x65;
const PT_RC: u8 = 0x64;
const PT_CONTROL: u8 = 0x61;
const MAX_PAYLOAD: usize = 1200;
pub const PHONE_ICE_PASSWORD: &str = "phone-password-0123456789";

enum Cmd {
    Create {
        id: u16,
        name: String,
        inbound: mpsc::UnboundedSender<RawInbound>,
    },
    Open(u16),
    Send {
        id: u16,
        mode: SendMode,
        payload_type: u8,
        body: Vec<u8>,
    },
    Close(u16),
    /// Stop answering: the PC's keepalive times out.
    GoSilent,
    /// Send the keepalive close packet.
    CloseConnection,
}

struct PhoneLink(mpsc::UnboundedSender<Cmd>);

impl ChannelLink for PhoneLink {
    fn open(&self, id: u16) -> Result<(), ChannelError> {
        self.0.send(Cmd::Open(id)).map_err(|_| ChannelError::ConnectionGone)
    }
    fn send(&self, id: u16, mode: SendMode, payload_type: u8, body: Vec<u8>) -> Result<(), ChannelError> {
        self.0.send(Cmd::Send { id, mode, payload_type, body }).map_err(|_| ChannelError::ConnectionGone)
    }
    fn close(&self, id: u16) {
        let _ = self.0.send(Cmd::Close(id));
    }
    fn max_payload(&self) -> usize {
        MAX_PAYLOAD - 60
    }
    fn send_queue_fill(&self) -> f64 {
        0.0
    }
    fn bandwidth_bps(&self) -> f64 {
        100_000_000.0
    }
}

/// How the phone is reached.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    IpSrtp,
    IceSrtp,
}

/// The scripted phone.
pub struct FakePhone {
    pub address: SocketAddr,
    pub key: MasterKey,
    cmd: mpsc::UnboundedSender<Cmd>,
    next_id: Mutex<u16>,
    offer_key: Arc<Mutex<Option<MasterKey>>>,
}

impl FakePhone {
    pub async fn start(mode: Mode) -> Arc<Self> {
        let socket = Arc::new(UdpSocket::bind("127.0.0.1:0").await.unwrap());
        let address = socket.local_addr().unwrap();
        let key = MasterKey::random();
        let (cmd, rx) = mpsc::unbounded_channel();
        let offer_key = Arc::new(Mutex::new(None));

        let server = Server {
            socket,
            mode,
            key: key.clone(),
            offer_key: offer_key.clone(),
            srtp: None,
            pc: None,
            flow: None,
            synced: false,
            channels: HashMap::new(),
            queue: Vec::new(),
            silent: false,
            last_keepalive: Instant::now(),
        };

        tokio::spawn(server.run(rx));
        Arc::new(Self { address, key, cmd, next_id: Mutex::new(1), offer_key })
    }

    /// The `IpSrtp` answer.
    pub fn ip_answer(&self) -> Value {
        self.ip_answer_with_warning(5000)
    }

    /// The `IpSrtp` answer with `keepalive.warningtimeout` = `warning_ms`.
    pub fn ip_answer_with_warning(&self, warning_ms: u64) -> Value {
        json!({
            "security": {"key": to_hex(&self.key.key), "salt": to_hex(&self.key.salt)},
            "ip": {"count": "2", "0": {"iname": "rmnet0", "addr": "10.255.255.1"}, "1": {"iname": "wlan0", "addr": "127.0.0.1"}},
            "port": self.address.port().to_string(),
            "keepalive": {"warningtimeout": warning_ms.to_string()},
            "tbst": {"wifiIp": "127.0.0.1"},
        })
    }

    /// The `IceSrtp` answer to `offer`: the phone's one host candidate; the PC's key is taken from the
    /// offer.
    pub fn ice_answer(&self, offer: &serde_json::Map<String, Value>) -> Value {
        let key = hex::decode(offer["security"]["key"].as_str().unwrap()).unwrap();
        let salt = hex::decode(offer["security"]["salt"].as_str().unwrap()).unwrap();
        *self.offer_key.lock().unwrap() =
            Some(MasterKey { key: key.try_into().unwrap(), salt: salt.try_into().unwrap() });

        let d = SessionDescription::local(
            "phoneufrag".into(),
            PHONE_ICE_PASSWORD.into(),
            vec![Candidate::local(CandidateType::Host, self.address, self.address, None, 65_535)],
        );

        json!({"ice": {"candidates": d.to_json_string()}, "keepalive": {"warningtimeout": "5000"}, "contractVersion": {"versionMax": 6}})
    }

    /// Creates a channel announced as `name` and returns the phone's end of it.
    pub fn create_channel(&self, name: &str) -> RawChannel {
        let id = {
            let mut n = self.next_id.lock().unwrap();
            let id = *n;
            *n += 1;
            id
        };

        let (tx, rx) = mpsc::unbounded_channel();
        self.cmd.send(Cmd::Create { id, name: name.to_string(), inbound: tx }).unwrap();
        RawChannel::new(id, Announcement::parse(name).unwrap(), Arc::new(PhoneLink(self.cmd.clone())), rx)
    }

    pub fn go_silent(&self) {
        self.cmd.send(Cmd::GoSilent).unwrap();
    }

    pub fn close_connection(&self) {
        self.cmd.send(Cmd::CloseConnection).unwrap();
    }
}

struct PhoneChannel {
    tx: SequencerTx,
    rx: SequencerRx,
    inbound: mpsc::UnboundedSender<RawInbound>,
    local_open: bool,
    remote_open: bool,
}

struct Server {
    socket: Arc<UdpSocket>,
    mode: Mode,
    key: MasterKey,
    offer_key: Arc<Mutex<Option<MasterKey>>>,
    srtp: Option<SrtpSession>,
    pc: Option<SocketAddr>,
    flow: Option<(Receiver, Sender)>,
    synced: bool,
    channels: HashMap<u16, PhoneChannel>,
    /// Data waiting for the rate-control handshake.
    queue: Vec<Outgoing>,
    silent: bool,
    last_keepalive: Instant,
}

fn packet(payload_type: u8, payload: Vec<u8>) -> RtpPacket {
    RtpPacket { payload_type, marker: false, timestamp: 0, ssrc: 0, sequence: 0, payload }
}

impl Server {
    async fn run(mut self, mut cmd: mpsc::UnboundedReceiver<Cmd>) {
        let mut buf = vec![0u8; 4096];
        let mut tick = tokio::time::interval(Duration::from_millis(50));

        loop {
            tokio::select! {
                r = self.socket.recv_from(&mut buf) => {
                    let Ok((n, from)) = r else { return };
                    let data = buf[..n].to_vec();
                    self.on_datagram(from, &data).await;
                }
                c = cmd.recv() => match c {
                    Some(c) => self.on_cmd(c).await,
                    None => return,
                },
                _ = tick.tick() => self.on_tick().await,
            }
        }
    }

    async fn send(&mut self, p: RtpPacket) {
        if self.silent {
            return;
        }
        let (Some(srtp), Some(pc)) = (self.srtp.as_mut(), self.pc) else {
            return;
        };

        let bytes = srtp.protect(&p);
        let _ = self.socket.send_to(&bytes, pc).await;
    }

    async fn on_datagram(&mut self, from: SocketAddr, data: &[u8]) {
        if self.silent {
            return;
        }

        if stun::is_stun(data) {
            let d = Message::decode(data).unwrap();
            if d.message.class == Class::Request && d.message.method == method::BINDING {
                d.verify_integrity(PHONE_ICE_PASSWORD.as_bytes())
                    .expect("the PC keys checks with the phone's password");
                assert!(!d.message.has(attr::FINGERPRINT), "no FINGERPRINT on ICE paths");

                let tx = d.message.transaction;
                let reply = Message::new(Class::Success, method::BINDING, tx)
                    .with(attr::XOR_MAPPED_ADDRESS, stun::encode_xor_address(from, &tx))
                    .encode(Some(PHONE_ICE_PASSWORD.as_bytes()), false);
                let _ = self.socket.send_to(&reply, from).await;

                if d.message.has(attr::USE_CANDIDATE) {
                    self.pc = Some(from);
                }
            }
            return;
        }

        if self.srtp.is_none() {
            let key = match self.mode {
                Mode::IpSrtp => self.key.clone(),
                Mode::IceSrtp => self.offer_key.lock().unwrap().clone().expect("the answer was built"),
            };
            self.srtp = Some(SrtpSession::new(&key));
        }

        if self.pc.is_none() {
            self.pc = Some(from);
        }

        let Ok(p) = self.srtp.as_mut().unwrap().unprotect(data) else {
            return;
        };

        let now = Instant::now();
        match p.payload_type {
            PT_PROBE => self.on_probe(&p).await,
            PT_KEEPALIVE => {}
            PT_RC => self.on_rc(&p).await,
            _ => self.on_rate_controlled(now, p, data.len()).await,
        }
    }

    async fn on_probe(&mut self, p: &RtpPacket) {
        if p.payload[..2] == [1, 0] && p.payload[2] == 1 && !self.synced {
            self.synced = true;

            let mut syn = vec![0u8; 1000];
            syn[0] = 1;
            syn[2] = 1;
            self.send(packet(PT_PROBE, syn)).await;

            let mut synack = vec![2, 0];
            synack.extend_from_slice(&(p.payload.len() as u16 + 80).to_le_bytes());
            self.send(packet(PT_PROBE, synack)).await;

            // Keepalive handshake: parameters without MTU parameters (normal mode).
            let mut ka = 0u32.to_le_bytes().to_vec();
            ka.extend_from_slice(&1u32.to_le_bytes());
            ka.extend_from_slice(&400u64.to_le_bytes());
            ka.extend_from_slice(&3000u64.to_le_bytes());
            self.send(packet(PT_KEEPALIVE, ka)).await;
        }
    }

    async fn on_rc(&mut self, p: &RtpPacket) {
        match u16::from_le_bytes([p.payload[0], p.payload[1]]) {
            0 => {
                let retry = &p.payload[15..17];
                let mut syn = 0u16.to_le_bytes().to_vec();
                syn.extend_from_slice(&3u16.to_le_bytes());
                syn.push(14);
                syn.extend_from_slice(&100u64.to_le_bytes());
                syn.extend_from_slice(&5u16.to_le_bytes());
                syn.extend_from_slice(retry);
                self.send(packet(PT_RC, syn)).await;
            }
            1 => {
                if self.flow.is_none() {
                    let now = Instant::now();
                    self.flow = Some((Receiver::new(now, 100), Sender::new(now)));
                }

                self.send(packet(PT_RC, 2u16.to_le_bytes().to_vec())).await;
                for o in std::mem::take(&mut self.queue) {
                    self.send_data(o).await;
                }
            }
            _ => {}
        }
    }

    async fn send_data(&mut self, o: Outgoing) {
        let Some((receiver, sender)) = self.flow.as_mut() else {
            self.queue.push(o);
            return;
        };
        let now = Instant::now();
        if let Some(p) = sender.send(now, Some(o), receiver) {
            self.send(p).await;
        }
    }

    async fn on_rate_controlled(&mut self, now: Instant, p: RtpPacket, raw: usize) {
        let Some((receiver, sender)) = self.flow.as_mut() else {
            return;
        };

        let (h, len) = RcHeader::decode(&p.payload).unwrap();
        receiver.on_packet(now, &h, Some(raw - (p.payload.len() - len)));
        let _ = sender.on_header(now, &h);
        let body = p.payload[len..].to_vec();

        if receiver.ack_due(now) {
            if let Some(ack) = sender.ack_only(now, receiver) {
                self.send(ack).await;
            }
        }

        if body.is_empty() {
            return;
        }

        let id = p.ssrc as u16;
        if p.payload_type == PT_CONTROL {
            match Control::decode(&body).unwrap() {
                Control::Open { .. } => {
                    if let Some(c) = self.channels.get_mut(&id) {
                        c.remote_open = true;
                        if c.local_open {
                            let _ = c.inbound.send(RawInbound::Opened);
                        }
                    }
                }
                Control::Close => {
                    if let Some(c) = self.channels.remove(&id) {
                        let _ = c.inbound.send(RawInbound::Closed);
                    }
                }
                Control::Create { .. } => panic!("the PC never creates channels"),
            }
            return;
        }

        let Some(c) = self.channels.get_mut(&id) else {
            return;
        };
        for delivered in c.rx.on_packet(p.sequence, &body).unwrap() {
            let _ = c.inbound.send(RawInbound::Data { payload_type: p.payload_type, payload: delivered });
        }
    }

    fn control(id: u16, c: &Control) -> Outgoing {
        Outgoing { payload_type: PT_CONTROL, ssrc: u32::from(id), sequence: 0, body: c.encode(), reliable: true }
    }

    async fn on_cmd(&mut self, c: Cmd) {
        match c {
            Cmd::Create { id, name, inbound } => {
                self.channels.insert(
                    id,
                    PhoneChannel {
                        tx: SequencerTx::default(),
                        rx: SequencerRx::default(),
                        inbound,
                        local_open: false,
                        remote_open: false,
                    },
                );

                let create =
                    Control::Create { name: name.into_bytes(), fec_type: 0, init_info: Vec::new(), priority: 1.0 };
                self.send_data(Self::control(id, &create)).await;
            }
            Cmd::Open(id) => {
                let Some(ch) = self.channels.get_mut(&id) else { return };
                ch.local_open = true;
                if ch.remote_open {
                    let _ = ch.inbound.send(RawInbound::Opened);
                }
                self.send_data(Self::control(id, &Control::Open { init_info: Vec::new(), priority: 1.0 })).await;
            }
            Cmd::Send { id, mode, payload_type, body } => {
                let Some(ch) = self.channels.get_mut(&id) else { return };
                let (sn, header) = ch.tx.header(mode);
                let mut b = header.to_vec();
                b.extend_from_slice(&body);

                let o = Outgoing {
                    payload_type,
                    ssrc: u32::from(id),
                    sequence: sn as u16,
                    body: b,
                    reliable: mode != SendMode::Unreliable,
                };
                self.send_data(o).await;
            }
            Cmd::Close(id) => {
                self.channels.remove(&id);
                self.send_data(Self::control(id, &Control::Close)).await;
            }
            Cmd::GoSilent => self.silent = true,
            Cmd::CloseConnection => {
                let mut p = 0xFFFF_FFFFu32.to_le_bytes().to_vec();
                p.extend_from_slice(&4u32.to_le_bytes());
                self.send(packet(PT_KEEPALIVE, p)).await;
            }
        }
    }

    async fn on_tick(&mut self) {
        let now = Instant::now();
        if self.synced && now.duration_since(self.last_keepalive) >= Duration::from_millis(400) {
            self.last_keepalive = now;
            let mut p = 0u32.to_le_bytes().to_vec();
            p.extend_from_slice(&0u32.to_le_bytes());
            self.send(packet(PT_KEEPALIVE, p)).await;
        }

        let Some((receiver, sender)) = self.flow.as_mut() else { return };
        let _ = sender.on_timeout(now);

        let mut out = Vec::new();
        while sender.has_retransmission() {
            match sender.send(now, None, receiver) {
                Some(p) => out.push(p),
                None => break,
            }
        }

        if receiver.ack_due(now) && receiver.has_feedback() {
            out.extend(sender.ack_only(now, receiver));
        }

        for p in out {
            self.send(p).await;
        }
    }
}

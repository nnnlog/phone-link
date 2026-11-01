//! The PC's sending rate.
//!
//! It covers the URCPV2 rate controller (client role) with its token bucket, loss- and delay-based
//! targets, path-capacity estimate from packet trains, and the reference-timestamp controller that
//! turns the phone's receive timestamps into one-way delays.
//!
//! Rates are bytes/s,
//! times seconds since the connection started.

use super::stats::{CountWindow, Staggered};

/// The packet size the controller reasons with.
pub const MTU: f64 = 1368.0;
/// The lowest rate: 256 kbit/s.
pub const FLOOR: f64 = 32_000.0;
/// The client's upper clamp factor.
const BW_FACTOR: f64 = 0.9;
/// The highest rate: unlimited.
const HARD_MAX: f64 = f64::INFINITY;
/// The PC's advertised delayed-ACK timeout.
pub const DELAYED_ACK_TIMEOUT_MS: u16 = 20;

/// One packet the rate controller learnt the fate of.
#[derive(Debug, Clone, PartialEq)]
pub struct PacketEvent {
    pub sn: u64,
    pub send_time: f64,
    pub size: usize,
    pub in_flight_at_send: usize,
    pub fate: Fate,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Fate {
    Lost,
    /// `peer_receive`: the phone's receive time (its clock, seconds); `rtt`: the round-trip sample.
    Acked {
        peer_receive: Option<f64>,
        rtt: Option<f64>,
        ack_time: f64,
    },
}

/// The reference-timestamp controller: the phone clock's offset and drift from the samples of
/// smallest round trip, one anchor per 10 s, the last 8 kept.
#[derive(Debug, Clone, Default)]
pub struct RefTimestamps {
    /// `(local mid time, offset = peer time − local mid time, rtt)`.
    anchors: Vec<(f64, f64, f64)>,
}

impl RefTimestamps {
    const PERIOD: f64 = 10.0;
    const KEEP: usize = 8;

    pub fn add(&mut self, send_time: f64, rtt: f64, peer_receive: f64) {
        let mid = send_time + rtt / 2.0;
        let sample = (mid, peer_receive - mid, rtt);

        match self.anchors.last_mut() {
            Some(last) if mid - last.0 < Self::PERIOD => {
                if rtt < last.2 {
                    *last = (last.0, sample.1, rtt);
                }
            }
            _ => {
                self.anchors.push(sample);
                if self.anchors.len() > Self::KEEP {
                    self.anchors.remove(0);
                }
            }
        }
    }

    fn offset(&self, t: f64) -> Option<f64> {
        match self.anchors.as_slice() {
            [] => None,
            [a] => Some(a.1),
            [.., a, b] => {
                let span = b.0 - a.0;
                if span <= 0.0 { Some(b.1) } else { Some(a.1 + (b.1 - a.1) * (t - a.0) / span) }
            }
        }
    }

    /// The one-way delay of a packet sent at `send_time` and received at `peer_receive`.
    pub fn one_way_delay(&self, send_time: f64, peer_receive: f64) -> Option<f64> {
        self.offset(send_time).map(|o| peer_receive - o - send_time)
    }

    /// Half the smallest round trip of the current anchor.
    pub fn min_delay(&self) -> Option<f64> {
        self.anchors.last().map(|a| a.2 / 2.0)
    }
}

/// History of the last four loss intervals and the TFRC loss-based rate.
#[derive(Debug, Clone)]
struct LossEstimator {
    /// Most recent first.
    history: Vec<f64>,
    aside: Option<f64>,
    current: f64,
    x: f64,
    r0: f64,
}

impl LossEstimator {
    const WEIGHTS: [f64; 4] = [1.0, 1.0, 0.6, 0.2];

    fn new() -> Self {
        Self { history: Vec::new(), aside: None, current: 0.0, x: 0.0, r0: 10_000.0 }
    }

    fn reset(&mut self) {
        *self = Self::new();
    }

    fn record(&mut self, n: f64) {
        self.history.insert(0, n);
        self.history.truncate(4);
    }

    fn mean_interval(&self) -> Option<f64> {
        if self.history.is_empty() {
            return None;
        }

        let total: f64 = Self::WEIGHTS.iter().sum();
        let mut sum = 0.0;
        let mut missing = 0.0;
        for (i, w) in Self::WEIGHTS.iter().enumerate() {
            match self.history.get(i) {
                Some(n) => sum += w / total * n,
                None => missing += w / total,
            }
        }

        Some(sum / (1.0 - missing))
    }

    /// Closes the current interval on a new loss event and updates `X`.
    fn close_interval(&mut self, rtt: f64) {
        let n = self.current;
        match self.history.first().copied() {
            Some(last) if 3.0 * n < last => self.history.clear(),
            Some(last) if n > 1.8 * last => match self.aside.take() {
                None => self.aside = Some(n),
                Some(kept) => {
                    self.record(kept);
                    self.record(n);
                }
            },
            _ => {
                if let Some(kept) = self.aside.take() {
                    self.record(kept);
                }
                self.record(n);
            }
        }

        self.current = 0.0;
        self.update(rtt);
    }

    fn update(&mut self, rtt: f64) {
        self.r0 = rtt;
        let Some(mean) = self.mean_interval() else {
            return;
        };

        let p = if mean > 0.0 { 1.0 / mean } else { 1.0 };
        self.x = if p <= 1e-5 {
            0.0
        } else {
            let s = MTU - 136.0;
            s / (rtt * (2.0 * p / 3.0).sqrt() + 12.0 * rtt * (3.0 * p / 8.0).sqrt() * p * (1.0 + 32.0 * p * p))
        };
    }

    fn target(&self, rtt: f64) -> f64 {
        self.x * (self.r0 / rtt).min(1.0)
    }
}

/// The path-capacity estimate from packet trains.
#[derive(Debug, Clone)]
struct PathCapacity {
    /// Previous packet: `(sn, send ms, receive ms, size)`.
    prev: Option<(u64, f64, f64, usize)>,
    weak_size: Staggered,
    weak_gap: Staggered,
    weak_samples: u64,
    total_bytes: f64,
    total_time: f64,
    /// The current train: `(span ms, size)` per packet after the first.
    train: Vec<(f64, usize)>,
    train_bytes: CountWindow,
    train_span: CountWindow,
    rates: Vec<f64>,
    medians: CountWindow,
    deviations: CountWindow,
    state: u8,
    best: f64,
    c: f64,
    start: f64,
}

impl PathCapacity {
    fn new(start: f64) -> Self {
        Self {
            prev: None,
            weak_size: Staggered::time_window(10.0, 2.0),
            weak_gap: Staggered::time_window(10.0, 2.0),
            weak_samples: 0,
            total_bytes: 0.0,
            total_time: 0.0,
            train: Vec::new(),
            train_bytes: CountWindow::new(4, 5),
            train_span: CountWindow::new(4, 5),
            rates: Vec::new(),
            medians: CountWindow::new(8, 10),
            deviations: CountWindow::new(8, 10),
            state: 0,
            best: 0.6,
            c: 0.0,
            start,
        }
    }

    fn weak(&self) -> (f64, f64) {
        let gap = self.weak_gap.mean();
        let rate = if gap > 0.0 { self.weak_size.mean() * 1000.0 / gap } else { 0.0 };
        (rate, (1.0 - 20.0 / self.weak_samples.max(1) as f64).max(0.0))
    }

    fn total(&self) -> f64 {
        if self.total_time > 0.0 { self.total_bytes * 1000.0 / self.total_time } else { 0.0 }
    }

    fn strong(&self) -> (f64, f64) {
        if !self.medians.is_ready() && self.medians.count() == 0 {
            return (0.0, 0.0);
        }
        let rate = self.medians.mean();
        if rate <= 0.0 { (0.0, 0.0) } else { (rate, 1.0 - self.deviations.mean() / rate) }
    }

    fn process_train(&mut self) {
        let train = std::mem::take(&mut self.train);
        let max = train.iter().map(|(span, size)| span / *size as f64).fold(0.0, f64::max);
        if max <= 0.0 {
            return;
        }

        let mut burst: Option<(f64, f64)> = None;
        for (span, size) in train {
            let per_byte = span / size as f64;
            if per_byte > 0.6 * max {
                self.finish_burst(&mut burst);
                burst = Some((size as f64, span));
            } else if per_byte < 0.2 * max {
                if let Some((bytes, s)) = burst.as_mut() {
                    *bytes += size as f64;
                    *s += span;
                }
            } else {
                self.finish_burst(&mut burst);
            }
        }

        self.finish_burst(&mut burst);
    }

    /// Adds a finished burst's bytes and span to the train averages and, once they are ready, the
    /// train rate to the ring.
    fn finish_burst(&mut self, burst: &mut Option<(f64, f64)>) {
        let Some((bytes, span)) = burst.take() else {
            return;
        };
        if span <= 0.0 {
            return;
        }

        self.train_bytes.add(bytes);
        self.train_span.add(span);
        if self.train_bytes.is_ready() {
            let rate = self.train_bytes.mean() * 1000.0 / self.train_span.mean();
            self.rates.push(rate);
            if self.rates.len() == 10 {
                self.rate_ring_full();
            }
        }
    }

    fn rate_ring_full(&mut self) {
        let mut sorted = std::mem::take(&mut self.rates);
        sorted.sort_by(f64::total_cmp);
        let median = (sorted[4] + sorted[5]) / 2.0;

        let mut devs: Vec<f64> = sorted.iter().map(|r| (r - median).abs()).collect();
        devs.sort_by(f64::total_cmp);
        devs.pop();

        self.medians.add(median);
        self.deviations.add(devs.iter().sum::<f64>() / devs.len() as f64);
    }

    fn reset_trains(&mut self) {
        self.train.clear();
        self.prev = None;
    }

    /// Feeds one acknowledged packet (times in ms).
    fn feed(&mut self, sn: u64, send_ms: f64, recv_ms: f64, size: usize) {
        if size <= 599 || self.prev.is_some_and(|p| recv_ms < p.2) {
            self.process_train();
            self.reset_trains();
            return;
        }

        match self.prev {
            Some((psn, psend, precv, _)) if sn == psn + 1 && send_ms - psend <= 1.0 => {
                let gap = recv_ms - precv;
                self.weak_samples += 1;
                self.total_bytes += size as f64;
                self.total_time += gap;
                self.weak_size.add(recv_ms / 1000.0, size as f64);
                self.weak_gap.add(recv_ms / 1000.0, gap);
                self.train.push((gap, size));
            }
            _ => self.process_train(),
        }

        self.prev = Some((sn, send_ms, recv_ms, size));
    }

    /// The combination step: the capacity `c` and whether it is a confident estimate.
    fn estimate(&mut self, now: f64) -> (f64, bool) {
        let (strong, strong_conf) = self.strong();
        let (weak, weak_conf) = self.weak();
        let mut confident = false;

        match self.state {
            3 => {
                if strong_conf > self.best {
                    self.best = strong_conf;
                    self.c = self.c.max(strong);
                }
            }
            0 => {
                if strong > 0.0 && strong_conf > 0.7 {
                    self.state = 3;
                    self.best = strong_conf;
                    self.c = self.c.max(strong);
                } else if weak_conf > 0.8 {
                    self.state = 2;
                    self.c = self.c.max(weak);
                }
            }
            _ => {
                if strong > 0.0 && strong_conf > self.best {
                    self.state = 3;
                    self.best = strong_conf;
                    self.c = strong;
                    confident = true;
                } else if weak_conf > 0.9 {
                    self.state = 1;
                    self.c = self.c.max(self.total());
                    confident = true;
                }
            }
        }

        if self.c == 0.0 && now - self.start > 2.0 {
            self.c = if weak_conf < 0.5 { self.c.max(self.total()) } else { self.c.max(weak) };
            self.state = 2;
        }

        (self.c, confident)
    }
}

/// The URCPV2 sender.
#[derive(Debug, Clone)]
pub struct Urcpv2 {
    hard_max: f64,
    max_rate: f64,
    rate: f64,
    /// 1 = delay-based, 2 = loss-based.
    mode: u8,
    started: bool,
    allowed: f64,
    sent: f64,
    t_last: Option<f64>,
    ran_out: Option<f64>,
    max_want: f64,
    loss_period_end: f64,
    warmup_end: f64,
    min_delay: f64,
    smoothed_delay: f64,
    op_point: f64,
    max_delayed_acks: u8,
    loss: LossEstimator,
    consecutive_losses: u32,
    rtt_p10: Staggered,
    delay_p10: Staggered,
    op_rate: CountWindow,
    rop: Staggered,
    dbar: Staggered,
    loss_w: Staggered,
    acked_tw: Staggered,
    send_rate: Staggered,
    capacity: PathCapacity,
    pub refts: RefTimestamps,
}

impl Urcpv2 {
    pub fn new(now: f64) -> Self {
        // The initial path capacity: 16 × the floor taken as bits, in bytes (64 000).
        let max_rate = HARD_MAX.min(FLOOR * 16.0 / 8.0);
        let mut c = Self {
            hard_max: HARD_MAX,
            max_rate,
            rate: max_rate,
            mode: 2,
            started: false,
            allowed: MTU,
            sent: 0.0,
            t_last: None,
            ran_out: None,
            max_want: 0.0,
            loss_period_end: f64::INFINITY,
            warmup_end: f64::INFINITY,
            min_delay: 1.0,
            smoothed_delay: 0.0,
            op_point: 0.02,
            max_delayed_acks: 1,
            loss: LossEstimator::new(),
            consecutive_losses: 0,
            rtt_p10: Staggered::percentile(0.3),
            delay_p10: Staggered::percentile(0.3),
            op_rate: CountWindow::new(32, 40),
            rop: Staggered::time_window(10.0, 2.0),
            dbar: Staggered::time_window(5.0, 1.0),
            loss_w: Staggered::time_window(10.0, 2.0),
            acked_tw: Staggered::time_window(10.0, 2.0),
            send_rate: Staggered::byte_rate(2.0),
            capacity: PathCapacity::new(now),
            refts: RefTimestamps::default(),
        };

        c.recompute_op_point();
        c
    }

    /// The first path-capacity estimate arrived.
    pub fn is_started(&self) -> bool {
        self.started
    }

    /// The pacing rate (bytes/s).
    pub fn rate(&self) -> f64 {
        self.rate
    }

    #[cfg(feature = "test-support")]
    /// The path-capacity estimate (bytes/s).
    pub fn max_rate(&self) -> f64 {
        self.max_rate
    }

    /// `maxDelayedAcks` of the PC's `DELAYACKINFO`.
    pub fn max_delayed_acks(&self) -> u8 {
        self.max_delayed_acks
    }

    fn recompute_op_point(&mut self) {
        let d = MTU / self.max_rate;
        if 3.0 * d >= 0.01 {
            self.op_point = 3.0 * d;
            self.max_delayed_acks = 1;
        } else {
            self.op_point = 0.01;
            self.max_delayed_acks = ((0.01 / d).floor() as u64).clamp(6, 255) as u8;
        }
    }

    /// Bytes the sender may send now for a send opportunity of `want` bytes.
    pub fn allowance(&mut self, now: f64, want: usize) -> f64 {
        self.max_want = self.max_want.max(want as f64);

        if !self.started {
            self.allowed = self.sent + 100.0 * MTU - 13_600.0;
        } else {
            let last = self.t_last.unwrap_or(now);
            let t = match self.ran_out {
                Some(r) if r > last => r,
                _ => now,
            };
            self.allowed =
                (self.allowed.max(self.sent) + self.rate * (t - last).max(0.0)).min(self.sent + 1.1 * self.max_want);
        }

        self.t_last = Some(now);
        self.allowed - self.sent
    }

    pub fn on_sent(&mut self, size: usize) {
        self.sent += size as f64;
    }

    /// The sender has nothing left to send.
    pub fn on_idle(&mut self, now: f64) {
        self.ran_out = Some(now);
    }

    /// The controller's round trip (`max(0.05, P10 of the samples)`), seconds.
    pub fn rtt(&self) -> f64 {
        self.rtt_p10.percentile_value().max(0.05)
    }

    fn set_max_rate(&mut self, now: f64, c: f64, confident: bool) {
        if !self.started {
            self.max_rate = c.max(self.max_rate).min(self.hard_max);
            self.started = true;
            self.allowed = self.sent + MTU;
            self.rate = self.max_rate / 2.0;
            self.warmup_end = now + 2.0;
            self.loss.reset();
            self.loss.x = self.max_rate / 2.0;
            self.loss.r0 = self.rtt();
        } else {
            self.max_rate = self.hard_max.min(if confident { c } else { c.max(self.max_rate) });
        }

        self.recompute_op_point();
    }

    /// One ACK batch.
    pub fn on_batch(&mut self, now: f64, events: &[PacketEvent]) {
        for e in events {
            if let Fate::Acked { peer_receive: Some(tr), rtt: Some(rtt), .. } = e.fate {
                self.refts.add(e.send_time, rtt, tr);
                self.capacity.feed(e.sn, e.send_time * 1000.0, tr * 1000.0, e.size);
            }
        }

        let (c, confident) = self.capacity.estimate(now);
        if c > 0.0 {
            self.set_max_rate(now, c, confident);
        }

        if let Some(min) = self.refts.min_delay() {
            self.min_delay = min;
            self.recompute_op_point();
        }

        for e in events {
            self.packet_event(now, e);
        }

        if self.started {
            self.update_rate(events);
        }
    }

    fn packet_event(&mut self, now: f64, e: &PacketEvent) {
        match e.fate {
            Fate::Acked { peer_receive, rtt, .. } => {
                if let Some(rtt) = rtt {
                    self.rtt_p10.add(now, rtt);
                }

                if self.started {
                    if let (Some(tr), Some(_)) = (peer_receive, rtt) {
                        if let Some(owd) = self.refts.one_way_delay(e.send_time, tr) {
                            self.delay_sample(tr, owd, e);
                        }
                    }
                }

                if now > self.warmup_end {
                    self.send_rate.add(now, e.size as f64);
                    self.acked_tw.add(now, 1.0);
                    self.loss_w.add(now, 0.0);
                    self.consecutive_losses = 0;
                }
            }
            Fate::Lost => {
                if now > self.warmup_end {
                    self.consecutive_losses += 1;
                    let k = f64::from(self.consecutive_losses);
                    self.loss_w.add(now, 1.0 / (k * k));
                }
                self.on_loss(e);
            }
        }
    }

    fn delay_sample(&mut self, tr: f64, owd: f64, e: &PacketEvent) {
        let q = (owd - self.min_delay).max(0.0);
        let load = (e.size + e.in_flight_at_send) as f64;

        if q > 0.2 * self.min_delay {
            self.op_rate.add_weighted(load / q, (load / MTU).min(1.0));
        }
        if e.in_flight_at_send >= 501 || (self.op_rate.is_ready() && self.op_rate.count() > 10) {
            self.rop.add(tr, self.op_rate.weighted_mean());
        }

        let rop = self.rop.mean();
        let sample = if q > 0.0 && rop > 0.0 { q.min((e.in_flight_at_send as f64 + MTU) / rop) } else { q };
        self.delay_p10.add(tr, sample);
        let v = if self.delay_p10.is_ready() { self.delay_p10.percentile_value() } else { sample };
        self.smoothed_delay = if v >= self.smoothed_delay { v } else { (v + self.smoothed_delay) / 2.0 };

        self.loss.current += 1.0;
    }

    fn on_loss(&mut self, e: &PacketEvent) {
        if e.send_time <= self.loss_period_end {
            self.loss.current = 0.0;
            return;
        }

        let rtt = self.rtt();
        if self.mode == 1 {
            let m = self.max_rate;
            let r = self.send_rate.rate();
            let x = if r == 0.0 { m / 2.0 } else { m.min(1.2 * r * m / (r + m)) };
            self.loss.reset();
            self.loss.x = x;
            self.loss.r0 = rtt;
        } else {
            self.loss.close_interval(rtt);
        }

        self.loss_period_end = e.send_time + rtt * (4.0f64).max(0.3 * rtt * self.rate / (2.0 * MTU));
    }

    fn delay_target(&self) -> f64 {
        let d = if self.smoothed_delay <= 0.0 {
            self.max_rate
        } else {
            1.2 * (self.op_point / self.smoothed_delay) * (self.max_rate * FLOOR.max(self.rop.mean())).sqrt()
        };
        d.min(1.1 * self.max_rate)
    }

    fn update_rate(&mut self, events: &[PacketEvent]) {
        let rtt = self.rtt();
        let lo = FLOOR;
        let hi = BW_FACTOR * self.max_rate;
        let clamp = |x: f64| x.min(hi).max(lo);
        let d = self.delay_target();

        if let Some(last) = events.last() {
            self.dbar.add(last.send_time, d);
        }

        let l = self.loss.target(rtt);
        let acked = self.acked_tw.sum();
        let loss_w = self.loss_w.sum();
        let drop = if acked > 20.0 { (loss_w / (acked + loss_w)).min(0.7) } else { 0.0 };
        let w = self.loss.mean_interval().map_or(0.0, |m| if m > 0.0 { 1.0 / m } else { 0.0 });

        let mut delay_update = false;
        if self.mode == 2 {
            let reference = if self.dbar.is_ready() { self.dbar.mean() } else { d };
            if reference < l {
                self.rate = (1.0 - w) * self.rate + w * clamp(l);
            } else {
                self.mode = 1;
                self.loss.reset();
                if let Some(last) = events.last() {
                    self.loss_period_end = last.send_time;
                }
                delay_update = true;
            }
        } else if d < l {
            self.mode = 2;
            self.dbar.clear();
            self.rate = (1.0 - w) * self.rate + w * clamp(l);
        } else {
            delay_update = true;
        }

        if delay_update {
            self.rate = clamp(d);
            if drop > 0.0 {
                self.rate *= (1.0 - drop) * (1.0 - drop);
            }
        }

        self.rate = self.rate.min(self.hard_max);
    }
}

#[cfg(test)]
mod tests;

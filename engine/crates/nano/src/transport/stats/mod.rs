//! The windowed statistics of the URCPV2 sender: staggered accumulators over time or over sample
//! counts, a low-percentile window and a byte-rate meter. Times are seconds.

/// One accumulator.
#[derive(Debug, Clone, Default)]
struct Acc {
    started: bool,
    start: f64,
    last: f64,
    count: u64,
    sum: f64,
    weighted_sum: f64,
    weight: f64,
    max: f64,
    /// The smallest samples, ascending (percentile windows only).
    smallest: Vec<f64>,
}

impl Acc {
    fn begin(&mut self, t: f64) {
        *self = Acc { started: true, start: t, last: t, max: f64::MIN, ..Default::default() };
    }

    fn add(&mut self, t: f64, x: f64, w: f64, keep: usize) {
        self.last = t;
        self.count += 1;
        self.sum += x;
        self.weighted_sum += w * x;
        self.weight += w;
        self.max = self.max.max(x);

        if keep > 0 {
            let at = self.smallest.partition_point(|s| *s <= x);
            if at < keep {
                self.smallest.insert(at, x);
                self.smallest.truncate(keep);
            }
        }
    }
}

/// `S(k, W, D)`: `k` accumulators started `D` apart, each restarted when it covers more than `W`.
#[derive(Debug, Clone)]
pub struct Staggered {
    accs: Vec<Acc>,
    window: f64,
    stagger: f64,
    keep: usize,
    current: usize,
    ready: bool,
}

impl Staggered {
    pub fn new(k: usize, window: f64, stagger: f64) -> Self {
        Self { accs: vec![Acc::default(); k], window, stagger, keep: 0, current: 0, ready: false }
    }

    /// `TW(W, D)`.
    pub fn time_window(window: f64, stagger: f64) -> Self {
        Self::new(5, window, stagger)
    }

    /// `P10(W)`.
    pub fn percentile(window: f64) -> Self {
        Self { keep: 5, ..Self::new(5, window, window / 5.0) }
    }

    /// `BR(W)`.
    pub fn byte_rate(window: f64) -> Self {
        Self::new(50, window, window / 50.0)
    }

    pub fn clear(&mut self) {
        let (k, w, d, keep) = (self.accs.len(), self.window, self.stagger, self.keep);
        *self = Self { keep, ..Self::new(k, w, d) };
    }

    pub fn add(&mut self, t: f64, x: f64) {
        self.add_weighted(t, x, 1.0);
    }

    pub fn add_weighted(&mut self, t: f64, x: f64, w: f64) {
        if !self.accs[0].started {
            self.accs[0].begin(t);
        }

        let start0 = self.accs[0].start;
        let keep = self.keep;
        let mut restarted = None;

        if !self.ready && t - start0 > self.window {
            self.ready = true;
        }

        if self.ready && t - self.accs[self.current].start > self.window {
            self.accs[self.current].begin(t);
            self.accs[self.current].add(t, x, w, keep);
            restarted = Some(self.current);
            self.current = (self.current + 1) % self.accs.len();
        }

        for i in 1..self.accs.len() {
            if !self.accs[i].started && t - start0 > i as f64 * self.stagger {
                self.accs[i].begin(t);
            }
        }

        for (i, acc) in self.accs.iter_mut().enumerate() {
            if acc.started && Some(i) != restarted {
                acc.add(t, x, w, keep);
            }
        }
    }

    pub fn is_ready(&self) -> bool {
        self.ready
    }

    fn cur(&self) -> &Acc {
        &self.accs[self.current]
    }

    pub fn count(&self) -> u64 {
        self.cur().count
    }

    pub fn sum(&self) -> f64 {
        self.cur().sum
    }

    /// `sum / count`, 0 when empty.
    pub fn mean(&self) -> f64 {
        let a = self.cur();
        if a.count == 0 { 0.0 } else { a.sum / a.count as f64 }
    }

    /// The current accumulator's largest sample; `None` while it holds none.
    pub fn max_value(&self) -> Option<f64> {
        let a = self.cur();
        (a.count > 0).then_some(a.max)
    }

    /// The low percentile: the kept element at `min(kept − 1, ⌊0.1 × count⌋)`, 0 when none.
    pub fn percentile_value(&self) -> f64 {
        let a = self.cur();
        if a.smallest.is_empty() {
            return 0.0;
        }
        let at = ((0.1 * a.count as f64) as usize).min(a.smallest.len() - 1);
        a.smallest[at]
    }

    /// Bytes added during the current accumulator ÷ its time span.
    pub fn rate(&self) -> f64 {
        let a = self.cur();
        let span = a.last - a.start;
        if span <= 0.0 { 0.0 } else { a.sum / span }
    }
}

/// `CW(N, M)`: five accumulators rotated every `M − N` samples, holding about the last `N … M`.
#[derive(Debug, Clone)]
pub struct CountWindow {
    accs: Vec<Acc>,
    low: u64,
    high: u64,
    counter: u64,
    current: usize,
    ready: bool,
}

impl CountWindow {
    pub fn new(low: u64, high: u64) -> Self {
        Self { accs: vec![Acc::default(); 5], low, high, counter: low, current: 0, ready: false }
    }

    #[cfg(feature = "test-support")]
    pub fn clear(&mut self) {
        *self = Self::new(self.low, self.high);
    }

    pub fn add(&mut self, x: f64) {
        self.add_weighted(x, 1.0);
    }

    pub fn add_weighted(&mut self, x: f64, w: f64) {
        let before = self.counter;
        self.counter += 1;
        let mut restarted = None;

        if before >= self.high {
            self.counter -= self.high - self.low;
            self.accs[self.current].begin(0.0);
            self.accs[self.current].add(0.0, x, w, 0);
            restarted = Some(self.current);
            self.current = (self.current + 1) % self.accs.len();
            if self.current == 0 {
                self.ready = true;
            }
        }

        for (i, acc) in self.accs.iter_mut().enumerate() {
            if !acc.started {
                acc.begin(0.0);
            }
            if Some(i) != restarted {
                acc.add(0.0, x, w, 0);
            }
        }
    }

    pub fn is_ready(&self) -> bool {
        self.ready
    }

    pub fn count(&self) -> u64 {
        self.accs[self.current].count
    }

    pub fn mean(&self) -> f64 {
        let a = &self.accs[self.current];
        if a.count == 0 { 0.0 } else { a.sum / a.count as f64 }
    }

    pub fn weighted_mean(&self) -> f64 {
        let a = &self.accs[self.current];
        if a.weight == 0.0 { 0.0 } else { a.weighted_sum / a.weight }
    }
}

#[cfg(test)]
mod tests;

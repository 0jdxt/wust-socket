// Stores and calculates average of last N latencies
use std::time::Duration;

pub(crate) struct PingStats<const N: usize> {
    history: [Option<Duration>; N],
    idx: usize,
}

impl<const N: usize> PingStats<N> {
    pub(crate) fn new() -> Self {
        Self {
            history: [None; N],
            idx: 0,
        }
    }

    pub(crate) fn add(&mut self, rtt: Duration) {
        self.history[self.idx] = Some(rtt);
        self.idx = (self.idx + 1) % N;
    }

    pub(crate) fn average(&self) -> Option<Duration> {
        let mut sum = Duration::ZERO;
        let mut count = 0;
        for &x in &self.history {
            if let Some(v) = x {
                sum += v;
                count += 1;
            }
        }
        if count > 0 {
            Some(sum / count)
        } else {
            None
        }
    }
}

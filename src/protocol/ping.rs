use std::time::Instant;

// Stores and calculates average of last N latencies
const N: usize = 5;
pub struct PingStats {
    history: [Option<u16>; N],
    idx: usize,
    last_nonce: u16,
    last_ping: Instant,
}

impl PingStats {
    pub(crate) fn new() -> Self {
        Self {
            history: [None; N],
            idx: 0,
            last_nonce: 0,
            last_ping: Instant::now(),
        }
    }

    pub(crate) fn new_ping(&mut self) -> [u8; 2] {
        self.last_ping = Instant::now();
        self.last_nonce += 1;
        self.last_nonce.to_be_bytes()
    }

    pub(crate) fn on_pong(&mut self, nonce: u16) -> Result<u16, PongError> {
        if nonce == self.last_nonce {
            let latency_ms = self.last_ping.elapsed().as_millis();
            let latency = u16::try_from(latency_ms).map_err(|_| PongError::Late(latency_ms))?;
            self.add(latency);
            Ok(latency)
        } else {
            Err(PongError::Nonce(self.last_nonce))
        }
    }

    pub(crate) fn add(&mut self, rtt: u16) {
        self.history[self.idx] = Some(rtt);
        self.idx = (self.idx + 1) % N;
    }

    pub(crate) fn average(&self) -> Option<u16> {
        let mut sum = 0;
        let mut count = 0;
        for &x in &self.history {
            if let Some(v) = x {
                sum += v;
                count += 1;
            }
        }
        sum.checked_div(count)
    }
}

pub(crate) enum PongError {
    Nonce(u16),
    Late(u128),
}

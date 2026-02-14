use std::time::Instant;

const N: usize = 5;
const NONCE_LEN: usize = 8;

// Stores and calculates average of last N latencies
pub struct PingStats {
    history: [Option<u16>; N],
    idx: usize,
    last_nonce: [u8; NONCE_LEN],
    last_ping: Instant,
}

impl PingStats {
    pub(crate) fn new() -> Self {
        Self {
            history: [None; N],
            idx: 0,
            last_nonce: [0; NONCE_LEN],
            last_ping: Instant::now(),
        }
    }

    pub(crate) fn new_nonce(&mut self) -> [u8; NONCE_LEN] {
        self.last_ping = Instant::now();
        let mut buf = [0; NONCE_LEN];
        rand::fill(&mut buf);
        self.last_nonce = buf;
        buf
    }

    pub(crate) fn on_pong(&mut self, nonce: [u8; NONCE_LEN]) -> Result<u16, PongError> {
        if nonce == self.last_nonce {
            let latency_ms = self.last_ping.elapsed().as_millis();
            let latency = u16::try_from(latency_ms).map_err(|_| PongError::Late(latency_ms))?;
            self.history[self.idx] = Some(latency);
            self.idx = (self.idx + 1) % N;
            Ok(latency)
        } else {
            Err(PongError::Nonce(self.last_nonce))
        }
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
    Nonce([u8; NONCE_LEN]),
    Late(u128),
}

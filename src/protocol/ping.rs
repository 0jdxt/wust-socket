use std::time::Instant;

use tokio::sync::mpsc::{Sender, error::SendError};

use crate::{frames::ControlFrame, role::RolePolicy};

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

    pub(crate) async fn new_ping<R: RolePolicy>(
        &mut self,
        ctrl_tx: &Sender<Vec<u8>>,
    ) -> Result<(), SendError<Vec<u8>>> {
        self.last_ping = Instant::now();
        let mut buf = [0; NONCE_LEN];
        rand::fill(&mut buf);
        self.last_nonce = buf;

        let f = ControlFrame::<R>::ping(&buf).encode();
        ctrl_tx.send(f).await
    }

    pub(crate) fn on_pong(&mut self, nonce: [u8; NONCE_LEN]) -> Result<u16, PongError> {
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
    Nonce([u8; NONCE_LEN]),
    Late(u128),
}

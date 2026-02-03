use std::{
    io::Result,
    net::SocketAddr,
    sync::{atomic::Ordering, mpsc::Receiver, Arc},
    time::Duration,
};

use crate::{
    frames::Opcode,
    inner::ConnInner,
    role::{DecodePolicy, EncodePolicy},
    CloseReason, Event,
};

pub struct WebSocket<R: EncodePolicy + DecodePolicy> {
    pub(crate) inner: Arc<ConnInner<R>>,
    pub(crate) event_rx: Receiver<Event>,
}

/// Best-effort close if user forgets to call [`WebSocket::close`].
impl<R: EncodePolicy + DecodePolicy> Drop for WebSocket<R> {
    fn drop(&mut self) {
        if !self.inner.closing.load(Ordering::Acquire) {
            let _ = self.close();
        }
    }
}

impl<R: EncodePolicy + DecodePolicy> WebSocket<R> {
    pub fn send_text(&self, text: &str) -> Result<()> {
        self.inner.send(text.as_bytes(), Opcode::Text)
    }

    pub fn send_bytes(&self, bytes: &[u8]) -> Result<()> { self.inner.send(bytes, Opcode::Bin) }

    pub fn close(&mut self) -> Result<()> { self.close_reason(CloseReason::Normal, "") }

    pub fn close_payload(&mut self, payload: &[u8]) -> Result<()> {
        if self.inner.closing.swap(true, Ordering::AcqRel) {
            return Ok(());
        }
        self.inner.close_raw(payload)
    }

    pub fn close_reason(&mut self, reason: CloseReason, text: &'static str) -> Result<()> {
        if self.inner.closing.swap(true, Ordering::AcqRel) {
            return Ok(());
        }
        self.inner.close(reason, text)
    }

    pub fn ping(&self) -> Result<()> { self.inner.ping() }

    pub fn addr(&self) -> Result<SocketAddr> { self.inner.addr() }

    /// Average latency in ms form last 5 pings
    #[must_use]
    pub fn latency(&self) -> Option<u16> { self.inner.ping_stats.lock().unwrap().average() }

    pub fn recv(&mut self) -> Option<Event> { self.event_rx.recv().ok() }

    pub fn recv_timeout(&mut self, timeout: Duration) -> Option<Event> {
        self.event_rx.recv_timeout(timeout).ok()
    }
}

use std::{
    io::Result,
    marker::PhantomData,
    sync::{atomic::Ordering, mpsc::Receiver, Arc},
    time::Duration,
};

use crate::{
    frames::Opcode,
    inner::InnerTrait,
    role::{DecodePolicy, EncodePolicy},
    CloseReason, Event,
};

pub struct WebSocket<I, R>
where
    I: InnerTrait<R> + Send + Sync + 'static,
    R: EncodePolicy + DecodePolicy + 'static,
{
    pub(crate) inner: Arc<I>,
    pub(crate) event_rx: Receiver<Event>,
    pub(crate) _p: PhantomData<R>,
}

/// Best-effort close if user forgets to call [`WebSocket::close`].
impl<I: InnerTrait<R> + Send + Sync, R: EncodePolicy + DecodePolicy> Drop for WebSocket<I, R> {
    fn drop(&mut self) {
        if !self.inner.closing().load(Ordering::Acquire) {
            let _ = self.close();
        }
    }
}

impl<I: InnerTrait<R> + Send + Sync, R: EncodePolicy + DecodePolicy> WebSocket<I, R> {
    pub fn send_text(&self, text: &str) -> Result<()> {
        self.inner.send(text.as_bytes(), Opcode::Text)
    }

    pub fn send_bytes(&self, bytes: &[u8]) -> Result<()> { self.inner.send(bytes, Opcode::Bin) }

    pub fn close(&mut self) -> Result<()> { self.close_reason(CloseReason::Normal, "") }

    pub fn close_payload(&mut self, payload: &[u8]) -> Result<()> {
        if self.inner.closing().swap(true, Ordering::AcqRel) {
            return Ok(());
        }
        self.inner.close_raw(payload)
    }

    pub fn close_reason(&mut self, reason: CloseReason, text: &'static str) -> Result<()> {
        if self.inner.closing().swap(true, Ordering::AcqRel) {
            return Ok(());
        }
        self.inner.close(reason, text)
    }

    pub fn ping(&self) -> Result<()> { self.inner.ping() }

    pub fn recv(&mut self) -> Option<Event> { self.event_rx.recv().ok() }

    pub fn recv_timeout(&mut self, timeout: Duration) -> Option<Event> {
        self.event_rx.recv_timeout(timeout).ok()
    }
}

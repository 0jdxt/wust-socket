use std::{
    io::Result,
    sync::{atomic::Ordering, mpsc::Receiver, Arc},
    time::Duration,
};

use crate::{frames::Opcode, inner::InnerTrait, CloseReason, Event};

pub struct WebSocket<I: InnerTrait> {
    pub(crate) inner: Arc<I>,
    pub(crate) event_rx: Receiver<Event>,
}

/// Best-effort close if user forgets to call [`WebSocket::close`].
impl<I: InnerTrait> Drop for WebSocket<I> {
    fn drop(&mut self) {
        if !self.inner.closing().load(Ordering::Acquire) {
            let _ = self.close();
        }
    }
}

impl<I: InnerTrait> WebSocket<I> {
    pub fn send_text(&self, text: &str) -> Result<()> {
        self.inner.send(text.as_bytes(), Opcode::Text)
    }

    pub fn send_bytes(&self, bytes: &[u8]) -> Result<()> { self.inner.send(bytes, Opcode::Bin) }

    pub fn close(&mut self) -> Result<()> {
        if self.inner.closing().swap(true, Ordering::AcqRel) {
            return Ok(());
        }
        self.inner.close(CloseReason::Normal, "")
    }

    pub fn ping(&self) -> Result<()> { self.inner.ping() }

    pub fn recv(&mut self) -> Option<Event> { self.event_rx.recv().ok() }

    pub fn recv_timeout(&mut self, timeout: Duration) -> Option<Event> {
        self.event_rx.recv_timeout(timeout).ok()
    }
}

use std::{
    io::Result,
    sync::{atomic::Ordering, mpsc::Receiver, Arc},
};

use crate::{frames::Opcode, inner::InnerTrait, CloseReason, Event};

pub struct WebSocket<I: InnerTrait> {
    pub(crate) inner: Arc<I>,
    pub(crate) event_rx: Receiver<Event>,
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
}

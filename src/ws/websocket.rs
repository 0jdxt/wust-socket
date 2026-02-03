use std::{
    io::{Read, Result},
    net::SocketAddr,
    sync::{
        atomic::Ordering,
        mpsc::{Receiver, Sender},
        Arc,
    },
    thread,
    time::Duration,
};

use super::{frame_handler::handle_frame, ConnInner};
use crate::{
    frames::{FrameDecoder, FrameParseError, FrameState, Opcode},
    role::RolePolicy,
    CloseReason, Event, MAX_FRAME_PAYLOAD,
};

pub struct WebSocket<R: RolePolicy + Send + Sync + 'static> {
    pub(crate) inner: Arc<ConnInner<R>>,
    pub(crate) event_rx: Receiver<Event>,
}

/// Best-effort close if user forgets to call [`WebSocket::close`].
impl<R: RolePolicy + Send + Sync + 'static> Drop for WebSocket<R> {
    fn drop(&mut self) {
        if !self.inner.closing.load(Ordering::Acquire) {
            let _ = self.close();
        }
    }
}

impl<R: RolePolicy + Send + Sync + 'static> WebSocket<R> {
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

    pub(crate) fn ping_loop(&self, interval_secs: u64, event_tx: Sender<Event>) {
        // default to 30s
        let interval = Duration::from_secs(interval_secs);
        let inner = self.inner.clone();
        thread::spawn(move || loop {
            if inner.closing.load(Ordering::Acquire) {
                tracing::info!("socket closing, stopping ping loop");
                break;
            }
            if let Err(e) = inner.ping() {
                tracing::warn!("Ping failed, stopping ping loop.");
                let _ = event_tx.send(Event::Error(e));
                break;
            }
            thread::sleep(interval);
        });
    }

    pub(crate) fn recv_loop(&self, event_tx: Sender<Event>) {
        let inner = self.inner.clone();
        thread::spawn(move || {
            let mut buf = vec![0; MAX_FRAME_PAYLOAD];
            let mut partial_msg = None;

            let mut fd = FrameDecoder::<R>::new();
            loop {
                let n = {
                    match inner.reader.lock().unwrap().read(&mut buf) {
                        Ok(0) => {
                            tracing::info!("TCP FIN");
                            break;
                        }
                        Ok(n) => n,
                        Err(e) => {
                            tracing::warn!(error = ?e, "reader error");
                            break;
                        }
                    }
                };
                tracing::trace!(bytes = n, "read socket");

                fd.push_bytes(&buf[..n]);
                loop {
                    match fd.next_frame() {
                        Ok(Some(FrameState::Complete(frame))) => {
                            if handle_frame(&frame, &inner, &mut partial_msg, &event_tx).is_none() {
                                // finished processing frames for now
                                // if the connection is closing, just read and discard until FIN
                                inner.closing.store(true, Ordering::Release);
                                break;
                            }
                        }

                        Ok(Some(FrameState::Incomplete)) => {
                            // break to read more bytes
                            break;
                        }
                        Ok(None) => {
                            // EOF
                            break;
                        }
                        Err(FrameParseError::ProtoError) => {
                            // close connection with ProtoError
                            tracing::warn!("protocol violation detected, entering closing state");
                            inner.closing.store(true, Ordering::Release);
                            let _ = inner.close(
                                CloseReason::ProtoError,
                                "There was a ws protocol violation.",
                            );
                            break;
                        }
                        Err(FrameParseError::SizeErr) => {
                            // close connection with TooBig
                            tracing::warn!("size error detected, entering closing state");
                            inner.closing.store(true, Ordering::Release);
                            let _ = inner.close(CloseReason::TooBig, "Frame exceeded maximum size");
                            break;
                        }
                    }
                }
            }
            inner.closing.store(true, Ordering::Release);
            inner.closed.store(true, Ordering::Release);
            let _ = event_tx.send(Event::Closed);
        });
    }
}

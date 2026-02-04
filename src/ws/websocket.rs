use std::{
    net::SocketAddr,
    sync::{Arc, atomic::Ordering},
    time::Duration,
};

use tokio::{
    io::{AsyncReadExt, Result},
    net::tcp::OwnedReadHalf,
    sync::mpsc::{Receiver, Sender},
    time::interval,
};

use super::{ConnInner, frame_handler::handle_frame};
use crate::{
    CloseReason, Event, MAX_FRAME_PAYLOAD,
    frames::{FrameDecoder, FrameParseError, FrameState, Opcode},
    role::RolePolicy,
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
    pub async fn send_text(&self, text: &str) -> Result<()> {
        self.inner.send(text.as_bytes(), Opcode::Text).await
    }

    pub async fn send_bytes(&self, bytes: &[u8]) -> Result<()> {
        self.inner.send(bytes, Opcode::Bin).await
    }

    pub async fn close(&mut self) -> Result<()> { self.close_reason(CloseReason::Normal, "").await }

    pub async fn close_payload(&mut self, payload: &[u8]) -> Result<()> {
        if self.inner.closing.swap(true, Ordering::AcqRel) {
            return Ok(());
        }
        self.inner.close_raw(payload).await
    }

    pub async fn close_reason(&mut self, reason: CloseReason, text: &'static str) -> Result<()> {
        if self.inner.closing.swap(true, Ordering::AcqRel) {
            return Ok(());
        }
        self.inner.close(reason, text).await
    }

    pub async fn ping(&self) -> Result<()> { self.inner.ping().await }

    pub async fn addr(&self) -> Result<SocketAddr> { self.inner.addr().await }

    /// Average latency in ms form last 5 pings
    #[must_use]
    pub async fn latency(&self) -> Option<u16> { self.inner.ping_stats.lock().await.average() }

    pub fn recv(&mut self) -> Option<Event> { self.event_rx.blocking_recv() }

    pub async fn recv_timeout(&mut self, timeout: Duration) -> Option<Event> {
        tokio::time::timeout(timeout, self.event_rx.recv())
            .await
            .unwrap_or_default()
    }

    pub(crate) fn ping_loop(&self, interval_secs: u64, event_tx: Sender<Event>) {
        let inner = self.inner.clone();
        tokio::spawn(async move {
            let mut interval = interval(Duration::from_secs(interval_secs));
            loop {
                interval.tick().await;
                if inner.closing.load(Ordering::Acquire) {
                    tracing::info!("socket closing, stopping ping loop");
                    break;
                }

                if let Err(e) = inner.ping().await {
                    tracing::warn!("Ping failed, stopping ping loop.");
                    let _ = event_tx.send(Event::Error(e)).await;
                    break;
                }
            }
        });
    }

    pub(crate) fn recv_loop(&self, mut reader: OwnedReadHalf, event_tx: Sender<Event>) {
        let inner = self.inner.clone();
        tokio::spawn(async move {
            let mut buf = vec![0; MAX_FRAME_PAYLOAD];
            let mut partial_msg = None;

            let mut fd = FrameDecoder::<R>::new();
            loop {
                let n = {
                    match reader.read(&mut buf).await {
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
                            if handle_frame(&frame, &inner, &mut partial_msg, &event_tx)
                                .await
                                .is_none()
                            {
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
                            let _ = inner
                                .close(
                                    CloseReason::ProtoError,
                                    "There was a ws protocol violation.",
                                )
                                .await;
                            break;
                        }
                        Err(FrameParseError::SizeErr) => {
                            // close connection with TooBig
                            tracing::warn!("size error detected, entering closing state");
                            inner.closing.store(true, Ordering::Release);
                            let _ = inner
                                .close(CloseReason::TooBig, "Frame exceeded maximum size")
                                .await;
                            break;
                        }
                    }
                }
            }
            inner.closing.store(true, Ordering::Release);
            inner.closed.store(true, Ordering::Release);
            let _ = event_tx.send(Event::Closed).await;
        });
    }
}

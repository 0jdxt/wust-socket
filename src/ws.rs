use std::{
    io::{Read, Result, Write},
    marker::PhantomData,
    sync::{
        Arc,
        atomic::Ordering,
        mpsc::{Receiver, Sender},
    },
    thread,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use crate::{
    CloseReason, Event, MAX_FRAME_PAYLOAD, MAX_MESSAGE_SIZE,
    frames::{ControlFrame, DecodedFrame, FrameDecoder, FrameParseError, FrameState, Opcode},
    inner::InnerTrait,
    message::{Message, PartialMessage},
    role::{DecodePolicy, EncodePolicy},
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

    pub(crate) fn recv_loop<H: FrameHandler<R, I> + Send + 'static>(
        &self,
        event_tx: Sender<Event>,
        handler: H,
    ) {
        let inner = Arc::clone(&self.inner);
        thread::spawn(move || {
            let mut buf = vec![0; MAX_FRAME_PAYLOAD];
            let mut partial_msg = None;

            let span = tracing::info_span!(
                "recv",
                addr = %inner.addr().unwrap(),
                role = if R::MASK_OUTGOING { "CLI" } else { "SRV" },
            );
            let _enter = span.enter();

            let mut fd = FrameDecoder::<R>::new();
            loop {
                let n = {
                    match inner.reader().lock().unwrap().read(&mut buf) {
                        Ok(0) => {
                            tracing::info!("TCP FIN");
                            break;
                        }
                        Err(e) => {
                            tracing::warn!(error = ?e, "reader error");
                            break;
                        }
                        Ok(n) => n,
                    }
                };
                tracing::trace!(bytes = n, "read socket");

                fd.push_bytes(&buf[..n]);
                loop {
                    match fd.next_frame() {
                        Ok(Some(FrameState::Complete(frame))) => {
                            if handler
                                .handle_frame(frame, &inner, &mut partial_msg, &event_tx)
                                .is_none()
                            {
                                // finished processing frames for now
                                // if the connection is closing, just read and discard until FIN
                                inner.closing().store(true, Ordering::Release);
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
                            inner.closing().store(true, Ordering::Release);
                            let _ = inner.close(
                                CloseReason::ProtoError,
                                "There was a ws protocol violation.",
                            );
                            break;
                        }
                        Err(FrameParseError::SizeErr) => {
                            // close connection with TooBig
                            tracing::warn!("size error detected, entering closing state");
                            inner.closing().store(true, Ordering::Release);
                            let _ = inner.close(CloseReason::TooBig, "Frame exceeded maximum size");
                            break;
                        }
                    }
                }
            }
            inner.closing().store(true, Ordering::Release);
            inner.closed().store(true, Ordering::Release);
            let _ = event_tx.send(Event::Closed);
        });
    }
}

pub(crate) trait FrameHandler<P: EncodePolicy, I: InnerTrait<P>> {
    fn handle_frame(
        &self,
        frame: DecodedFrame,
        inner: &Arc<I>,
        partial_msg: &mut Option<PartialMessage>,
        event_tx: &Sender<Event>,
    ) -> Option<()>;
}

pub(crate) struct Handler<P: EncodePolicy + DecodePolicy> {
    _p: PhantomData<P>,
}
impl<P: EncodePolicy + DecodePolicy> Handler<P> {
    pub(crate) fn new() -> Self { Self { _p: PhantomData } }
}
impl<P: DecodePolicy + EncodePolicy, I: InnerTrait<P>> FrameHandler<P, I> for Handler<P> {
    fn handle_frame(
        &self,
        frame: DecodedFrame,
        inner: &Arc<I>,
        partial_msg: &mut Option<PartialMessage>,
        event_tx: &Sender<Event>,
    ) -> Option<()> {
        let role = if P::MASK_OUTGOING { "CLI" } else { "SRV" };
        match frame.opcode {
            // If unsolicited, ignore; otherwise, parse timestamp and calculate latency
            Opcode::Pong => {
                // try to parse payload as timestamp,
                // otherwise its unsolicited and we ignore
                tracing::debug!("received PONG");
                if let Ok(bytes) = frame.payload.try_into() {
                    let sent = u128::from_be_bytes(bytes);
                    let now = SystemTime::now()
                        .duration_since(UNIX_EPOCH)
                        .unwrap()
                        .as_millis();

                    // assume anomalous if latency > u64::MAX
                    if let Ok(latency) = u64::try_from(now - sent) {
                        inner
                            .ping_stats()
                            .lock()
                            .unwrap()
                            .add(Duration::from_millis(latency));

                        let _ = event_tx.send(Event::Pong(latency));
                    }
                }
            }
            // Reply with pong
            Opcode::Ping => {
                tracing::debug!("received PING, scheduling PONG");
                let bytes = ControlFrame::<P>::pong(&frame.payload).encode();
                let mut ws = inner.writer().lock().unwrap();
                let _ = ws.write_all(&bytes);
            }
            // Build message out of frames
            Opcode::Text | Opcode::Bin | Opcode::Cont => {
                // TODO: Leniency
                // allow overwriting partial messages
                // if we get a new TEXT or BINARY
                tracing::trace!("{role} {}, {:?}", partial_msg.is_some(), frame.opcode);
                let partial = match (partial_msg.as_mut(), frame.opcode) {
                    (None, Opcode::Text) => partial_msg.insert(PartialMessage::Text(vec![])),
                    (None, Opcode::Bin) => partial_msg.insert(PartialMessage::Binary(vec![])),
                    (Some(p), Opcode::Cont) => p,
                    _ => {
                        // if we get a CONT before TEXT or BINARY
                        // or we get TEXT/BINARY without finishing the last message
                        // close the connection
                        let _ = inner.close(CloseReason::ProtoError, "Unexpected frame");
                        return None;
                    }
                };

                if partial.len() + frame.payload.len() > MAX_MESSAGE_SIZE {
                    let _ = inner.close(CloseReason::TooBig, "Message exceeded maximum size");
                    return None;
                }

                partial.push_bytes(&frame.payload);
                tracing::trace!(
                    current_len = partial.len(),
                    added = frame.payload.len(),
                    "message fragment appended"
                );

                if frame.is_fin {
                    let msg = match partial_msg.take().unwrap() {
                        PartialMessage::Binary(buf) => Message::Binary(buf),
                        PartialMessage::Text(buf) => {
                            // if invalid UTF-8 immediately close connection
                            if let Ok(s) = String::from_utf8(buf) {
                                Message::Text(s)
                            } else {
                                let _ = inner.close(CloseReason::DataError, "Invalid UTF-8");
                                return None;
                            }
                        }
                    };
                    tracing::debug!(
                        opcode = ?frame.opcode,
                        total_len = msg.len(),
                        "message assembly complete"
                    );
                    let _ = event_tx.send(Event::Message(msg));
                }
            }
            // If closing, shutdown; otherwise, reply with close frame
            Opcode::Close => {
                let code = CloseReason::from([frame.payload[0], frame.payload[1]]);
                tracing::info!(reason=?code, "recieved Close frame");
                // if not already closing try to send close frame, log err
                if !inner.closing().swap(true, Ordering::AcqRel)
                    && let Err(e) = inner.close_raw(&frame.payload)
                {
                    tracing::warn!("{role} Err: error sending close: {e}");
                    let _ = event_tx.send(Event::Error(e));
                }

                return None;
            }
        }
        Some(())
    }
}

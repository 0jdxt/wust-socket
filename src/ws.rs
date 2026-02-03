use std::{
    io::{Read, Result},
    marker::PhantomData,
    sync::{
        Arc,
        atomic::Ordering,
        mpsc::{Receiver, Sender},
    },
    thread,
    time::Duration,
};

use crate::{
    CloseReason, Event, MAX_FRAME_PAYLOAD, MAX_MESSAGE_SIZE,
    frames::{ControlFrame, DecodedFrame, FrameDecoder, FrameParseError, FrameState, Opcode},
    inner::InnerTrait,
    message::{Message, PartialMessage},
    ping::PongError,
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

    pub(crate) fn recv_loop(&self, event_tx: Sender<Event>) {
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

fn handle_frame<P: EncodePolicy, I: InnerTrait<P>>(
    frame: &DecodedFrame,
    inner: &Arc<I>,
    partial_msg: &mut Option<PartialMessage>,
    event_tx: &Sender<Event>,
) -> Option<()> {
    match frame.opcode {
        // Try to parse payload as nonce and check it matches,
        // otherwise if latency exceeds u16::MAX ms, we close the connection
        // else its unsolicited and we ignore
        Opcode::Pong => handle_pong(frame, inner, event_tx),
        // Reply with pong
        Opcode::Ping => handle_ping(frame, inner),
        // Build message out of frames
        Opcode::Text | Opcode::Bin | Opcode::Cont => {
            handle_message(frame, partial_msg, inner, event_tx)?;
        }
        // If closing, shutdown; otherwise, reply with close frame
        Opcode::Close => {
            handle_close(frame, inner, event_tx);
            return None;
        }
    }
    Some(())
}

fn handle_ping<P, I>(frame: &DecodedFrame, inner: &Arc<I>)
where
    P: EncodePolicy,
    I: InnerTrait<P>,
{
    tracing::debug!("received PING, scheduling PONG");
    let bytes = ControlFrame::<P>::pong(&frame.payload).encode();
    let _ = inner.write_once(&bytes);
}

fn handle_pong<P: EncodePolicy, I: InnerTrait<P>>(
    frame: &DecodedFrame,
    inner: &Arc<I>,
    event_tx: &Sender<Event>,
) {
    tracing::debug!("received PONG");
    if let Ok(bytes) = frame.payload.as_slice().try_into() {
        let nonce = u16::from_be_bytes(bytes);
        match inner.ping_stats().lock().unwrap().on_pong(nonce) {
            Ok(latency) => {
                let _ = event_tx.send(Event::Pong(latency));
            }
            Err(PongError::Late(latency)) => {
                tracing::warn!(latency = latency, "late pong");
                let _ = inner.close(CloseReason::Policy, "ping timeout");
            }
            Err(PongError::Nonce(expected)) => {
                tracing::warn!(got = nonce, expected = expected, "mismatched pong nonce");
            }
        }
    }
}

fn handle_close<P: EncodePolicy, I: InnerTrait<P>>(
    frame: &DecodedFrame,
    inner: &Arc<I>,
    event_tx: &Sender<Event>,
) {
    let code = CloseReason::from([frame.payload[0], frame.payload[1]]);
    tracing::info!(reason=?code, "recieved Close frame");
    // if not already closing try to send close frame, log err
    if !inner.closing().swap(true, Ordering::AcqRel)
        && let Err(e) = inner.close_raw(&frame.payload)
    {
        tracing::warn!("Err: error sending close: {e}");
        let _ = event_tx.send(Event::Error(e));
    }
}

fn handle_message<P: EncodePolicy, I: InnerTrait<P>>(
    frame: &DecodedFrame,
    partial_msg: &mut Option<PartialMessage>,
    inner: &Arc<I>,
    event_tx: &Sender<Event>,
) -> Option<()> {
    // TODO: Leniency
    // allow overwriting partial messages
    // if we get a new TEXT or BINARY
    tracing::trace!(
        partial = partial_msg.is_some(),
        opcode = ?frame.opcode,
        "handling message"
    );
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
    Some(())
}

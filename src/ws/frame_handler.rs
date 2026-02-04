use std::sync::{Arc, atomic::Ordering, mpsc::Sender};

use super::ConnInner;
use crate::{
    CloseReason, Event, MAX_MESSAGE_SIZE,
    frames::{ControlFrame, DecodedFrame, Opcode},
    protocol::{Message, PartialMessage, PongError},
    role::RolePolicy,
};

pub(super) fn handle_frame<P: RolePolicy>(
    frame: &DecodedFrame,
    inner: &Arc<ConnInner<P>>,
    partial_msg: &mut Option<PartialMessage>,
    event_tx: &Sender<Event>,
) -> Option<()> {
    match frame.opcode {
        Opcode::Pong => handle_pong(frame, inner, event_tx),
        Opcode::Ping => handle_ping(frame, inner),
        Opcode::Text | Opcode::Bin | Opcode::Cont => {
            handle_message(frame, partial_msg, inner, event_tx)?;
        }
        Opcode::Close => {
            handle_close(frame, inner, event_tx);
            return None;
        }
    }
    Some(())
}

// Reply with pong
fn handle_ping<R: RolePolicy>(frame: &DecodedFrame, inner: &Arc<ConnInner<R>>) {
    tracing::info!("received PING, scheduling PONG");
    let bytes = ControlFrame::<R>::pong(&frame.payload).encode();
    let _ = inner.write_once(&bytes);
}

// Try to parse payload as nonce and check it matches,
// otherwise if latency exceeds u16::MAX ms, we close the connection
// else its unsolicited and we ignore
fn handle_pong<R: RolePolicy>(
    frame: &DecodedFrame,
    inner: &Arc<ConnInner<R>>,
    event_tx: &Sender<Event>,
) {
    tracing::debug!("received PONG");
    if let Ok(bytes) = frame.payload.as_slice().try_into() {
        match inner.ping_stats.lock().unwrap().on_pong(bytes) {
            Ok(latency) => {
                let _ = event_tx.send(Event::Pong(latency));
            }
            Err(PongError::Late(latency)) => {
                tracing::warn!(latency = latency, "late pong");
                let _ = inner.close(CloseReason::Policy, "ping timeout");
            }
            Err(PongError::Nonce(expected)) => {
                tracing::warn!(
                    got = ?bytes,
                    expected = ?expected,
                    "mismatched pong nonce"
                );
            }
        }
    }
}

// If closing, shutdown; otherwise, reply with close frame
fn handle_close<R: RolePolicy>(
    frame: &DecodedFrame,
    inner: &Arc<ConnInner<R>>,
    event_tx: &Sender<Event>,
) {
    let code = CloseReason::from([frame.payload[0], frame.payload[1]]);
    tracing::info!(reason=?code, "recieved Close frame");
    // if not already closing try to send close frame, log err
    if !inner.closing.swap(true, Ordering::AcqRel)
        && let Err(e) = inner.close_raw(&frame.payload)
    {
        tracing::warn!("Err: error sending close: {e}");
        let _ = event_tx.send(Event::Error(e));
    }
}

// Build message out of frames
fn handle_message<R: RolePolicy>(
    frame: &DecodedFrame,
    partial_msg: &mut Option<PartialMessage>,
    inner: &Arc<ConnInner<R>>,
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

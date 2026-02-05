use std::sync::{Arc, atomic::Ordering};

use tokio::sync::mpsc::Sender;

use super::Inner;
use crate::{
    CloseReason, Event, MAX_MESSAGE_SIZE,
    frames::{ControlFrame, DecodedFrame, Opcode},
    protocol::{Message, PartialMessage, PongError},
    role::RolePolicy,
};

pub(super) async fn handle_frame<R: RolePolicy>(
    frame: &DecodedFrame,
    inner: &Arc<Inner>,
    partial_msg: &mut Option<PartialMessage>,
    close_tx: &Sender<Vec<u8>>,
    ctrl_tx: &Sender<Vec<u8>>,
    event_tx: &Sender<Event>,
) -> Option<()> {
    match frame.opcode {
        Opcode::Pong => handle_pong::<R>(frame, close_tx, event_tx, inner).await,
        Opcode::Ping => handle_ping::<R>(frame, ctrl_tx).await,
        Opcode::Text | Opcode::Bin | Opcode::Cont => {
            handle_message::<R>(frame, partial_msg, close_tx, event_tx).await?;
        }
        Opcode::Close => {
            handle_close::<R>(frame, inner, close_tx, event_tx).await;
            return None;
        }
    }
    Some(())
}

// Reply with pong
async fn handle_ping<R: RolePolicy>(frame: &DecodedFrame, ctrl_tx: &Sender<Vec<u8>>) {
    tracing::info!("received PING, scheduling PONG");
    let bytes = ControlFrame::<R>::pong(&frame.payload).encode();
    let _ = ctrl_tx.send(bytes).await;
}

// Try to parse payload as nonce and check it matches,
// otherwise if latency exceeds u16::MAX ms, we close the connection
// else its unsolicited and we ignore
async fn handle_pong<R: RolePolicy>(
    frame: &DecodedFrame,
    close_tx: &Sender<Vec<u8>>,
    event_tx: &Sender<Event>,
    inner: &Arc<Inner>,
) {
    tracing::debug!("received PONG");
    if let Ok(bytes) = frame.payload.as_slice().try_into() {
        match inner.ping_stats.lock().await.on_pong(bytes) {
            Ok(latency) => {
                let _ = event_tx.send(Event::Pong(latency)).await;
            }
            Err(PongError::Late(latency)) => {
                tracing::warn!(latency = latency, "late pong");
                let _ = close_tx
                    .send(ControlFrame::<R>::close_reason(
                        CloseReason::Policy,
                        "ping timeout",
                    ))
                    .await;
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
async fn handle_close<R: RolePolicy>(
    frame: &DecodedFrame,
    inner: &Arc<Inner>,
    close_tx: &Sender<Vec<u8>>,
    event_tx: &Sender<Event>,
) {
    let code = CloseReason::from([frame.payload[0], frame.payload[1]]);
    tracing::info!(reason=?code, "recieved Close frame");
    // if not already closing try to send close frame, log err
    if !inner.closing.swap(true, Ordering::AcqRel) {
        let f = ControlFrame::<R>::close(&frame.payload);
        if let Err(e) = close_tx.send(f.encode()).await {
            tracing::warn!("Err: error sending close: {e}");
            let _ = event_tx.send(Event::Error(e)).await;
        }
    }
}

// Build message out of frames
async fn handle_message<R: RolePolicy>(
    frame: &DecodedFrame,
    partial_msg: &mut Option<PartialMessage>,
    close_tx: &Sender<Vec<u8>>,
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

            let _ = close_tx
                .send(ControlFrame::<R>::close_reason(
                    CloseReason::ProtoError,
                    "Unexpected frame",
                ))
                .await;
            return None;
        }
    };

    if partial.len() + frame.payload.len() > MAX_MESSAGE_SIZE {
        let _ = close_tx
            .send(ControlFrame::<R>::close_reason(
                CloseReason::TooBig,
                "Message exceeded maximum size",
            ))
            .await;
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
                    let _ = close_tx
                        .send(ControlFrame::<R>::close_reason(
                            CloseReason::DataError,
                            "Invalid UTF-8",
                        ))
                        .await;
                    return None;
                }
            }
        };
        tracing::debug!(
            opcode = ?frame.opcode,
            total_len = msg.len(),
            "message assembly complete"
        );
        let _ = event_tx.send(Event::Message(msg)).await;
    }
    Some(())
}

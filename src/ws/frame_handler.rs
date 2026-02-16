use std::sync::{Arc, atomic::Ordering};

use flate2::write::DeflateDecoder;

use super::{Inner, PartialMessage};
use crate::{
    Event, MAX_MESSAGE_SIZE,
    error::CloseReason,
    frames::{DecodedFrame, Opcode, control},
    protocol::PongError,
    role::RolePolicy,
    ws::{event::MessageError, websocket::WsSender},
};

pub(super) async fn handle_frame<R: RolePolicy>(
    frame: &DecodedFrame,
    inner: &Arc<Inner>,
    partial_msg: &mut Option<PartialMessage>,
    sender: &WsSender,
    inflater: &mut Option<DeflateDecoder<Vec<u8>>>,
    use_context: bool,
) -> Option<()> {
    tracing::trace!(
        "got frame {:?} {} fin={}",
        frame.opcode,
        frame.payload.len(),
        frame.is_fin
    );
    match frame.opcode {
        Opcode::Text | Opcode::Bin | Opcode::Cont => {
            handle_data::<R>(frame, partial_msg, sender, inflater, use_context).await?;
        }
        Opcode::Pong => handle_pong::<R>(frame, sender, inner).await,
        Opcode::Ping => handle_ping::<R>(frame, sender).await,
        Opcode::Close => {
            handle_close::<R>(frame, inner, sender).await;
            return None;
        }
    }
    Some(())
}

// Reply with pong
async fn handle_ping<R: RolePolicy>(frame: &DecodedFrame, sender: &WsSender) {
    tracing::info!("received PING, scheduling PONG");
    let bytes = control::pong::<R>(&frame.payload);
    let _ = sender.ctrl(bytes).await;
}

// Try to parse payload as nonce and check it matches,
// otherwise if latency exceeds u16::MAX ms, we close the connection
// else its unsolicited and we ignore
async fn handle_pong<R: RolePolicy>(frame: &DecodedFrame, sender: &WsSender, inner: &Arc<Inner>) {
    tracing::debug!("received PONG");
    if let Ok(bytes) = (*frame.payload).try_into() {
        match inner.ping_stats.lock().await.on_pong(bytes) {
            Ok(latency) => {
                let _ = sender.event(Event::Pong(latency)).await;
            }
            Err(PongError::Late(latency)) => {
                tracing::warn!(latency = latency, "late pong");
                let _ = sender
                    .close(control::close::<R>(CloseReason::Policy, "ping timeout"))
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
async fn handle_close<R: RolePolicy>(frame: &DecodedFrame, inner: &Arc<Inner>, sender: &WsSender) {
    // Here we parse the close reason in order to give the appropriate response.
    // If empty, treat as normal. otherwise we validate close payload
    let code = if frame.payload.is_empty() {
        tracing::info!("recieved empty Close frame");
        CloseReason::Normal
    } else {
        let Ok(text) = str::from_utf8(&frame.payload[2..]) else {
            let _ = sender
                .close(control::close::<R>(
                    CloseReason::ProtoError,
                    "invalid close message",
                ))
                .await;
            return;
        };
        let code = CloseReason::from([frame.payload[0], frame.payload[1]]);
        tracing::info!(reason=?code, text=text, "recieved Close frame");
        code
    };

    let reason = match code {
        // codes that should never touch the wire
        CloseReason::Rsv | CloseReason::NoneGiven | CloseReason::Abnormal | CloseReason::Tls => {
            CloseReason::ProtoError
        }
        // we dont echo any codes back, jsut reply with normal
        _ => CloseReason::Normal,
    };

    // if not already closing try to send close frame, log err
    if !inner.closing.swap(true, Ordering::AcqRel) {
        tracing::trace!(reason=?reason, "sending Close frame");
        if sender
            .close(control::close::<R>(reason, "peer closed"))
            .await
            .is_err()
        {
            tracing::trace!("close_rx dropped");
        }
    }
}

// Build message out of frames
async fn handle_data<R: RolePolicy>(
    frame: &DecodedFrame,
    partial_msg: &mut Option<PartialMessage>,
    sender: &WsSender,
    inflater: &mut Option<DeflateDecoder<Vec<u8>>>,
    use_context: bool,
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
        (None, Opcode::Text) => partial_msg.insert(PartialMessage::text()),
        (None, Opcode::Bin) => partial_msg.insert(PartialMessage::binary()),
        // CONT frames must NEVER set RSV1
        (Some(p), Opcode::Cont) if !frame.compressed => p,
        _ => {
            // if we get a CONT before TEXT or BINARY
            // or we get TEXT/BINARY without finishing the last message
            // close the connection
            let _ = sender
                .close(control::close::<R>(
                    CloseReason::ProtoError,
                    "Unexpected frame",
                ))
                .await;
            return None;
        }
    };

    if partial.len() + frame.payload.len() > MAX_MESSAGE_SIZE {
        let _ = sender
            .close(control::close::<R>(
                CloseReason::TooBig,
                "Message exceeded maximum size",
            ))
            .await;
        return None;
    }

    tracing::trace!(
        current_len = partial.len(),
        added = frame.payload.len(),
        "message fragment appended"
    );
    partial.push_bytes(&frame.payload);

    if frame.is_fin {
        match partial_msg
            .take()
            .unwrap()
            .into_message(inflater, use_context)
        {
            Ok(msg) => {
                tracing::trace!(
                    opcode = ?frame.opcode,
                    total_len = msg.len(),
                    "message assembly complete"
                );
                let _ = sender.event(msg).await;
            }
            Err(MessageError::Utf8) => {
                let _ = sender
                    .close(control::close::<R>(CloseReason::DataError, "Invalid UTF-8"))
                    .await;
                return None;
            }
            Err(MessageError::Deflate) => {
                let _ = sender
                    .close(control::close::<R>(
                        CloseReason::ProtoError,
                        "bad deflate stream",
                    ))
                    .await;
                return None;
            }
        }
    }
    Some(())
}

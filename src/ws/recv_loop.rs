use std::{
    io::Read,
    sync::{atomic::Ordering, mpsc::Sender, Arc},
    thread,
};

use super::frame_handler::handle_frame;
use crate::{
    frames::{FrameDecoder, FrameParseError, FrameState},
    role::{DecodePolicy, EncodePolicy},
    CloseReason, Event, WebSocket, MAX_FRAME_PAYLOAD,
};

impl<R: EncodePolicy + DecodePolicy + Send + Sync + 'static> WebSocket<R> {
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

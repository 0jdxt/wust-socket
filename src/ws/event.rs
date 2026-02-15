use std::io::Write;

use bytes::{Bytes, BytesMut};
use flate2::write::DeflateDecoder;

use crate::MAX_MESSAGE_SIZE;

/// `Event`s are produced by [`WebSocketClient::recv`](crate::WebSocketClient::recv)
/// and [`WebSocketClient::recv_timeout`](crate::WebSocketClient::recv_timeout)
#[derive(Debug)]
pub enum Event {
    /// Pong event with its latency in milliseconds.
    Pong(u16),
    /// Valid UTF-8 message.
    Text(Bytes),
    /// Binary message bytes.
    Binary(Bytes),
    /// The connection to the websocket has been closed.
    Closed,
    /// An error sending a message, generally indicating the connection closed.
    /// Returns the bytes that failed to send.
    Error(Bytes),
}

impl Event {
    pub(crate) fn len(&self) -> usize {
        match self {
            Self::Text(s) => s.len(),
            Self::Binary(b) => b.len(),
            Self::Error(e) => e.len(),
            _ => 0,
        }
    }
}

#[derive(Debug)]
pub(crate) enum PartialMessage {
    Text(BytesMut),
    Binary(BytesMut),
}

#[derive(Debug)]
pub(crate) enum MessageError {
    Utf8,
    Deflate,
}

impl PartialMessage {
    pub(crate) fn text() -> Self { Self::Text(BytesMut::with_capacity(MAX_MESSAGE_SIZE)) }

    pub(crate) fn binary() -> Self { Self::Binary(BytesMut::with_capacity(MAX_MESSAGE_SIZE)) }

    pub(crate) fn push_bytes(&mut self, bytes: &[u8]) {
        match self {
            Self::Text(v) | Self::Binary(v) => v.extend_from_slice(bytes),
        }
    }

    pub(crate) fn len(&self) -> usize {
        match self {
            Self::Text(v) | Self::Binary(v) => v.len(),
        }
    }

    pub(crate) fn into_message(
        self,
        inflater: &mut Option<DeflateDecoder<Vec<u8>>>,
        use_context: bool,
    ) -> Result<Event, MessageError> {
        let (mut data, text) = match self {
            Self::Text(v) => (v, true),
            Self::Binary(v) => (v, false),
        };

        if let Some(inflater) = inflater {
            let init_size = data.len();

            let end = if use_context {
                inflater.get_ref().len()
            } else {
                data.extend_from_slice(&[0, 0, 0xFF, 0xFF]);
                let _ = inflater.reset(vec![]);
                0
            };

            if inflater.write_all(&data).is_err() || inflater.flush().is_err() {
                return Err(MessageError::Deflate);
            }
            data.clear();
            data.extend_from_slice(&inflater.get_ref()[end..]);
            tracing::trace!("inflated {init_size} => {}", data.len());
        }
        let data = data.freeze();

        if text {
            if str::from_utf8(&data).is_err() {
                return Err(MessageError::Utf8);
            }
            Ok(Event::Text(data))
        } else {
            Ok(Event::Binary(data))
        }
    }
}

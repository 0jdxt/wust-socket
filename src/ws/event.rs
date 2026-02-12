use std::io::Write;

use flate2::write::DeflateDecoder;
use tokio::sync::mpsc::error::SendError;

/// `Event`s are produced by [`WebSocketClient::recv`](crate::WebSocketClient::recv)
/// and [`WebSocketClient::recv_timeout`](crate::WebSocketClient::recv_timeout)
#[derive(Debug)]
pub enum Event {
    /// Pong event with its latency in milliseconds.
    Pong(u16),
    /// A text or binary message.
    Message(Message),
    /// The connection to the websocket has been closed.
    Closed,
    /// An error sending a message, generally indicating the connection closed.
    /// Returns the bytes that failed to send.
    Error(SendError<Vec<u8>>),
}

/// Assembled messages recieved from an endpoint.
#[derive(Debug)]
pub enum Message {
    /// Valid UTF-8 message.
    Text(String),
    /// Binary message bytes.
    Binary(Vec<u8>),
}

impl Message {
    /// If the type is `Message::Text`, returns a reference to the internal `String`, otherwise
    /// `None`.
    #[must_use]
    pub fn as_str(&self) -> Option<&str> {
        match self {
            Message::Binary(..) => None,
            Message::Text(s) => Some(s),
        }
    }

    /// Returns a reference to the data as bytes.
    #[must_use]
    pub fn as_bytes(&self) -> &[u8] {
        match self {
            Message::Binary(b) => b,
            Message::Text(s) => s.as_bytes(),
        }
    }

    #[must_use]
    pub fn len(&self) -> usize {
        match self {
            Message::Binary(b) => b.len(),
            Message::Text(s) => s.len(),
        }
    }

    #[must_use]
    pub fn is_empty(&self) -> bool { self.len() == 0 }
}

#[derive(Debug)]
pub(crate) enum PartialMessage {
    Text(Vec<u8>),
    Binary(Vec<u8>),
}

impl PartialMessage {
    pub(crate) fn text() -> Self { Self::Text(vec![]) }

    pub(crate) fn binary() -> Self { Self::Binary(vec![]) }

    pub(crate) fn push_bytes(&mut self, bytes: &[u8]) {
        match self {
            Self::Text(v) | Self::Binary(v) => v.extend(bytes),
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
    ) -> Option<Message> {
        let (mut data, text) = match self {
            Self::Text(v) => (v, true),
            Self::Binary(v) => (v, false),
        };

        if let Some(inflater) = inflater {
            if use_context {
                data.extend_from_slice(&[0, 0, 0xFF, 0xFF]);
            } else {
                inflater.reset(vec![]).unwrap();
            }
            inflater.write_all(&data).unwrap();
            inflater.flush().unwrap();
            data.clone_from(inflater.get_ref());
        }

        if text {
            String::from_utf8(data).map(Message::Text).ok()
        } else {
            Some(Message::Binary(data))
        }
    }
}

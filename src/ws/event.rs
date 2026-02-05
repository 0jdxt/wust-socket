use tokio::sync::mpsc::error::SendError;

use crate::protocol::Message;

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
    /// An `io::Error`
    Error(SendError<Vec<u8>>),
}

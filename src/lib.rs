#![feature(test)]
#![warn(clippy::all, clippy::pedantic)]

mod client;
mod error;
mod frames;
mod protocol;
mod role;
mod server;
mod ws;

/// This is a re-export of [`async_trait::async_trait`]
///
/// extra context
pub use async_trait::async_trait;
pub use client::WebSocketClient;
pub use error::UpgradeError;
pub use server::WebSocketServer;
pub use ws::{Event, Message, MessageHandler, WebSocket, WsMessage};

// If using autobahn, set frames to 16M for testing
// otherwise our real max is 16K frames
const AUTOBAHN: bool = true;
pub(crate) const MAX_FRAME_PAYLOAD: usize = if AUTOBAHN {
    16 * 1024 * 1024 // 16M
} else {
    16 * 1024 // 16K
};

pub(crate) const MAX_MESSAGE_SIZE: usize = 16 * 1024 * 1024; // 16MB

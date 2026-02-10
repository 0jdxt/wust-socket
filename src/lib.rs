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

pub(crate) const MAX_FRAME_PAYLOAD: usize = 16 * 1024 * 1024; // 16M
pub(crate) const MAX_MESSAGE_SIZE: usize = MAX_FRAME_PAYLOAD;

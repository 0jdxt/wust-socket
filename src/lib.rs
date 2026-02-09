#![feature(test)]
#![warn(clippy::all, clippy::pedantic)]

mod client;
mod error;
mod frames;
mod protocol;
mod role;
mod server;
mod ws;

pub use client::WebSocketClient;
pub use error::{CloseReason, UpgradeError};
pub use protocol::Message;
pub use server::{MessageHandler, ServerConn, WebSocketServer};
pub use ws::{Event, WebSocket};

pub(crate) const MAX_FRAME_PAYLOAD: usize = 16 * 1024 * 1024; // 16M
pub(crate) const MAX_MESSAGE_SIZE: usize = MAX_FRAME_PAYLOAD;

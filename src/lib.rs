#![feature(test)]
#![warn(clippy::all, clippy::pedantic)]
#![allow(
    clippy::empty_docs,
    clippy::missing_errors_doc,
    clippy::missing_panics_doc,
    clippy::missing_safety_doc
)]

mod client;
mod error;
mod frames;
mod inner;
mod protocol;
mod role;
mod server;
mod ws;

pub use client::{UpgradeError, WebSocketClient};
pub use error::CloseReason;
pub use protocol::Message;
pub use server::{ServerConn, WebSocketServer};
pub use ws::{Event, WebSocket};

pub(crate) const MAX_FRAME_PAYLOAD: usize = 32 * 1024;
pub(crate) const MAX_MESSAGE_SIZE: usize = 1024 * 1024;

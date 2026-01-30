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
mod event;
mod frames;
mod inner;
mod mask;
mod message;
mod ping;
mod role;
mod server;
mod ws;

pub use client::WebSocketClient;
pub use error::CloseReason;
pub use event::Event;
pub use message::Message;
pub use server::{ServerConn, WebSocketServer};
pub use ws::WebSocket;

pub(crate) const MAX_FRAME_PAYLOAD: usize = 32 * 1024;
pub(crate) const MAX_MESSAGE_SIZE: usize = 1024 * 1024;

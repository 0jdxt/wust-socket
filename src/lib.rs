#![warn(clippy::all, clippy::pedantic)]
#![allow(
    clippy::empty_docs,
    clippy::missing_errors_doc,
    clippy::missing_panics_doc,
    clippy::missing_safety_doc
)]

mod error;
mod event;
mod frames;
mod message;
mod ping;
mod role;
mod websocket;

pub use error::CloseReason;
pub use event::Event;
pub use message::Message;
pub use websocket::WebSocket;

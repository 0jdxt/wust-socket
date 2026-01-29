use std::{
    io::Result,
    sync::atomic::AtomicBool,
    time::{SystemTime, UNIX_EPOCH},
};

use crate::{
    frames::{ControlFrame, DataFrame, Opcode},
    role::Role,
    CloseReason,
};

pub trait InnerTrait {
    const ROLE: Role;
    fn write_chunks(&self, chunks: impl IntoIterator<Item = Vec<u8>>) -> Result<()>;
    // getters
    fn closing(&self) -> &AtomicBool;
    fn closed(&self) -> &AtomicBool;

    // send data (bytes) over the websocket
    fn send(&self, bytes: &[u8], ty: Opcode) -> Result<()> {
        let frame = DataFrame::new(bytes, ty, Self::ROLE);
        self.write_chunks(frame.encode())
    }

    // send close request
    fn close_raw(&self, payload: &[u8]) -> Result<()> {
        let frame = ControlFrame::close(payload, Self::ROLE);
        self.write_chunks(std::iter::once(frame.encode()))
    }

    fn close(&self, reason: CloseReason, text: &'static str) -> Result<()> {
        println!("sending close: {reason:?} {text}");
        let code: [u8; 2] = reason.into();
        let mut payload = Vec::with_capacity(2 + text.len());
        payload.extend_from_slice(&code);
        payload.extend_from_slice(text.as_bytes());
        self.close_raw(&payload)
    }

    // send ping with a timestamp
    fn ping(&self) -> Result<()> {
        let timestamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_millis()
            .to_be_bytes();

        let frame = ControlFrame::ping(&timestamp, Self::ROLE);
        self.write_chunks(std::iter::once(frame.encode()))
    }
}

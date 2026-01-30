use std::{
    io::{Result, Write},
    net::TcpStream,
    sync::{atomic::AtomicBool, Mutex},
    time::{SystemTime, UNIX_EPOCH},
};

use crate::{
    frames::{ControlFrame, DataFrame, Opcode},
    role::EncodePolicy,
    CloseReason,
};

pub trait InnerTrait<P: EncodePolicy> {
    fn closing(&self) -> &AtomicBool;
    fn closed(&self) -> &AtomicBool;
    fn writer(&self) -> &Mutex<TcpStream>;

    // send data (bytes) over the websocket
    fn send(&self, bytes: &[u8], ty: Opcode) -> Result<()> {
        let frame = DataFrame::<P>::new(bytes, ty);
        self.write_chunks(frame.encode())
    }

    // send close request
    fn close_raw(&self, payload: &[u8]) -> Result<()> {
        let frame = ControlFrame::<P>::close(payload);
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

        let frame = ControlFrame::<P>::ping(&timestamp);
        self.write_chunks(std::iter::once(frame.encode()))
    }

    fn write_chunks(&self, chunks: impl IntoIterator<Item = Vec<u8>>) -> Result<()> {
        let mut ws = self.writer().lock().unwrap();
        for chunk in chunks {
            ws.write_all(&chunk)?;
        }
        ws.flush()
    }
}

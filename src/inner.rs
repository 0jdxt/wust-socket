use std::{
    io::{Result, Write},
    net::{SocketAddr, TcpStream},
    sync::{atomic::AtomicBool, Mutex},
};

use crate::{
    frames::{ControlFrame, DataFrame, Opcode},
    protocol::PingStats,
    role::EncodePolicy,
    CloseReason,
};

pub trait InnerTrait<P: EncodePolicy> {
    fn closing(&self) -> &AtomicBool;
    fn closed(&self) -> &AtomicBool;
    fn writer(&self) -> &Mutex<TcpStream>;
    fn reader(&self) -> &Mutex<TcpStream>;
    fn ping_stats(&self) -> &Mutex<PingStats>;

    fn addr(&self) -> Result<SocketAddr> { self.writer().lock().unwrap().local_addr() }

    // send data (bytes) over the websocket
    fn send(&self, bytes: &[u8], ty: Opcode) -> Result<()> {
        let frame = DataFrame::<P>::new(bytes, ty);
        self.write_chunks(frame.encode())
    }

    // close with reason code and text
    fn close(&self, reason: CloseReason, text: &'static str) -> Result<()> {
        println!(
            "{}: sending close: {reason:?} {text}",
            if P::MASK_OUTGOING { "CLI" } else { "SRV" }
        );
        let code: [u8; 2] = reason.into();
        let mut payload = Vec::with_capacity(2 + text.len());
        payload.extend_from_slice(&code);
        payload.extend_from_slice(text.as_bytes());
        self.close_raw(&payload)
    }

    // send close request
    fn close_raw(&self, payload: &[u8]) -> Result<()> {
        let bytes = ControlFrame::<P>::close(payload).encode();
        self.write_once(&bytes)
    }

    // send ping with a timestamp
    fn ping(&self) -> Result<()> {
        // send nonce as payload, store Instant
        let payload = self.ping_stats().lock().unwrap().new_ping();
        self.write_once(&ControlFrame::<P>::ping(&payload).encode())
    }

    fn write_once(&self, bytes: &[u8]) -> Result<()> {
        let mut ws = self.writer().lock().unwrap();
        ws.write_all(bytes)?;
        ws.flush()
    }

    fn write_chunks(&self, chunks: impl IntoIterator<Item = Vec<u8>>) -> Result<()> {
        let mut ws = self.writer().lock().unwrap();
        for chunk in chunks {
            ws.write_all(&chunk)?;
        }
        ws.flush()
    }
}

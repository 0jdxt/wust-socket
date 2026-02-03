use std::{
    io::{Result, Write},
    marker::PhantomData,
    net::{SocketAddr, TcpStream},
    sync::{atomic::AtomicBool, Mutex},
};

use crate::{
    frames::{ControlFrame, DataFrame, Opcode},
    protocol::PingStats,
    role::{RolePolicy},
    CloseReason,
};

pub(crate) struct ConnInner<R: RolePolicy> {
    pub(crate) reader: Mutex<TcpStream>,
    pub(crate) writer: Mutex<TcpStream>,
    pub(crate) ping_stats: Mutex<PingStats>,
    pub(crate) closed: AtomicBool,
    pub(crate) closing: AtomicBool,
    pub(crate) _role: PhantomData<R>,
}

impl<R: RolePolicy> ConnInner<R> {
    pub(crate) fn addr(&self) -> Result<SocketAddr> { self.writer.lock().unwrap().local_addr() }

    // send data (bytes) over the websocket
    pub(crate) fn send(&self, bytes: &[u8], ty: Opcode) -> Result<()> {
        let frame = DataFrame::<R>::new(bytes, ty);
        self.write_chunks(frame.encode())
    }

    // close with reason code and text
    pub(crate) fn close(&self, reason: CloseReason, text: &'static str) -> Result<()> {
        println!(
            "{}: sending close: {reason:?} {text}",
            if R::MASK_OUTGOING { "CLI" } else { "SRV" }
        );
        let code: [u8; 2] = reason.into();
        let mut payload = Vec::with_capacity(2 + text.len());
        payload.extend_from_slice(&code);
        payload.extend_from_slice(text.as_bytes());
        self.close_raw(&payload)
    }

    // send close request
    pub(crate) fn close_raw(&self, payload: &[u8]) -> Result<()> {
        let bytes = ControlFrame::<R>::close(payload).encode();
        self.write_once(&bytes)
    }

    // send ping with a timestamp
    pub(crate) fn ping(&self) -> Result<()> {
        // send nonce as payload, store Instant
        let payload = self.ping_stats.lock().unwrap().new_ping();
        self.write_once(&ControlFrame::<R>::ping(&payload).encode())
    }

    pub(crate) fn write_once(&self, bytes: &[u8]) -> Result<()> {
        let mut ws = self.writer.lock().unwrap();
        ws.write_all(bytes)?;
        ws.flush()
    }

    pub(crate) fn write_chunks(&self, chunks: impl IntoIterator<Item = Vec<u8>>) -> Result<()> {
        let mut ws = self.writer.lock().unwrap();
        for chunk in chunks {
            ws.write_all(&chunk)?;
        }
        ws.flush()
    }
}

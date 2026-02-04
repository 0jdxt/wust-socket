use std::{marker::PhantomData, net::SocketAddr, sync::atomic::AtomicBool};

use tokio::{
    io::{AsyncWriteExt, Result},
    net::tcp::OwnedWriteHalf,
    sync::Mutex,
};

use crate::{
    CloseReason,
    frames::{ControlFrame, DataFrame, Opcode},
    protocol::PingStats,
    role::RolePolicy,
};

pub(crate) struct ConnInner<R: RolePolicy> {
    pub(crate) writer: Mutex<OwnedWriteHalf>,
    pub(crate) ping_stats: Mutex<PingStats>,
    pub(crate) closed: AtomicBool,
    pub(crate) closing: AtomicBool,
    pub(crate) _role: PhantomData<R>,
}

impl<R: RolePolicy> ConnInner<R> {
    pub(crate) async fn addr(&self) -> Result<SocketAddr> { self.writer.lock().await.peer_addr() }

    // send data (bytes) over the websocket
    pub(crate) async fn send(&self, bytes: &[u8], ty: Opcode) -> Result<()> {
        let frame = DataFrame::<R>::new(bytes, ty);
        self.write_chunks(frame.encode()).await
    }

    // close with reason code and text
    pub(crate) async fn close(&self, reason: CloseReason, text: &'static str) -> Result<()> {
        tracing::info!(reason = ?reason, "sending close");
        let code: [u8; 2] = reason.into();
        let mut payload = Vec::with_capacity(2 + text.len());
        payload.extend_from_slice(&code);
        payload.extend_from_slice(text.as_bytes());
        self.close_raw(&payload).await
    }

    // send close request
    pub(crate) async fn close_raw(&self, payload: &[u8]) -> Result<()> {
        let bytes = ControlFrame::<R>::close(payload).encode();
        self.write_once(&bytes).await
    }

    // send ping with a timestamp
    pub(crate) async fn ping(&self) -> Result<()> {
        // send nonce as payload, store Instant
        let mut stats = self.ping_stats.lock().await;
        let payload = stats.new_ping();
        self.write_once(&ControlFrame::<R>::ping(payload).encode())
            .await
    }

    pub(crate) async fn write_once(&self, bytes: &[u8]) -> Result<()> {
        let mut ws = self.writer.lock().await;
        ws.write_all(bytes).await?;
        ws.flush().await
    }

    pub(crate) async fn write_chunks(
        &self,
        chunks: impl IntoIterator<Item = Vec<u8>>,
    ) -> Result<()> {
        let mut ws = self.writer.lock().await;
        for chunk in chunks {
            ws.write_all(&chunk).await?;
        }
        ws.flush().await
    }
}

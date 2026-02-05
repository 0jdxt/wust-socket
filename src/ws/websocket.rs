use std::{
    marker::PhantomData,
    net::SocketAddr,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::{Duration, Instant},
};

use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::{
        TcpStream,
        tcp::{OwnedReadHalf, OwnedWriteHalf},
    },
    sync::{
        Mutex,
        mpsc::{Receiver, Sender, channel, error::SendError},
    },
    time::interval,
};

use super::frame_handler::handle_frame;
use crate::{
    CloseReason, Event, MAX_FRAME_PAYLOAD,
    frames::{ControlFrame, DataFrame, FrameDecoder, FrameParseError, FrameState, Opcode},
    protocol::PingStats,
    role::RolePolicy,
};

pub(crate) struct Inner {
    pub(crate) ping_stats: Mutex<PingStats>,
    pub(crate) closed: AtomicBool,
    pub(crate) closing: AtomicBool,
    pub(crate) last_seen: Mutex<Instant>,
}

pub struct WebSocket<R: RolePolicy> {
    pub(crate) inner: Arc<Inner>,
    pub(crate) ctrl_tx: Sender<Vec<u8>>,
    pub(crate) data_tx: Sender<Vec<u8>>,
    pub(crate) close_tx: Sender<Vec<u8>>,
    pub(crate) event_rx: Receiver<Event>,
    pub(crate) _role: PhantomData<R>,
    pub(crate) addr: SocketAddr,
}

/// Best-effort close if user forgets to call [`WebSocket::close`].
impl<R: RolePolicy> Drop for WebSocket<R> {
    fn drop(&mut self) { self.inner.closing.store(true, Ordering::Release); }
}

type Result = std::result::Result<(), SendError<Vec<u8>>>;

impl<R: RolePolicy> WebSocket<R> {
    pub(crate) fn from_stream(stream: TcpStream) -> Self {
        let (event_tx, event_rx) = channel(64);
        let (close_tx, close_rx) = channel(64);
        let (data_tx, data_rx) = channel(64);
        let (ctrl_tx, ctrl_rx) = channel(64);

        let ws = Self {
            inner: Arc::new(Inner {
                ping_stats: Mutex::new(PingStats::new()),
                closed: AtomicBool::new(false),
                closing: AtomicBool::new(false),
                last_seen: Mutex::new(Instant::now()),
            }),
            addr: stream.peer_addr().unwrap(),
            close_tx: close_tx.clone(),
            data_tx,
            ctrl_tx: ctrl_tx.clone(),
            event_rx,
            _role: PhantomData,
        };

        let (reader, writer) = stream.into_split();
        tokio::spawn(Self::writer_loop(close_rx, ctrl_rx, data_rx, writer));
        ws.ping_loop(30, ctrl_tx.clone(), close_tx.clone(), event_tx.clone());
        ws.reader_loop(reader, ctrl_tx, close_tx, event_tx);
        ws
    }

    pub async fn send_text(&self, text: &str) -> Result {
        self.send_data(text.as_bytes(), Opcode::Text).await
    }

    pub async fn send_bytes(&self, bytes: &[u8]) -> Result {
        self.send_data(bytes, Opcode::Bin).await
    }

    async fn send_data(&self, bytes: &[u8], opcode: Opcode) -> Result {
        let f = DataFrame::<R>::new(bytes, opcode);
        for chunk in f.encode() {
            self.data_tx.send(chunk).await?;
        }
        Ok(())
    }

    pub async fn close(&mut self) -> Result { self.close_reason(CloseReason::Normal, "").await }

    // close with reason code and text
    pub async fn close_reason(&mut self, reason: CloseReason, text: &'static str) -> Result {
        if self.inner.closing.swap(true, Ordering::AcqRel) {
            return Ok(());
        }
        tracing::info!(reason = ?reason, "sending close");

        let code: [u8; 2] = reason.into();
        let mut payload = Vec::with_capacity(2 + text.len());
        payload.extend_from_slice(&code);
        payload.extend_from_slice(text.as_bytes());

        let bytes = ControlFrame::<R>::close(&payload).encode();
        self.close_tx.send(bytes).await
    }

    // send ping with a timestamp
    pub async fn ping(&self) -> Result {
        // send nonce as payload, store Instant
        let mut stats = self.inner.ping_stats.lock().await;
        let payload = stats.new_ping();
        let f = ControlFrame::<R>::ping(payload).encode();
        self.ctrl_tx.send(f).await
    }

    #[must_use]
    pub fn addr(&self) -> SocketAddr { self.addr }

    /// Average latency in ms from last 5 pings
    #[must_use]
    pub async fn latency(&self) -> Option<u16> { self.inner.ping_stats.lock().await.average() }

    pub async fn recv(&mut self) -> Option<Event> { self.event_rx.recv().await }

    pub async fn recv_timeout(&mut self, timeout: Duration) -> Option<Event> {
        tokio::time::timeout(timeout, self.event_rx.recv())
            .await
            .unwrap_or_default()
    }

    pub(crate) async fn writer_loop(
        mut close_rx: Receiver<Vec<u8>>,
        mut ctrl_rx: Receiver<Vec<u8>>,
        mut data_rx: Receiver<Vec<u8>>,
        mut writer: OwnedWriteHalf,
    ) -> tokio::io::Result<()> {
        loop {
            tokio::select! {
                biased;
                Some(close) = close_rx.recv() => writer.write_all(&close).await?,
                Some(ctrl) = ctrl_rx.recv() => writer.write_all(&ctrl).await?,
                Some(data) = data_rx.recv() => writer.write_all(&data).await?,
                else => break Ok(()),
            }
        }
    }

    pub(crate) fn ping_loop(
        &self,
        interval_secs: u64,
        ctrl_tx: Sender<Vec<u8>>,
        close_tx: Sender<Vec<u8>>,
        event_tx: Sender<Event>,
    ) {
        let inner = self.inner.clone();
        tokio::spawn(async move {
            let mut interval = interval(Duration::from_secs(10));
            let mut ping_sent = None;
            loop {
                interval.tick().await;

                if inner.closing.load(Ordering::Acquire) {
                    tracing::info!("socket closing, stopping ping loop");
                    break;
                }

                let last_seen = inner.last_seen.lock().await;
                if last_seen.elapsed() >= Duration::from_secs(interval_secs) && ping_sent.is_none()
                {
                    // send ping
                    let mut p = inner.ping_stats.lock().await;
                    let ping = ControlFrame::<R>::ping(p.new_ping());

                    if let Err(e) = ctrl_tx.send(ping.encode()).await {
                        tracing::warn!("Ping failed, stopping ping loop.");
                        let _ = event_tx.send(Event::Error(e)).await;
                        break;
                    }
                    ping_sent = Some(Instant::now());
                } else if let Some(sent) = ping_sent
                    && sent.elapsed() >= Duration::from_secs(interval_secs * 2)
                {
                    // send close
                    let _ = close_tx
                        .send(ControlFrame::<R>::close_reason(
                            CloseReason::Policy,
                            "ping timed out",
                        ))
                        .await;
                }

                tracing::info!("last seen within interval");
            }
        });
    }

    pub(crate) fn reader_loop(
        &self,
        mut reader: OwnedReadHalf,
        ctrl_tx: Sender<Vec<u8>>,
        close_tx: Sender<Vec<u8>>,
        event_tx: Sender<Event>,
    ) {
        let inner = self.inner.clone();
        tokio::spawn(async move {
            let mut buf = vec![0; MAX_FRAME_PAYLOAD];
            let mut partial_msg = None;

            let mut fd = FrameDecoder::<R>::new();
            loop {
                let n = {
                    match reader.read(&mut buf).await {
                        Ok(0) => {
                            tracing::info!("TCP FIN");
                            break;
                        }
                        Ok(n) => n,
                        Err(e) => {
                            tracing::warn!(error = ?e, "reader error");
                            break;
                        }
                    }
                };
                tracing::trace!(bytes = n, "read socket");
                *inner.last_seen.lock().await = Instant::now();

                fd.push_bytes(&buf[..n]);
                loop {
                    match fd.next_frame() {
                        Ok(Some(FrameState::Complete(frame))) => {
                            if handle_frame::<R>(
                                &frame,
                                &inner,
                                &mut partial_msg,
                                &close_tx,
                                &ctrl_tx,
                                &event_tx,
                            )
                            .await
                            .is_none()
                            {
                                // finished processing frames for now
                                // if the connection is closing, just read and discard until FIN
                                inner.closing.store(true, Ordering::Release);
                                break;
                            }
                        }

                        Ok(Some(FrameState::Incomplete)) => {
                            // break to read more bytes
                            break;
                        }
                        Ok(None) => {
                            // EOF
                            break;
                        }
                        Err(FrameParseError::ProtoError) => {
                            // close connection with ProtoError
                            tracing::warn!("protocol violation detected, entering closing state");
                            inner.closing.store(true, Ordering::Release);
                            let _ = close_tx
                                .send(ControlFrame::<R>::close_reason(
                                    CloseReason::ProtoError,
                                    "There was a ws protocol violation.",
                                ))
                                .await;
                            break;
                        }
                        Err(FrameParseError::SizeErr) => {
                            // close connection with TooBig
                            tracing::warn!("size error detected, entering closing state");
                            inner.closing.store(true, Ordering::Release);
                            let _ = close_tx
                                .send(ControlFrame::<R>::close_reason(
                                    CloseReason::TooBig,
                                    "Frame exceeded maximum size",
                                ))
                                .await;
                            break;
                        }
                    }
                }
            }
            inner.closing.store(true, Ordering::Release);
            inner.closed.store(true, Ordering::Release);
            let _ = event_tx.send(Event::Closed).await;
        });
    }
}

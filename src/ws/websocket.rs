use std::{
    collections::HashMap,
    marker::PhantomData,
    net::SocketAddr,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::{Duration, Instant},
};

use bytes::{Bytes, BytesMut};
use flate2::{
    Compression,
    write::{DeflateDecoder, DeflateEncoder},
};
use tokio::{
    io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt, ReadHalf, WriteHalf},
    sync::{
        Mutex,
        mpsc::{Receiver, Sender, channel, error::SendError},
    },
    time::interval,
};

use super::frame_handler::handle_frame;
use crate::{
    Event, MAX_FRAME_PAYLOAD, UpgradeError,
    error::CloseReason,
    frames::{ControlFrame, DataFrame, FrameDecoder, FrameParseError, FrameState, Opcode},
    protocol::PingStats,
    role::RolePolicy,
};

/// Generic WebSocket connection that applies masking according to role R, either client or server.
pub struct WebSocket<R: RolePolicy> {
    pub(crate) inner: Arc<Inner>,
    pub(crate) close_tx: Sender<Bytes>,
    pub(crate) ctrl_tx: Sender<Bytes>,
    pub(crate) data_tx: Sender<Bytes>,
    pub(crate) event_rx: Receiver<Event>,
    pub(crate) local_addr: SocketAddr,
    pub(crate) peer_addr: SocketAddr,
    pub(crate) deflater: Option<DeflateEncoder<Vec<u8>>>,
    pub(crate) use_context: bool,
    pub(crate) _role: PhantomData<R>,
}

pub(crate) struct Inner {
    pub(crate) ping_stats: Mutex<PingStats>,
    pub(crate) last_seen: Mutex<Instant>,
    pub(crate) closed: AtomicBool,
    pub(crate) closing: AtomicBool,
}

/// Message to be sent over the websocket.
pub enum Message {
    Text(Bytes),
    Binary(Bytes),
}

impl Message {
    #[must_use]
    pub fn text(s: &str) -> Self { Self::Text(Bytes::from(s.to_string())) }
}

#[async_trait::async_trait]
pub trait MessageHandler: Send + Sync + 'static {
    async fn on_text(&self, s: Bytes) -> Option<Message>;
    async fn on_binary(&self, b: Bytes) -> Option<Message>;
    async fn on_close(&self);
    async fn on_error(&self, e: Bytes);
    async fn on_pong(&self, latency: u16);
}

#[derive(Clone)]
pub(crate) struct WsSender {
    ctrl: Sender<Bytes>,
    close: Sender<Bytes>,
    event: Sender<Event>,
}

impl WsSender {
    pub fn new(ctrl: Sender<Bytes>, close: Sender<Bytes>, event: Sender<Event>) -> Self {
        Self { ctrl, close, event }
    }

    pub async fn ctrl(&self, data: Bytes) -> Result<Bytes> { self.ctrl.send(data).await }

    pub async fn close(&self, data: Bytes) -> Result<Bytes> { self.close.send(data).await }

    pub async fn event(&self, event: Event) -> Result<Event> { self.event.send(event).await }
}

/// Best-effort close if user forgets to call [`WebSocket::close`].
impl<R: RolePolicy> Drop for WebSocket<R> {
    fn drop(&mut self) { self.inner.closing.store(true, Ordering::Release); }
}

type Result<T> = std::result::Result<(), SendError<T>>;

impl<R: RolePolicy> WebSocket<R> {
    pub(crate) fn from_stream<S>(
        stream: S,
        local_addr: SocketAddr,
        peer_addr: SocketAddr,
        compressed: bool,
        use_context: bool,
    ) -> Self
    where
        S: AsyncRead + AsyncWrite + Send + 'static,
    {
        // create channels
        const CHAN_BUF: usize = 64;
        let (event_tx, event_rx) = channel(CHAN_BUF);
        let (close_tx, close_rx) = channel(CHAN_BUF);
        let (ctrl_tx, ctrl_rx) = channel(CHAN_BUF);
        let (data_tx, data_rx) = channel(CHAN_BUF);

        // create WebSocket struct
        let ws = Self {
            inner: Arc::new(Inner {
                ping_stats: Mutex::new(PingStats::new()),
                last_seen: Mutex::new(Instant::now()),
                closed: AtomicBool::new(false),
                closing: AtomicBool::new(false),
            }),
            close_tx: close_tx.clone(),
            ctrl_tx: ctrl_tx.clone(),
            data_tx,
            event_rx,
            local_addr,
            peer_addr,
            deflater: if compressed {
                Some(DeflateEncoder::new(vec![], Compression::fast()))
            } else {
                None
            },
            use_context,
            _role: PhantomData,
        };

        // initiate background loops
        let (reader, writer) = tokio::io::split(stream);
        let sender = WsSender::new(ctrl_tx, close_tx, event_tx);

        Self::writer_loop(close_rx, ctrl_rx, data_rx, writer);
        ws.ping_loop(30, sender.clone());
        ws.reader_loop(
            reader,
            sender,
            if compressed {
                Some(DeflateDecoder::new(vec![]))
            } else {
                None
            },
        );
        ws
    }

    /// Sends text to the connected endpoint.
    /// # Errors
    /// If the peer has disconnected or we are currently closing, this function returns an error.
    /// The error includes the value passed.
    pub async fn send_text(&mut self, text: &str) -> Result<Bytes> {
        self.send_data(text.as_bytes(), Opcode::Text).await
    }

    /// Sends bytes to the connected endpoint.
    /// # Errors
    /// If the peer has disconnected or we are currently closing, this function returns an error.
    pub async fn send_bytes(&mut self, bytes: &[u8]) -> Result<Bytes> {
        self.send_data(bytes, Opcode::Bin).await
    }

    async fn send_data(&mut self, bytes: &[u8], opcode: Opcode) -> Result<Bytes> {
        let f = DataFrame::<R>::new(bytes, opcode);
        let chunks = f.encode(&mut self.deflater, self.use_context);
        for chunk in chunks {
            self.data_tx.send(chunk).await?;
        }
        Ok(())
    }

    /// Request close from peer and close the connection.
    pub async fn close(&mut self) { self.close_reason(CloseReason::Normal, "").await; }

    async fn close_reason(&mut self, reason: CloseReason, text: &'static str) {
        if !self.inner.closing.swap(true, Ordering::AcqRel) {
            let _ = self
                .close_tx
                .send(ControlFrame::<R>::close_reason(reason, text))
                .await;
        }
    }

    /// Send a ping to the peer. The associated latency measurement will appear
    /// as an [`Event::Pong`].
    /// # Errors
    /// If the peer has disconnected or we are currently closing, this function returns an error.
    pub async fn ping(&self) -> Result<Bytes> {
        let nonce = self.inner.ping_stats.lock().await.new_nonce();
        let f = ControlFrame::<R>::ping(&nonce);
        self.ctrl_tx.send(f.encode()).await
    }

    /// Returns the peer socket address.
    #[must_use]
    pub fn peer_addr(&self) -> SocketAddr { self.peer_addr }

    /// Returns the local socket address.
    #[must_use]
    pub fn local_addr(&self) -> SocketAddr { self.local_addr }

    /// Returns the average latency in ms from last 5 pings
    #[must_use]
    pub async fn latency(&self) -> Option<u16> { self.inner.ping_stats.lock().await.average() }

    /// Wait for and return the next [`Event`].
    pub async fn recv(&mut self) -> Option<Event> { self.event_rx.recv().await }

    /// Wait for and return the next [`Event`] with a given timeout.
    pub async fn recv_timeout(&mut self, timeout: Duration) -> Option<Event> {
        tokio::time::timeout(timeout, self.event_rx.recv())
            .await
            .unwrap_or_default()
    }

    /// Start a recv loop which handles the events with a [`MessageHandler`]
    pub async fn recv_loop<H: MessageHandler>(&mut self, handler: Arc<H>) {
        // start a loop to handle events from this client
        while let Some(event) = self.event_rx.recv().await {
            match event {
                Event::Text(s) => {
                    self.handle_ws_message(handler.on_text(s).await).await;
                }
                Event::Binary(b) => {
                    self.handle_ws_message(handler.on_binary(b).await).await;
                }
                Event::Closed => {
                    handler.on_close().await;
                    break;
                }
                Event::Error(e) => handler.on_error(e).await,
                Event::Pong(latency) => handler.on_pong(latency).await,
            }
        }
    }

    async fn handle_ws_message(&mut self, msg: Option<Message>) {
        match msg {
            Some(Message::Text(s)) => {
                if let Err(e) = self.send_data(&s, Opcode::Text).await {
                    tracing::error!(e = ?e, "failed to send text message");
                }
            }
            Some(Message::Binary(b)) => {
                if let Err(e) = self.send_bytes(&b).await {
                    tracing::error!(e = ?e, "failed to send binary message");
                }
            }
            None => {}
        }
    }

    pub(crate) fn hash_key(s: &str) -> String {
        use base64::engine::{Engine, general_purpose::STANDARD as BASE64};
        use sha1::{Digest, Sha1};

        let mut hasher = Sha1::new();
        hasher.update(s.as_bytes());
        hasher.update("258EAFA5-E914-47DA-95CA-C5AB0DC85B11");
        BASE64.encode(hasher.finalize())
    }

    pub(crate) fn validate_header(
        headers: &HashMap<String, String>,
        field: &'static str,
        expected: &str,
    ) -> std::result::Result<(), UpgradeError> {
        let value = headers
            .get(field)
            .ok_or(UpgradeError::MissingHeader(field))?;

        if value.eq_ignore_ascii_case(expected) {
            Ok(())
        } else {
            Err(UpgradeError::Header {
                field,
                expected: expected.into(),
                got: value.into(),
            })
        }
    }

    pub(crate) fn writer_loop<S: AsyncWrite + Send + 'static>(
        mut close_rx: Receiver<Bytes>,
        mut ctrl_rx: Receiver<Bytes>,
        mut data_rx: Receiver<Bytes>,
        mut writer: WriteHalf<S>,
    ) {
        tokio::spawn(async move {
            loop {
                tokio::select! {
                    biased;
                     Some(close) = close_rx.recv() => {
                         let _ = writer.write_all(&close).await;
                         let _ = writer.flush().await;
                         if let Err(e) = writer.shutdown().await{
                             tracing::warn!(e = ?e, "stream shutdown");
                         }
                         tracing::trace!("TLS shutdown sent");
                         break;

                     }
                    Some(ctrl) = ctrl_rx.recv() => {
                        if writer.write_all(&ctrl).await.is_err()
                            || writer.flush().await.is_err() {
                            break;
                        }
                    }
                    Some(data) = data_rx.recv() => {
                        if writer.write_all(&data).await.is_err()
                            || writer.flush().await.is_err() {
                                break;
                        }
                    }
                    else => break
                }
            }
        });
    }

    pub(crate) fn ping_loop(&self, interval_secs: u64, sender: WsSender) {
        let inner = self.inner.clone();
        tokio::spawn(async move {
            let mut interval = interval(Duration::from_secs(10));
            let mut ping_sent = None;
            loop {
                interval.tick().await;

                if inner.closing.load(Ordering::Acquire) {
                    tracing::trace!("socket closing, stopping ping loop");
                    break;
                }

                let last_seen = inner.last_seen.lock().await;
                if last_seen.elapsed() >= Duration::from_secs(interval_secs) && ping_sent.is_none()
                {
                    // send ping
                    tracing::trace!("interval exceeded, sending ping");
                    let nonce = inner.ping_stats.lock().await.new_nonce();
                    let frame = ControlFrame::<R>::ping(&nonce).encode();

                    if let Err(e) = sender.ctrl(frame).await {
                        tracing::warn!("Ping failed, stopping ping loop.");
                        let _ = sender.event(Event::Error(e.0)).await;
                        break;
                    }
                    ping_sent = Some(Instant::now());
                } else if let Some(sent) = ping_sent
                    && sent.elapsed() >= Duration::from_secs(interval_secs * 2)
                {
                    // send close
                    let _ = sender
                        .close(ControlFrame::<R>::close_reason(
                            CloseReason::Policy,
                            "ping timed out",
                        ))
                        .await;
                }

                tracing::trace!("last seen within interval");
            }
        });
    }

    pub(crate) fn reader_loop<S: AsyncRead + Send + 'static>(
        &self,
        mut reader: ReadHalf<S>,
        sender: WsSender,
        mut inflater: Option<DeflateDecoder<Vec<u8>>>,
    ) {
        let inner = self.inner.clone();
        let use_context = self.use_context;

        tokio::spawn(async move {
            let mut buf = BytesMut::with_capacity(MAX_FRAME_PAYLOAD);
            let mut partial_msg = None;

            let mut fd = FrameDecoder::<R>::new(inflater.is_some());
            loop {
                let n = {
                    match reader.read_buf(&mut buf).await {
                        Ok(0) => {
                            tracing::trace!("TCP FIN");
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

                fd.push_bytes(&buf.split_to(n));
                loop {
                    match fd.next_frame() {
                        Ok(Some(FrameState::Complete(frame))) => {
                            if handle_frame::<R>(
                                &frame,
                                &inner,
                                &mut partial_msg,
                                &sender,
                                &mut inflater,
                                use_context,
                            )
                            .await
                            .is_none()
                            {
                                // connection closed, nothing to process anymore
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
                            let _ = sender
                                .close(ControlFrame::<R>::close_reason(
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
                            let _ = sender
                                .close(ControlFrame::<R>::close_reason(
                                    CloseReason::TooBig,
                                    "Frame exceeded maximum size",
                                ))
                                .await;
                            break;
                        }
                    }
                }
            }
            tracing::trace!("reading finished");
            inner.closing.store(true, Ordering::Release);
            inner.closed.store(true, Ordering::Release);
            let _ = sender.event(Event::Closed).await;
        });
    }
}

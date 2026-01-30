use std::{
    collections::HashMap,
    io::{BufRead, BufReader, Error, ErrorKind, Read, Result, Write},
    marker::PhantomData,
    net::{SocketAddr, TcpStream, ToSocketAddrs},
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
        mpsc::{Sender, channel},
    },
    thread,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use base64::engine::{Engine, general_purpose::STANDARD as BASE64};

use crate::{
    MAX_MESSAGE_SIZE,
    error::CloseReason,
    event::Event,
    frames::{ControlFrame, Frame, FrameDecoder, FrameParseResult, Opcode},
    inner::InnerTrait,
    message::{Message, PartialMessage},
    ping::PingStats,
    role::{Client, EncodePolicy},
    ws::WebSocket,
};

pub type WebSocketClient = WebSocket<ClientInner, Client>;

pub struct ClientInner {
    reader: Mutex<TcpStream>,
    writer: Mutex<TcpStream>,
    ping_stats: Mutex<PingStats<5>>,
    closing: AtomicBool,
    closed: AtomicBool,
}

impl InnerTrait<Client> for ClientInner {
    fn closing(&self) -> &AtomicBool { &self.closing }

    fn closed(&self) -> &AtomicBool { &self.closed }

    fn writer(&self) -> &Mutex<TcpStream> { &self.writer }
}

impl WebSocketClient {
    pub fn connect(addr: impl ToSocketAddrs) -> Result<Self> {
        TcpStream::connect(addr)?.try_into()
    }

    pub fn connect_timeout(addr: &SocketAddr, timeout: Duration) -> Result<Self> {
        TcpStream::connect_timeout(addr, timeout)?.try_into()
    }

    #[must_use]
    pub fn latency(&self) -> Option<Duration> { self.inner.ping_stats.lock().unwrap().average() }

    /// Start a ping loop in a background thread
    fn start_ping_loop(&self, interval_secs: u64, event_tx: Sender<Event>) {
        let inner = self.inner.clone();
        let interval = Duration::from_secs(interval_secs);
        thread::spawn(move || {
            loop {
                if inner.closing.load(Ordering::Acquire) {
                    println!("INFO: socket closing, stopping ping loop");
                    break;
                }
                if let Err(e) = inner.ping() {
                    eprintln!("ERR: Ping failed, stopping ping loop.");
                    let _ = event_tx.send(Event::Error(e));
                    break;
                }
                thread::sleep(interval);
            }
        });
    }

    fn start_recv_loop(&self, event_tx: Sender<Event>) {
        let inner = Arc::clone(&self.inner);
        thread::spawn(move || {
            let mut buf = [0u8; 2048];
            let mut partial_msg = None;

            let mut fd = FrameDecoder::<Client>::new();

            loop {
                let n = {
                    match inner.reader.lock().unwrap().read(&mut buf) {
                        Ok(0) => {
                            println!("TCP FIN");
                            break;
                        }
                        Err(e) => {
                            eprintln!("Err: {e}");
                            break;
                        }
                        Ok(n) => n,
                    }
                };

                fd.push_bytes(&buf[..n]);
                while let Some(result) = fd.next_frame() {
                    match result {
                        FrameParseResult::Complete(frame) => {
                            if handle_frame::<Client>(frame, &inner, &mut partial_msg, &event_tx)
                                .is_none()
                            {
                                // finished processing frames for now
                                // if the connection is closing, just read and discard until FIN
                                inner.closing.store(true, Ordering::Release);
                                break;
                            }
                        }
                        FrameParseResult::Incomplete => {
                            // break while to read more bytes
                            break;
                        }
                        FrameParseResult::ProtoError => {
                            // close connection with ProtoError
                            let _ = inner.close(
                                CloseReason::ProtoError,
                                "There was a ws protocol violation.",
                            );
                            break;
                        }
                        FrameParseResult::SizeErr => {
                            // close connection with TooBig
                            let _ = inner.close(CloseReason::TooBig, "Frame exceeded maximum size");
                            break;
                        }
                    }
                }
            }
            inner.closing.store(true, Ordering::Release);
            inner.closed.store(true, Ordering::Release);
            let _ = event_tx.send(Event::Closed);
        });
    }
}

fn handle_frame<P: EncodePolicy>(
    frame: Frame,
    inner: &Arc<ClientInner>,
    partial_msg: &mut Option<PartialMessage>,
    event_tx: &Sender<Event>,
) -> Option<()> {
    match frame.opcode {
        // If unsolicited, ignore; otherwise, parse timestamp and calculate latency
        Opcode::Pong => {
            // try to parse payload as timestamp,
            // otherwise its unsolicited and we ignore
            if let Ok(bytes) = frame.payload.try_into() {
                let sent = u128::from_be_bytes(bytes);
                let now = SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .unwrap()
                    .as_millis();

                // assume anomalous if latency > u64::MAX
                if let Ok(latency) = u64::try_from(now - sent) {
                    inner
                        .ping_stats
                        .lock()
                        .unwrap()
                        .add(Duration::from_millis(latency));

                    let _ = event_tx.send(Event::Pong(latency));
                }
            }
        }
        // Reply with pong
        Opcode::Ping => {
            let bytes = ControlFrame::<P>::pong(&frame.payload).encode();
            let mut ws = inner.writer.lock().unwrap();
            let _ = ws.write_all(&bytes);
        }
        // Build message out of frames
        Opcode::Text | Opcode::Bin | Opcode::Cont => {
            // println!("{frame:?}");
            // TODO: Leniency
            // allow overwriting partial messages
            // if we get a new TEXT or BINARY
            let partial = match (partial_msg.as_mut(), frame.opcode) {
                (None, Opcode::Text) => partial_msg.insert(PartialMessage::Text(vec![])),
                (None, Opcode::Bin) => partial_msg.insert(PartialMessage::Binary(vec![])),
                (Some(p), Opcode::Cont) => p,
                _ => {
                    // if we get a CONT before TEXT or BINARY
                    // or we get TEXT/BINARY without finishing the last message
                    // close the connection
                    let _ = inner.close(CloseReason::ProtoError, "Unexpected frame");
                    return None;
                }
            };

            if partial.len() + frame.payload.len() > MAX_MESSAGE_SIZE {
                let _ = inner.close(CloseReason::TooBig, "Message exceeded maximum size");
                return None;
            }

            partial.push_bytes(&frame.payload);

            if frame.is_fin {
                let msg = match partial_msg.take().unwrap() {
                    PartialMessage::Binary(buf) => Message::Binary(buf),
                    PartialMessage::Text(buf) => {
                        // if invalid UTF-8 immediately close connection
                        if let Ok(s) = String::from_utf8(buf) {
                            Message::Text(s)
                        } else {
                            let _ = inner.close(CloseReason::DataError, "Invalid UTF-8");
                            return None;
                        }
                    }
                };
                let _ = event_tx.send(Event::Message(msg));
            }
        }
        // If closing, shutdown; otherwise, reply with close frame
        Opcode::Close => {
            println!("got close! {frame:?}");
            // if not already closing try to send close frame, log err
            if !inner.closing.swap(true, Ordering::AcqRel)
                && let Err(e) = inner.close_raw(&frame.payload)
            {
                eprintln!("ERR: error sending close: {e}");
                let _ = event_tx.send(Event::Error(e));
            }

            return None;
        }
    }
    Some(())
}

/// Takes a [`TcpStream`] and attempts to upgrade the connection to WS.
impl TryFrom<TcpStream> for WebSocketClient {
    /// Returns [`std::io::Error`] if unable to read from or write to the [`TcpStream`], or if there was a handshake failure whilst upgrading the connection.
    type Error = Error;

    fn try_from(mut stream: TcpStream) -> std::result::Result<Self, Self::Error> {
        let sec_websocket_key = {
            let mut key_bytes = [0u8; 16];
            rand::fill(&mut key_bytes);
            BASE64.encode(key_bytes)
        };

        let req = format!(
            "GET / HTTP/1.1\r\n\
            Host: {}\r\n\
            Upgrade: websocket\r\n\
            Connection: Upgrade\r\n\
            Sec-WebSocket-Key: {sec_websocket_key}\r\n\
            Sec-WebSocket-Version: 13\r\n\r\n",
            stream.peer_addr()?
        );
        // send upgrade request
        stream.write_all(req.as_bytes())?;

        // get status line and validate status code
        let mut reader = BufReader::new(stream);
        let mut status_line = String::new();
        reader.read_line(&mut status_line)?;

        let mut status_parts = status_line.split_whitespace();
        if status_parts.next().is_none() || status_parts.next() != Some("101") {
            return Err(Error::new(
                ErrorKind::InvalidData,
                format!("Handshake failed: invalid status from server: {status_line}"),
            ));
        }

        // collect headers in a hashmap
        let mut headers = HashMap::new();
        loop {
            let mut line = String::new();
            reader.read_line(&mut line)?;
            let line = line.trim_end(); // remove \r\n
            if line.is_empty() {
                break;
            } // end of headers

            if let Some((name, value)) = line.split_once(':') {
                headers.insert(name.trim().to_string(), value.trim().to_string());
            }
        }

        // validate Upgrade: websocket
        match headers.get("Upgrade").map(|v| v.to_lowercase()) {
            Some(x) if x == "websocket" => {}
            r => {
                return Err(Error::new(
                    ErrorKind::InvalidData,
                    format!("Handshake failed: missing Upgrade header: {r:?}"),
                ));
            }
        }

        // validate Connection: upgrade
        match headers.get("Connection").map(|v| v.to_lowercase()) {
            Some(x) if x == "upgrade" => {}
            r => {
                return Err(Error::new(
                    ErrorKind::InvalidData,
                    format!("Handshake failed: missing Connection header: {r:?}"),
                ));
            }
        }

        // validate key was processed properly
        let expected_accept = {
            use sha1::{Digest, Sha1};
            let mut hasher = Sha1::new();
            hasher.update(sec_websocket_key);
            hasher.update("258EAFA5-E914-47DA-95CA-C5AB0DC85B11");
            BASE64.encode(hasher.finalize())
        };

        match headers.get("Sec-WebSocket-Accept") {
            Some(x) if x == &expected_accept => {}
            r => {
                return Err(Error::new(
                    ErrorKind::InvalidData,
                    format!("Handshake failed: Sec-WebSocket-Accept mismatch: {r:?}",),
                ));
            }
        }

        let (event_tx, event_rx) = channel();
        let stream = reader.into_inner();
        let ws = WebSocket {
            inner: Arc::new(ClientInner {
                reader: Mutex::new(stream.try_clone()?),
                writer: Mutex::new(stream),
                ping_stats: Mutex::new(PingStats::new()),
                closing: AtomicBool::new(false),
                closed: AtomicBool::new(false),
            }),
            event_rx,
            _p: PhantomData,
        };
        ws.start_recv_loop(event_tx.clone());
        ws.start_ping_loop(30, event_tx.clone());
        Ok(ws)
    }
}

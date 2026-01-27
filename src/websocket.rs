use std::{
    collections::HashMap,
    io::{BufRead, BufReader, Error, ErrorKind, Read, Result, Write},
    net::{Shutdown, SocketAddr, TcpStream, ToSocketAddrs},
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
        mpsc::{Receiver, Sender, channel},
    },
    thread,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use base64::engine::{Engine, general_purpose::STANDARD as BASE64};

use crate::{
    error::CloseReason,
    event::Event,
    frames::{ControlFrame, DataFrame, Frame, FrameParseResult, Opcode},
    message::{Message, PartialMessage},
    ping::PingStats,
    role::Role,
};

/// WebSocket
pub struct WebSocket {
    inner: Arc<Inner>,
    event_rx: Receiver<Event>,
}

// TODO: select client/server
// potentially Client and Server wrappers
impl WebSocket {
    pub fn connect<A: ToSocketAddrs>(addr: A) -> Result<Self> {
        TcpStream::connect(addr)?.try_into()
    }

    pub fn connect_timeout(addr: &SocketAddr, timeout: Duration) -> Result<Self> {
        TcpStream::connect_timeout(addr, timeout)?.try_into()
    }

    // # Safety
    // this ignores the handshake and assumes the stream is upgraded already
    // # Errors
    // Will fail if cloning the stream fails
    // pub unsafe fn from_raw_stream(stream: TcpStream, role: Role) -> Result<Self> {
    //     let (_tx, rx) = channel();
    //     Ok(Self {
    //         inner: Arc::new(Inner {
    //             closed: AtomicBool::new(false),
    //             closing: AtomicBool::new(false),
    //             ping_stats: Mutex::new(PingStats::new()),
    //             reader: Mutex::new(stream.try_clone()?),
    //             writer: Mutex::new(stream),
    //             role,
    //         }),
    //         event_rx: rx,
    //     })
    // }

    /// Closes the connection.
    ///
    /// **NB**: the WS instance can no longer be used after calling this method.
    pub fn close(&mut self) -> Result<()> {
        if self.inner.closing.swap(true, Ordering::AcqRel) {
            return Ok(());
        }
        let payload: [u8; 2] = CloseReason::Normal.into();
        self.inner.close(&payload)
    }

    /// Sends a ping to the server.
    ///
    /// **NB**: [`WebSocket::latency`] will only update when the corresponding pong arrives.
    pub fn ping(&self) -> Result<()> { self.inner.ping() }

    pub fn recv(&mut self) -> Option<Event> { self.event_rx.recv().ok() }

    pub fn recv_timeout(&mut self, timeout: Duration) -> Option<Event> {
        self.event_rx.recv_timeout(timeout).ok()
    }

    /// Returns the server latency as a [`Duration`] representing the average of the last 5 latencies calculated from pings and corresponding pongs from the server.
    ///
    /// This may return `None` if no pongs have been received yet.
    #[must_use]
    pub fn latency(&self) -> Option<Duration> { self.inner.ping_stats.lock().unwrap().average() }

    pub fn send_text(&self, text: &str) -> Result<()> {
        self.inner.send(text.as_bytes(), Opcode::Text)
    }

    pub fn send_bytes(&mut self, bytes: &[u8]) -> Result<()> { self.inner.send(bytes, Opcode::Bin) }

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

            loop {
                let n = {
                    match inner.reader.lock().unwrap().read(&mut buf) {
                        Ok(0) | Err(_) => break, // connection closed or error
                        Ok(n) => n,
                    }
                };
                for result in Frame::parse_bytes(&buf[..n], inner.role) {
                    match result {
                        FrameParseResult::Complete(frame) => {
                            if handle_frame(frame, &inner, &mut partial_msg, &event_tx).is_none() {
                                return;
                            }
                        }
                        FrameParseResult::Incomplete => {
                            println!("incomplete frame");
                        }
                        FrameParseResult::ProtocolError(reason) => {
                            let payload: [u8; 2] = reason.into();
                            let _ = inner.close(&payload);
                            break;
                        }
                    }
                }
            }
        });
    }
}

fn handle_frame(
    frame: Frame,
    inner: &Arc<Inner>,
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
            let bytes = ControlFrame::pong(&frame.payload, inner.role).encode();
            let mut ws = inner.writer.lock().unwrap();
            let _ = ws.write_all(&bytes);
        }
        // Build message out of frames
        Opcode::Text | Opcode::Bin | Opcode::Cont => {
            // println!("{frame:?}");
            let partial = match (partial_msg.as_mut(), frame.opcode) {
                (None, Opcode::Text) => partial_msg.insert(PartialMessage::Text(vec![])),
                (None, Opcode::Bin) => partial_msg.insert(PartialMessage::Binary(vec![])),
                (Some(p), Opcode::Cont) => p,
                _ => {
                    // if we get a CONT before TEXT or BINARY
                    // or we get TEXT/BINARY without finishing the last message
                    // close the connection
                    let payload: [u8; 2] = CloseReason::ProtoError.into();
                    let _ = inner.close(&payload);
                    return None;
                }
            };

            partial.push_bytes(&frame.payload);

            if frame.is_fin {
                let msg = match partial_msg.take().unwrap() {
                    PartialMessage::Binary(buf) => Message::Binary(buf),
                    PartialMessage::Text(buf) => {
                        // if invalid UTF-8 immediately close connection
                        if let Ok(s) = String::from_utf8(buf) {
                            Message::Text(s)
                        } else {
                            let payload: [u8; 2] = CloseReason::DataError.into();
                            let _ = inner.close(&payload);
                            return None;
                        }
                    }
                };
                let _ = event_tx.send(Event::Message(msg));
            }
        }
        // If closing, shutdown; otherwise, reply with close frame
        Opcode::Close => {
            // if not already closing try to send close frame, log err
            // TODO: make sure close payload is valid
            if !inner.closing.swap(true, Ordering::AcqRel)
                && let Err(e) = inner.close(&frame.payload)
            {
                eprintln!("ERR: error sending close: {e}");
                let _ = event_tx.send(Event::Error(e));
            }

            // request TCP shutdown if still open, ignore errors
            let mut stream = inner.writer.lock().unwrap();
            let _ = stream.flush();
            let _ = stream.shutdown(Shutdown::Both);
            inner.closed.store(true, Ordering::Release);
            let _ = event_tx.send(Event::Closed);
            return None;
        }
    }
    Some(())
}

struct Inner {
    reader: Mutex<TcpStream>,
    writer: Mutex<TcpStream>,
    ping_stats: Mutex<PingStats<5>>,
    closing: AtomicBool,
    closed: AtomicBool,
    role: Role,
}

impl Inner {
    // send data (bytes) over the websocket
    fn send(&self, bytes: &[u8], ty: Opcode) -> Result<()> {
        let frame = DataFrame::new(bytes, ty, self.role);
        let mut ws = self.writer.lock().unwrap();
        for chunk in frame.encode() {
            ws.write_all(&chunk)?;
        }
        ws.flush()
    }

    // send close request
    fn close(&self, payload: &[u8]) -> Result<()> {
        let bytes = ControlFrame::close(payload, self.role).encode();
        let mut ws = self.writer.lock().unwrap();
        ws.write_all(&bytes)?;
        ws.flush()
    }

    // send ping with a timestamp
    fn ping(&self) -> Result<()> {
        let mut ws = self.writer.lock().unwrap();

        let timestamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_millis();

        let bytes = ControlFrame::ping(&timestamp.to_be_bytes(), self.role).encode();
        ws.write_all(&bytes)?;
        ws.flush()
    }
}

/// Best-effort close if user forgets to call [`WebSocket::close`].
impl Drop for WebSocket {
    fn drop(&mut self) {
        if !self.inner.closing.load(Ordering::Acquire) {
            let _ = self.close();
        }
    }
}

/// Takes a [`TcpStream`] and attempts to upgrade the connection to WS.
impl TryFrom<TcpStream> for WebSocket {
    /// Returns [`std::io::Error`] if unable to read from or write to the [`TcpStream`], or if there was a handshake failure whilst upgrading the connection.
    type Error = std::io::Error;

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
            inner: Arc::new(Inner {
                reader: Mutex::new(stream.try_clone()?),
                writer: Mutex::new(stream),
                ping_stats: Mutex::new(PingStats::new()),
                closing: AtomicBool::new(false),
                closed: AtomicBool::new(false),
                role: Role::Client,
            }),
            event_rx,
        };
        ws.start_recv_loop(event_tx.clone());
        ws.start_ping_loop(30, event_tx.clone());
        Ok(ws)
    }
}

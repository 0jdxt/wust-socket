use base64::engine::{general_purpose::STANDARD as BASE64, Engine};

use std::collections::HashMap;
use std::io::{BufRead, BufReader, Error, ErrorKind, Read, Result, Write};
use std::net::{SocketAddr, TcpStream, ToSocketAddrs};
use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};
use std::time::{SystemTime, UNIX_EPOCH};

use crate::{
    structs::{ControlFrame, Frame, Message, MessageType, Opcode, SendMessage},
    Close,
};

/// WebSocket
pub struct WebSocket {
    reader: TcpStream,
    writer: Arc<Mutex<TcpStream>>,
    ping_stats: Arc<Mutex<PingStats<5>>>,
    event_rx: Receiver<Message>,
}

impl WebSocket {
    /// Opens a WebSocket connection to a remote host.
    ///
    /// `addr` is an address of the remote host. Anything which implements [`ToSocketAddrs`] trait can be supplied for the address; see this trait documentation for concrete examples.
    ///
    /// If `addr` yields multiple addresses, `connect` will be attempted with each of the addresses until a connection is successful. If none of the addresses result in a successful connection, the error returned from the last connection attempt (the last address) is returned.
    ///
    /// # Examples
    ///
    /// Open a WS connection to `127.0.0.1:8080`:
    ///
    /// ```no_run
    /// use wust_socket::WebSocket;
    ///
    /// if let Ok(stream) = WebSocket::connect("127.0.0.1:8080") {
    ///     println!("Connected to the server!");
    /// } else {
    ///     println!("Couldn't connect to server...");
    /// }
    /// ```
    ///
    /// Open a WS connection to `127.0.0.1:8080`. If the connection fails, open
    /// a WS connection to `127.0.0.1:8081`:
    ///
    /// ```no_run
    /// use std::net::SocketAddr;
    /// use wust_socket::WebSocket;
    ///
    /// let addrs = [
    ///     SocketAddr::from(([127, 0, 0, 1], 8080)),
    ///     SocketAddr::from(([127, 0, 0, 1], 8081)),
    /// ];
    /// if let Ok(stream) = WebSocket::connect(&addrs[..]) {
    ///     println!("Connected to the server!");
    /// } else {
    ///     println!("Couldn't connect to server...");
    /// }
    /// ```
    pub fn connect<A: ToSocketAddrs>(addr: A) -> Result<Self> {
        Self::from_tcp(TcpStream::connect(addr)?)
    }

    /// Opens a WS connection to a remote host with a timeout.
    ///
    /// Unlike `connect`, `connect_timeout` takes a single [`SocketAddr`] since timeout must be applied to individual addresses.
    ///
    /// It is an error to pass a zero `Duration` to this function.
    ///
    pub fn connect_timeout(addr: &SocketAddr, timeout: Duration) -> Result<Self> {
        Self::from_tcp(TcpStream::connect_timeout(addr, timeout)?)
    }

    fn from_tcp(mut stream: TcpStream) -> Result<Self> {
        let sec_websocket_key = {
            let mut key_bytes = [0u8; 16];
            rand::fill(&mut key_bytes);
            BASE64.encode(key_bytes)
        };

        let server = stream.peer_addr().unwrap();

        let req = format!(
            "GET / HTTP/1.1\r\n\
            Host: {server}\r\n\
            Upgrade: websocket\r\n\
            Connection: Upgrade\r\n\
            Sec-WebSocket-Key: {sec_websocket_key}\r\n\
            Sec-WebSocket-Version: 13\r\n\r\n"
        );

        // send upgrade request
        stream.write_all(req.as_bytes())?;

        let mut reader = BufReader::new(stream);

        // get status line and validate status code
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
        let mut ws = WebSocket {
            reader: stream.try_clone().unwrap(),
            writer: Arc::new(Mutex::new(stream)),
            ping_stats: Arc::new(Mutex::new(PingStats::new())),
            event_rx,
        };
        ws.start_ping_loop(30);
        ws.start_recv_loop(event_tx);
        Ok(ws)
    }

    /// Closes the connection.
    ///
    /// **NB**: the WS instance can no longer be used after calling this method.
    pub fn close(self) -> Result<()> {
        let mut ws = self.writer.lock().unwrap();
        ControlFrame::close(&Close::NORMAL).send(&mut ws)
    }

    /// Sends a ping to the server.
    ///
    /// **NB**: [`WebSocket::latency`] will only update when the corresponding pong arrives.
    pub fn ping(&self) -> Result<()> {
        let now = Instant::now();
        let mut ws = self.writer.lock().unwrap();
        ControlFrame::ping(&now.elapsed().as_millis().to_be_bytes()).send(&mut ws)
    }

    pub fn recv(&mut self) -> Option<Message> {
        self.event_rx.recv().ok()
    }

    pub fn recv_timeout(&mut self, timeout: Duration) -> Option<Message> {
        self.event_rx.recv_timeout(timeout).ok()
    }

    /// Returns the server latency as a [`Duration`] representing the average of the last 5 latencies calculated from pings and corresponding pongs from the server.
    ///
    /// This may return `None` if no pongs have been received yet.
    pub fn latency(&self) -> Option<Duration> {
        self.ping_stats.lock().unwrap().average()
    }

    pub fn send_text(&mut self, text: &str) -> Result<()> {
        self.send(text.as_bytes(), Opcode::Text)
    }

    pub fn send_bytes(&mut self, bytes: &[u8]) -> Result<()> {
        self.send(bytes, Opcode::Bin)
    }

    fn send(&mut self, bytes: &[u8], ty: Opcode) -> Result<()> {
        let mut ws = self.writer.lock().unwrap();
        let msg = SendMessage::new(bytes, ty);
        msg.send(&mut ws)
    }

    /// Start a ping loop in a background thread
    fn start_ping_loop(&self, interval_secs: u64) {
        let arc = Arc::clone(&self.writer);
        thread::spawn(move || {
            let interval = Duration::from_secs(interval_secs);
            loop {
                let now = Instant::now();

                // lock the WebSocket to send ping
                let result = {
                    let mut ws = arc.lock().unwrap();

                    let now = SystemTime::now()
                        .duration_since(UNIX_EPOCH)
                        .unwrap()
                        .as_millis() as u64;

                    ControlFrame::ping(&now.to_be_bytes()).send(&mut ws)
                };

                if result.is_err() {
                    eprintln!("Ping failed, stopping ping loop.");
                    break;
                }

                // sleep until next interval
                let elapsed = now.elapsed();
                if elapsed < interval {
                    thread::sleep(interval - elapsed);
                }
            }
        });
    }

    fn start_recv_loop(&mut self, event_tx: Sender<Message>) {
        let ping = Arc::clone(&self.ping_stats);
        let mut reader = self.reader.try_clone().unwrap();
        thread::spawn(move || {
            let mut buf = [0u8; 4096];
            // for assembling fragmented messages
            let mut data_buf = Vec::new();
            let mut message_type = MessageType::Text;

            loop {
                let n = {
                    match reader.read(&mut buf) {
                        Ok(0) | Err(_) => break, // connection closed or error
                        Ok(n) => n,
                    }
                };

                for frame in Frame::parse_bytes(&buf[..n]) {
                    match frame.opcode {
                        Opcode::Pong => {
                            let sent = u64::from_be_bytes(frame.payload.try_into().unwrap());

                            let now = SystemTime::now()
                                .duration_since(UNIX_EPOCH)
                                .unwrap()
                                .as_millis() as u64;

                            let latency = now - sent;
                            let mut ps = ping.lock().unwrap();
                            ps.add(Duration::from_millis(latency));

                            // TODO: Event struct to send to user
                            //event_tx.send(Event::Pong(latency)).unwrap();
                        }
                        Opcode::Ping => {
                            // TODO: handle unsent pongs
                            let _ = ControlFrame::pong(&frame.payload).send(&mut reader);
                        }
                        Opcode::Text | Opcode::Bin | Opcode::Cont => {
                            // println!("{frame:?}");

                            data_buf.extend_from_slice(&frame.payload);

                            match frame.opcode {
                                Opcode::Bin => message_type = MessageType::Binary,
                                Opcode::Text => message_type = MessageType::Text,
                                _ => {}
                            };

                            if frame.is_fin {
                                // reassemble message
                                let msg = Message {
                                    message_type,
                                    payload: data_buf.clone(),
                                };
                                event_tx.send(msg).unwrap();
                                data_buf.clear();
                            }
                        }
                        Opcode::Close => return,
                    }
                }
            }
        });
    }
}

/// Takes a [`TcpStream`] and attempts to upgrade the connection to WS.
impl TryFrom<TcpStream> for WebSocket {
    /// Returns [`std::io::Error`] if unable to read from or write to the [`TcpStream`], or if there was a handshake failure whilst upgrading the connection.
    type Error = std::io::Error;

    fn try_from(stream: TcpStream) -> std::result::Result<Self, Self::Error> {
        Self::from_tcp(stream)
    }
}

struct PingStats<const N: usize> {
    history: [Option<Duration>; N],
    idx: usize,
}

impl<const N: usize> PingStats<N> {
    fn new() -> Self {
        Self {
            history: [None; N],
            idx: 0,
        }
    }

    fn add(&mut self, rtt: Duration) {
        self.history[self.idx] = Some(rtt);
        self.idx = (self.idx + 1) % N;
    }

    fn average(&self) -> Option<Duration> {
        let mut sum = Duration::ZERO;
        let mut count = 0;
        for &x in &self.history {
            if let Some(v) = x {
                sum += v;
                count += 1;
            }
        }
        if count > 0 {
            Some(sum / count)
        } else {
            None
        }
    }
}

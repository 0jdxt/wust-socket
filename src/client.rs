use std::{
    collections::HashMap,
    io::{BufRead, BufReader, Write},
    marker::PhantomData,
    net::{SocketAddr, TcpStream, ToSocketAddrs},
    sync::{atomic::AtomicBool, mpsc::channel, Arc, Mutex},
    time::Duration,
};

use base64::engine::{general_purpose::STANDARD as BASE64, Engine};

use crate::{
    protocol::PingStats,
    role::Client,
    ws::{ConnInner, WebSocket},
};

type Result<T> = std::result::Result<T, UpgradeError>;

pub type WebSocketClient = WebSocket<Client>;

/// Client connection implementation for WebSocket
impl WebSocketClient {
    /// Attempts to connect to socket and upgrade connection.
    pub fn connect(addr: impl ToSocketAddrs) -> Result<Self> {
        TcpStream::connect(addr)
            .map_err(|_| UpgradeError::Connect)?
            .try_into()
    }

    /// Attempts to connect to socket and upgrade connection, with timeout.
    pub fn connect_timeout(addr: &SocketAddr, timeout: Duration) -> Result<Self> {
        TcpStream::connect_timeout(addr, timeout)
            .map_err(|_| UpgradeError::Connect)?
            .try_into()
    }
}

/// Errors that can occur when upgrading a TCP stream to a WebSocket.
#[derive(Debug)]
pub enum UpgradeError {
    /// Failed to read from the TCP stream.
    Read,
    /// Failed to write to the TCP steam.
    Write,
    /// Server returned an unexpected HTTP status line.
    StatusLine(String),
    /// A handshake header did not match expectations.
    Header {
        /// The name of the header field.
        field: &'static str,
        /// The expected value.
        expected: String,
        /// The actual value, if any.
        got: Option<String>,
    },
    /// Failed to obtain the socket's peer address.
    ///
    /// This can happen if the socket is not connected, has been closed,
    /// or the underlying OS socket is invalid.
    Addr,
    /// Failed to establish TCP connection.
    Connect,
}

/// Takes a [`TcpStream`] and attempts to upgrade the connection to WS.
impl TryFrom<TcpStream> for WebSocketClient {
    /// Returns [`std::io::Error`] if unable to read from or write to the [`TcpStream`], or if there was a handshake failure whilst upgrading the connection.
    type Error = UpgradeError;

    fn try_from(mut stream: TcpStream) -> Result<Self> {
        // store peer_addr
        let addr = stream.peer_addr().unwrap();

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
            stream.peer_addr().map_err(|_| UpgradeError::Addr)?
        );
        // send upgrade request
        stream
            .write_all(req.as_bytes())
            .map_err(|_| UpgradeError::Write)?;

        // get status line and validate status code
        let mut reader = BufReader::new(stream);
        let mut status_line = String::new();
        reader
            .read_line(&mut status_line)
            .map_err(|_| UpgradeError::Read)?;

        let mut status_parts = status_line.split_whitespace();
        if status_parts.next().is_none() || status_parts.next() != Some("101") {
            return Err(UpgradeError::StatusLine(status_line));
        }

        // collect headers in a hashmap
        let mut headers = HashMap::new();
        loop {
            let mut line = String::new();
            reader
                .read_line(&mut line)
                .map_err(|_| UpgradeError::Read)?;
            let line = line.trim_end(); // remove \r\n
            if line.is_empty() {
                break;
            } // end of headers

            if let Some((name, value)) = line.split_once(':') {
                headers.insert(name.trim().to_string(), value.trim().to_string());
            }
        }

        validate_header(&headers, "Upgrade", "websocket")?;
        validate_header(&headers, "Connection", "upgrade")?;

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
                return Err(UpgradeError::Header {
                    field: "Sec-WebSocket-Accept",
                    expected: expected_accept,
                    got: r.cloned(),
                });
            }
        }

        tracing::info!(addr = ?addr, "successfully connected to peer");

        let (event_tx, event_rx) = channel();
        let stream = reader.into_inner();
        let ws = WebSocket {
            inner: Arc::new(ConnInner {
                reader: Mutex::new(stream.try_clone().map_err(|_| UpgradeError::Read)?),
                writer: Mutex::new(stream),
                ping_stats: Mutex::new(PingStats::new()),
                closing: AtomicBool::new(false),
                closed: AtomicBool::new(false),
                _role: PhantomData,
            }),
            event_rx,
        };
        ws.recv_loop(event_tx.clone());
        ws.ping_loop(30, event_tx.clone());
        Ok(ws)
    }
}

fn validate_header(
    headers: &HashMap<String, String>,
    field: &'static str,
    expected: &str,
) -> std::result::Result<(), UpgradeError> {
    match headers.get(field).map(|c| c.to_lowercase()) {
        Some(x) if x == expected => {}
        r => {
            return Err(UpgradeError::Header {
                field,
                expected: expected.into(),
                got: r,
            });
        }
    }
    Ok(())
}

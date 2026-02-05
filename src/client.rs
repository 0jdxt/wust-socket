use std::{collections::HashMap, net::SocketAddr, time::Duration};

use base64::engine::{Engine, general_purpose::STANDARD as BASE64};
use tokio::{
    io::{AsyncBufReadExt, AsyncWriteExt, BufReader},
    net::{TcpStream, ToSocketAddrs},
};

use crate::{role::Client, ws::WebSocket};

type Result<T> = std::result::Result<T, UpgradeError>;

pub type WebSocketClient = WebSocket<Client>;

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
    /// Attempt to connect timed out.
    Timeout,
}

/// Client connection implementation for WebSocket
impl WebSocketClient {
    /// Attempts to connect to socket and upgrade connection.
    pub async fn connect(addr: impl ToSocketAddrs) -> Result<Self> {
        Self::try_upgrade(
            TcpStream::connect(addr)
                .await
                .map_err(|_| UpgradeError::Connect)?,
        )
        .await
    }

    /// Attempts to connect to socket and upgrade connection, with timeout.
    pub async fn connect_timeout(addr: &SocketAddr, timeout: Duration) -> Result<Self> {
        let fut = Self::connect(addr);
        match tokio::time::timeout(timeout, fut).await {
            Ok(Ok(s)) => Ok(s),
            _ => Err(UpgradeError::Timeout),
        }
    }

    async fn try_upgrade(mut stream: TcpStream) -> Result<Self> {
        // store peer_addr
        let addr = stream.peer_addr().map_err(|_| UpgradeError::Addr)?;

        let sec_websocket_key = {
            let mut key_bytes = [0u8; 16];
            rand::fill(&mut key_bytes);
            BASE64.encode(key_bytes)
        };

        let req = format!(
            "GET / HTTP/1.1\r\n\
            Host: {addr}\r\n\
            Upgrade: websocket\r\n\
            Connection: Upgrade\r\n\
            Sec-WebSocket-Key: {sec_websocket_key}\r\n\
            Sec-WebSocket-Version: 13\r\n\r\n",
        );
        // send upgrade request
        stream
            .write_all(req.as_bytes())
            .await
            .map_err(|_| UpgradeError::Write)?;

        // get status line and validate status code
        let mut reader = BufReader::new(stream);
        let mut status_line = String::new();
        reader
            .read_line(&mut status_line)
            .await
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
                .await
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

        let ws = Self::from_stream(reader.into_inner());
        tracing::info!(addr = ?addr, "successfully connected to peer");
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

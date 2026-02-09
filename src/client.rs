use std::{collections::HashMap, time::Duration};

use base64::engine::{Engine, general_purpose::STANDARD as BASE64};
use tokio::{
    io::{AsyncBufReadExt, AsyncWriteExt, BufReader},
    net::TcpStream,
};
use url::Url;

use crate::{error::UpgradeError, role::Client, ws::WebSocket};

type Result<T> = std::result::Result<T, UpgradeError>;

pub type WebSocketClient = WebSocket<Client>;

/// Client connection implementation for WebSocket
impl WebSocketClient {
    /// Attempts to connect to socket and upgrade connection.
    /// # Errors
    /// Fails if unable to connect to the peer.
    pub async fn connect(input: &str) -> Result<Self> {
        let url = Url::parse(input).map_err(|_| UpgradeError::InvalidUrl)?;
        if url.scheme() != "ws" && url.scheme() != "wss" {
            return Err(UpgradeError::InvalidUrl);
        }
        Self::try_upgrade(url).await
    }

    /// Attempts to connect to socket and upgrade connection, with timeout.
    /// # Errors
    /// Fails if unable to connect to the peer or timeout has elapsed.
    pub async fn connect_timeout(input: &str, timeout: Duration) -> Result<Self> {
        let fut = Self::connect(input);
        match tokio::time::timeout(timeout, fut).await {
            Ok(Ok(s)) => Ok(s),
            _ => Err(UpgradeError::Timeout),
        }
    }

    async fn try_upgrade(url: Url) -> Result<Self> {
        let host = url.host_str().ok_or(UpgradeError::InvalidUrl)?;
        let port = url.port_or_known_default().unwrap();
        let path = url.path();

        let mut stream = TcpStream::connect((host, port))
            .await
            .map_err(|_| UpgradeError::Connect)?;
        // store peer_addr
        let addr = stream.peer_addr().map_err(|_| UpgradeError::Addr)?;

        let sec_websocket_key = BASE64.encode(rand::random::<[u8; 16]>());

        let req = format!(
            "GET {path} HTTP/1.1\r\n\
            Host: {host}:{port}\r\n\
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
        stream.flush().await.map_err(|_| UpgradeError::Write)?;

        // get status line and validate status code
        let mut reader = BufReader::new(stream);
        let mut status_line = String::new();
        reader
            .read_line(&mut status_line)
            .await
            .map_err(|_| UpgradeError::Read)?;

        let mut status_parts = status_line.split_whitespace();
        if status_parts.next() != Some("HTTP/1.1") || status_parts.next() != Some("101") {
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
                headers.insert(name.trim().to_lowercase(), value.trim().to_string());
            }
        }

        Self::validate_header(&headers, "upgrade", "websocket")?;
        // TODO: multiple connection values
        Self::validate_header(&headers, "connection", "upgrade")?;
        // TODO: case sensitive check
        let expected_accept = Self::hash_key(&sec_websocket_key);
        Self::validate_header(&headers, "sec-websocket-accept", &expected_accept)?;

        let ws = Self::from_stream(reader.into_inner());
        tracing::info!(addr = ?addr, "successfully connected to peer");
        Ok(ws)
    }
}

use std::{collections::HashMap, net::SocketAddr, sync::Arc, time::Duration};

use base64::engine::{Engine, general_purpose::STANDARD as BASE64};
use rustls::ClientConfig;
use tokio::{
    io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader},
    net::TcpStream,
};
use tokio_rustls::{
    TlsConnector,
    rustls::pki_types::{CertificateDer, IpAddr, ServerName, pem::PemObject},
};

use crate::{error::UpgradeError, role::Client, ws::WebSocket};

type Result<T> = std::result::Result<T, UpgradeError>;

pub type WebSocketClient = WebSocket<Client>;

struct ClientContext<'a> {
    host: &'a str,
    path: &'a str,
    port: u16,
    local_addr: SocketAddr,
    peer_addr: SocketAddr,
}

/// Client connection implementation for WebSocket
impl WebSocketClient {
    /// Attempts to connect to the given url and upgrade connection.
    /// Urls must be of format `"ws[s]://host[:port][/path]"` where
    /// host is either a domain name or IP address.
    /// # Errors
    /// Fails if unable to connect to the peer.
    pub async fn connect(input: &str, compressed: bool) -> Result<Self> {
        // url metadata
        let url = url::Url::parse(input).map_err(|_| UpgradeError::InvalidUrl)?;
        let host = url.host_str().ok_or(UpgradeError::InvalidUrl)?;
        let port = url
            .port_or_known_default()
            .ok_or(UpgradeError::InvalidUrl)?;
        let path = url.path();

        let stream = TcpStream::connect((host, port))
            .await
            .map_err(|_| UpgradeError::Connect)?;

        let ctx = ClientContext {
            host,
            path,
            port,
            local_addr: stream.local_addr().map_err(|_| UpgradeError::Addr)?,
            peer_addr: stream.peer_addr().map_err(|_| UpgradeError::Addr)?,
        };

        if url.scheme() == "ws" {
            // standard TCP
            tracing::info!("attempting insecure upgrade");
            Self::try_upgrade(stream, ctx, compressed).await
        } else if url.scheme() == "wss" {
            // TCP with TLS

            // try host as hostname (DNS) otherwise try IP address
            let server = ServerName::try_from(host.to_string()).or_else(|_| {
                let name = format!("{host}:{port}");
                IpAddr::try_from(name.as_str())
                    .map(ServerName::from)
                    .map_err(|_| UpgradeError::InvalidUrl)
            })?;

            // convert stream to TLS and try handshake
            let connector = TlsConnector::from(get_tls_config());
            let stream = connector
                .connect(server, stream)
                .await
                .map_err(|_| UpgradeError::Connect)?;
            tracing::info!("attempting TLS upgrade");
            Self::try_upgrade(stream, ctx, compressed).await
        } else {
            tracing::error!("invalid scheme");
            Err(UpgradeError::InvalidUrl)
        }
    }

    /// Attempts to call [`connect`](WebSocketClient::connect), with a timeout. See `connect` for
    /// more information.
    /// # Errors
    /// Fails if unable to connect to the peer or timeout has elapsed.
    pub async fn connect_timeout(input: &str, timeout: Duration, compressed: bool) -> Result<Self> {
        let fut = Self::connect(input, compressed);
        match tokio::time::timeout(timeout, fut).await {
            Ok(Ok(s)) => Ok(s),
            _ => Err(UpgradeError::Timeout),
        }
    }

    async fn try_upgrade<S>(mut stream: S, ctx: ClientContext<'_>, compressed: bool) -> Result<Self>
    where
        S: AsyncReadExt + AsyncWriteExt + Send + Unpin + 'static,
    {
        let sec_websocket_key = BASE64.encode(rand::random::<[u8; 16]>());

        let mut req = format!(
            "GET {} HTTP/1.1\r\n\
            Host: {}:{}\r\n\
            Upgrade: websocket\r\n\
            Connection: Upgrade\r\n\
            Sec-WebSocket-Key: {sec_websocket_key}\r\n\
            Sec-WebSocket-Version: 13\r\n",
            ctx.path, ctx.host, ctx.port
        );
        if compressed {
            req.push_str("Sec-WebSocket-Extensions: permessage-deflate");
            //req.push_str("; client_no_context_takeover");
            //req.push_str("; server_no_context_takeover");
            req.push_str("\r\n");
        }
        req.push_str("\r\n");

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

        // TODO: parse context and compression from headers
        let compressed = true;
        let use_context = true;
        tracing::info!(addr = ?ctx.peer_addr, "successfully connected to peer");
        Ok(Self::from_stream(
            reader.into_inner(),
            ctx.local_addr,
            ctx.peer_addr,
            compressed,
            use_context,
        ))
    }
}

fn get_tls_config() -> Arc<ClientConfig> {
    let certs = CertificateDer::pem_file_iter("certs/root-ca.pem")
        .unwrap()
        .collect::<std::result::Result<Vec<_>, _>>()
        .unwrap();

    let root_cert_store = {
        let mut s = rustls::RootCertStore::empty();
        for cert in certs {
            s.add(cert).unwrap();
        }
        s.extend(webpki_roots::TLS_SERVER_ROOTS.iter().cloned());
        s
    };

    let config = rustls::ClientConfig::builder()
        .with_root_certificates(root_cert_store)
        .with_no_client_auth(); // i guess this was previously the default?
    Arc::new(config)
}

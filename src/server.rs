use std::{collections::HashMap, net::SocketAddr, sync::Arc};

use rustls::ServerConfig;
use tokio::{
    io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader},
    net::{TcpListener, ToSocketAddrs},
};
use tokio_rustls::{
    TlsAcceptor,
    rustls::pki_types::{CertificateDer, PrivateKeyDer, pem::PemObject},
};

use crate::{
    error::UpgradeError,
    role::Server,
    ws::{MessageHandler, WebSocket},
};

type Result<T> = std::result::Result<T, UpgradeError>;

pub struct WebSocketServer {
    listener: TcpListener,
    addr: SocketAddr,
    insecure: bool,
    ssl: bool,
}

impl WebSocketServer {
    /// Creates a new `WebSocketServer` which will be bound to the specified address.
    ///
    /// Binding with a port number of 0 will request that the OS assigns a port to this listener. The port allocated can be queried via the [`addr`](WebSocketServer::addr) method.
    ///
    /// The address type can be any implementor of [`ToSocketAddrs`] trait. See its documentation for concrete examples.
    ///
    /// If addr yields multiple addresses, bind will be attempted with each of the addresses until one succeeds and returns the listener.
    ///
    /// The `insecure` parameter sets whether the server accepts insecure connections over TCP.
    /// Similarly, the `ssl` parameter sets whether the server accepts secure connecions over TLS.
    ///
    /// # Errors
    /// Will fail if unable to bind to any address.
    pub async fn bind<A: ToSocketAddrs>(addr: A, insecure: bool, ssl: bool) -> Result<Self> {
        let listener = TcpListener::bind(addr)
            .await
            .map_err(|_| UpgradeError::Bind)?;
        let addr = listener.local_addr().map_err(|_| UpgradeError::Addr)?;
        tracing::info!(addr = ?addr, "Listening on");
        Ok(Self {
            listener,
            addr,
            insecure,
            ssl,
        })
    }

    /// TODO:
    pub async fn run<H: MessageHandler>(&self, handler: H) {
        let acceptor = TlsAcceptor::from(get_tls_config());

        let peer = self.addr;
        let insecure = self.insecure;
        let ssl = self.ssl;
        let handler = Arc::new(handler);
        while let Ok((stream, addr)) = self.listener.accept().await {
            let handler = handler.clone();
            let acceptor = acceptor.clone();
            tokio::task::spawn(async move {
                // check first few bytes of request.
                let mut peeker = [0; 4];
                match stream.peek(&mut peeker).await {
                    Ok(0) => {
                        return; // client disconnected
                    }
                    Err(e) => {
                        tracing::error!(e=?e, "could not read socket");
                        return;
                    }
                    _ => {}
                }

                // attempt to connect to and upgrade stream
                let conn_res = if peeker.starts_with(b"GET ") {
                    // if we have "GET ", it is plain TCP
                    if insecure {
                        tracing::info!("attempting insecure upgrade");
                        WebSocket::<Server>::try_upgrade(stream, addr, peer).await
                    } else {
                        Err(UpgradeError::Protocol)
                    }
                } else {
                    // otherwise try to use TLS
                    if ssl {
                        match acceptor.accept(stream).await {
                            Ok(stream) => {
                                tracing::info!("attempting TLS upgrade");
                                WebSocket::<Server>::try_upgrade(stream, addr, peer).await
                            }
                            Err(e) => {
                                tracing::error!(e=?e, "tls handshake");
                                Err(UpgradeError::Connect)
                            }
                        }
                    } else {
                        Err(UpgradeError::Protocol)
                    }
                };

                let mut ws = match conn_res {
                    Ok(ws) => ws,
                    Err(e) => {
                        tracing::error!(addr=?addr, e=?e, "failed to upgrade");
                        return;
                    }
                };

                ws.recv_loop(handler).await;
            });
        }
    }

    /// Returns the local socket address of this server.
    pub fn addr(&self) -> SocketAddr { self.addr }
}

fn get_tls_config() -> Arc<ServerConfig> {
    let certs = CertificateDer::pem_file_iter("certs/cert.pem")
        .unwrap()
        .collect::<std::result::Result<Vec<_>, _>>()
        .unwrap();
    let key = PrivateKeyDer::from_pem_file("certs/cert.key.pem").unwrap();

    let config = rustls::ServerConfig::builder()
        .with_no_client_auth()
        .with_single_cert(certs, key)
        .unwrap();
    Arc::new(config)
}

impl WebSocket<Server> {
    async fn try_upgrade<S>(
        stream: S,
        local_addr: SocketAddr,
        peer_addr: SocketAddr,
    ) -> Result<Self>
    where
        S: AsyncReadExt + AsyncWriteExt + Send + Unpin + 'static,
    {
        let mut reader = BufReader::new(stream);
        let mut status_line = String::new();
        reader
            .read_line(&mut status_line)
            .await
            .map_err(|_| UpgradeError::Read)?;

        let mut status_parts = status_line.split_whitespace();
        if status_parts.next() != Some("GET")
            || status_parts.next().is_none() // TODO: parse path
            || status_parts.next() != Some("HTTP/1.1")
        {
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
        if !headers.contains_key("host") {
            return Err(UpgradeError::MissingHeader("host"));
        }

        Self::validate_header(&headers, "upgrade", "websocket")?;
        Self::validate_header(&headers, "connection", "upgrade")?;
        Self::validate_header(&headers, "sec-websocket-version", "13")?;

        let key = headers
            .get("sec-websocket-key")
            .ok_or(UpgradeError::MissingHeader("sec-websocket-key"))?;

        let accept_key = Self::hash_key(key);

        let response = format!(
            "HTTP/1.1 101 Switching Protocols\r\n\
             Upgrade: websocket\r\n\
             Connection: Upgrade\r\n\
             Sec-WebSocket-Accept: {accept_key}\r\n\r\n",
        );

        let mut stream = reader.into_inner();
        stream
            .write_all(response.as_bytes())
            .await
            .map_err(|_| UpgradeError::Write)?;
        stream.flush().await.map_err(|_| UpgradeError::Write)?;

        tracing::info!(addr = ?local_addr, "upgraded client");
        Ok(Self::from_stream(stream, local_addr, peer_addr))
    }
}

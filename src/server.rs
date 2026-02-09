use std::{collections::HashMap, net::SocketAddr};

use tokio::{
    io::{AsyncBufReadExt, AsyncWriteExt, BufReader},
    net::{TcpListener, TcpStream, ToSocketAddrs},
};

use crate::{Event, Message, error::UpgradeError, role::Server, ws::WebSocket};

type Result<T> = std::result::Result<T, UpgradeError>;
pub type ServerConn = WebSocket<Server>;

pub struct WebSocketServer {
    listener: TcpListener,
    addr: SocketAddr,
}

// TODO: make handlers work properly
pub trait MessageHandler: Send + Sync + Copy + Clone + 'static {
    fn on_text<'a>(&self, s: &'a str) -> Option<impl AsRef<[u8]> + Send + 'a>;
    fn on_binary<'a>(&self, b: &'a [u8]) -> Option<impl AsRef<[u8]> + Send + 'a>;
    fn on_close(&self) -> Option<impl AsRef<[u8]>>;
    fn on_error(&self, e: &[u8]) -> Option<impl AsRef<[u8]>>;
    fn on_pong(&self, latency: u16) -> Option<impl AsRef<[u8]>>;
}

impl WebSocketServer {
    /// Creates a new `WebSocketServer` which will be bound to the specified address.
    ///
    /// Binding with a port number of 0 will request that the OS assigns a port to this listener. The port allocated can be queried via the [`WebSocketServer::addr`] method.
    ///
    /// The address type can be any implementor of [`ToSocketAddrs`] trait. See its documentation for concrete examples.
    ///
    /// If addr yields multiple addresses, bind will be attempted with each of the addresses until one succeeds and returns the listener.
    /// # Errors
    /// Will fail if unable to bind to any address.
    pub async fn bind(addr: impl ToSocketAddrs) -> Result<Self> {
        let listener = TcpListener::bind(addr)
            .await
            .map_err(|_| UpgradeError::Bind)?;
        let addr = listener.local_addr().map_err(|_| UpgradeError::Addr)?;
        tracing::info!(addr = ?addr, "Listening on");
        Ok(Self { listener, addr })
    }

    pub async fn run<H: MessageHandler>(&self, handler: H) {
        while let Ok((stream, addr)) = self.listener.accept().await {
            tokio::task::spawn(async move {
                let Ok(mut ws) = ServerConn::try_upgrade(stream, addr).await else {
                    println!("failed to upgrade");
                    return;
                };

                // Spawn a thread to handle events from this client
                while let Some(event) = ws.event_rx.recv().await {
                    match event {
                        Event::Message(msg) => match msg {
                            Message::Text(s) => {
                                if let Some(reply) = handler.on_text(&s)
                                    && let Err(e) = ws.send_bytes(reply.as_ref()).await
                                {
                                    tracing::error!(e = ?e, "failed to send text message");
                                }
                            }
                            Message::Binary(b) => {
                                if let Some(reply) = handler.on_binary(&b)
                                    && let Err(e) = ws.send_bytes(reply.as_ref()).await
                                {
                                    tracing::error!(e = ?e, "failed to send binary message");
                                }
                            }
                        },
                        Event::Closed => {
                            handler.on_close();
                            break;
                        }
                        Event::Error(e) => {
                            handler.on_error(&e.0);
                        }
                        Event::Pong(latency) => {
                            handler.on_pong(latency);
                        }
                    }
                }
            });
        }
    }

    /// Returns the local socket address of this server.
    pub fn addr(&self) -> SocketAddr { self.addr }
}

impl ServerConn {
    async fn try_upgrade(stream: TcpStream, addr: SocketAddr) -> Result<Self> {
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

        let ws = Self::from_stream(stream);
        tracing::info!(addr = ?addr, "upgraded client");
        Ok(ws)
    }
}

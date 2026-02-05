use std::io::Result;

use base64::engine::{Engine, general_purpose::STANDARD as base64};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::{TcpListener, TcpStream, ToSocketAddrs},
};

use crate::{Event, Message, role::Server, ws::WebSocket};

pub type ServerConn = WebSocket<Server>;

pub struct WebSocketServer {
    listener: TcpListener,
}

impl WebSocketServer {
    pub async fn bind(addr: impl ToSocketAddrs) -> Result<Self> {
        let listener = TcpListener::bind(addr).await?;
        tracing::info!(addr = listener.local_addr()?.to_string(), "Listening on");
        Ok(Self { listener })
    }

    pub async fn run(&self) -> Result<()> {
        while let Ok((stream, _addr)) = self.listener.accept().await {
            tokio::task::spawn(async move {
                let mut ws = ServerConn::try_upgrade(stream).await.unwrap();

                // Spawn a thread to handle events from this client
                while let Some(event) = ws.event_rx.recv().await {
                    match event {
                        Event::Message(msg) => match msg {
                            Message::Text(s) => {
                                println!("got message T {}", s.len());
                                ws.send_text(&s).await.unwrap();
                            }
                            Message::Binary(b) => {
                                println!("got messsage B {}", b.len());
                                ws.send_bytes(&b).await.unwrap();
                            }
                        },
                        Event::Closed => {
                            println!("client closed");
                            break;
                        }
                        Event::Error(e) => {
                            eprintln!("client error {e:?}");
                        }
                        Event::Pong(latency) => {
                            println!("pong latency {latency}ms");
                        }
                    }
                }
            });
        }
        Ok(())
    }
}

impl ServerConn {
    async fn try_upgrade(mut stream: TcpStream) -> std::result::Result<Self, std::io::Error> {
        let mut buf = [0; 1024];
        let n = stream.read(&mut buf).await?;
        let request = String::from_utf8_lossy(&buf[..n]);

        // Extract Sec-WebSocket-Key
        let key = request
            .lines()
            .find(|l| l.starts_with("Sec-WebSocket-Key:"))
            .map(|l| l.split(':').nth(1).unwrap().trim())
            .unwrap();

        let accept_key = {
            use sha1::{Digest, Sha1};
            let mut sha = Sha1::new();
            sha.update(key.as_bytes());
            sha.update(b"258EAFA5-E914-47DA-95CA-C5AB0DC85B11");
            base64.encode(sha.finalize())
        };

        let response = format!(
            "HTTP/1.1 101 Switching Protocols\r\n\
             Upgrade: websocket\r\n\
             Connection: Upgrade\r\n\
             Sec-WebSocket-Accept: {accept_key}\r\n\r\n",
        );

        stream.write_all(response.as_bytes()).await?;

        let ws = Self::from_stream(stream);
        tracing::info!(addr = ?ws.addr(), "upgraded client");
        Ok(ws)
    }
}

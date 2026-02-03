use std::{
    io::{Error, Read, Result, Write},
    marker::PhantomData,
    net::{SocketAddr, TcpListener, TcpStream, ToSocketAddrs},
    sync::{atomic::AtomicBool, mpsc::channel, Arc, Mutex},
    thread,
};

use base64::engine::{general_purpose::STANDARD as base64, Engine};

use crate::{inner::InnerTrait, protocol::PingStats, role::Server, ws::WebSocket, Event, Message};

pub struct WebSocketServer {
    listener: TcpListener,
}

pub type ServerConn = WebSocket<ServerConnInner, Server>;

impl ServerConn {
    pub fn addr(&self) -> Result<SocketAddr> { self.inner.reader.lock().unwrap().local_addr() }
}

pub struct ServerConnInner {
    reader: Mutex<TcpStream>,
    writer: Mutex<TcpStream>,
    ping_stats: Mutex<PingStats>,
    closed: AtomicBool,
    closing: AtomicBool,
}

impl InnerTrait<Server> for ServerConnInner {
    fn writer(&self) -> &Mutex<TcpStream> { &self.writer }

    fn reader(&self) -> &Mutex<TcpStream> { &self.reader }

    fn ping_stats(&self) -> &Mutex<PingStats> { &self.ping_stats }

    fn closed(&self) -> &AtomicBool { &self.closed }

    fn closing(&self) -> &AtomicBool { &self.closing }
}

impl WebSocketServer {
    pub fn bind(addr: impl ToSocketAddrs) -> Result<Self> {
        let listener = TcpListener::bind(addr)?;
        tracing::info!(addr = listener.local_addr()?.to_string(), "Listening on");
        Ok(Self { listener })
    }

    pub fn run(&self) -> Result<()> {
        for stream in self.listener.incoming() {
            let ws: ServerConn = stream?.try_into().unwrap();
            tracing::info!(addr = ws.inner.addr()?.to_string(), "SVR: client connected");
            let (event_tx, event_rx) = channel();
            ws.recv_loop(event_tx);
            ws.ping()?;

            // Spawn a thread to handle events from this client
            thread::spawn(move || {
                for event in event_rx {
                    match event {
                        Event::Message(msg) => match msg {
                            Message::Text(s) => {
                                println!("SVR: got message T {}", s.len());
                                ws.send_text(&s).unwrap();
                            }
                            Message::Binary(b) => {
                                println!("SVR: got messsage B {}", b.len());
                                ws.send_bytes(&b).unwrap();
                            }
                        },
                        Event::Closed => {
                            println!("SVR: client closed");
                            break;
                        }
                        Event::Error(e) => {
                            eprintln!("SVR: client error {e:?}");
                        }
                        Event::Pong(latency) => {
                            tracing::info!("SVR: pong latency {latency}ms");
                        }
                    }
                }
            });
        }
        Ok(())
    }

    pub fn addr(&self) -> Result<SocketAddr> { self.listener.local_addr() }
}

impl TryFrom<TcpStream> for ServerConn {
    type Error = Error;

    fn try_from(mut stream: TcpStream) -> std::result::Result<Self, Self::Error> {
        let mut buf = [0; 1024];
        let n = stream.read(&mut buf).unwrap();
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

        stream.write_all(response.as_bytes()).unwrap();

        let (_, event_rx) = channel();
        Ok(Self {
            inner: Arc::new(ServerConnInner {
                reader: Mutex::new(stream.try_clone()?),
                writer: Mutex::new(stream),
                ping_stats: Mutex::new(PingStats::new()),
                closed: AtomicBool::new(false),
                closing: AtomicBool::new(false),
            }),
            event_rx,
            _p: PhantomData,
        })
    }
}

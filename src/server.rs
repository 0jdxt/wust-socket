use std::{
    io::{Error, Result},
    marker::PhantomData,
    net::{SocketAddr, TcpListener, TcpStream, ToSocketAddrs},
    sync::{atomic::AtomicBool, mpsc::channel, Arc, Mutex},
};

use crate::{inner::InnerTrait, role::Server, ws::WebSocket};

pub struct WebSocketServer {
    listener: TcpListener,
}

pub type ServerConn = WebSocket<ServerConnInner, Server>;

pub struct ServerConnInner {
    reader: Mutex<TcpStream>,
    writer: Mutex<TcpStream>,
    closed: AtomicBool,
    closing: AtomicBool,
}

impl InnerTrait<Server> for ServerConnInner {
    fn writer(&self) -> &Mutex<TcpStream> { &self.writer }

    fn closed(&self) -> &AtomicBool { &self.closed }

    fn closing(&self) -> &AtomicBool { &self.closing }
}

impl WebSocketServer {
    pub fn bind(addr: impl ToSocketAddrs) -> Result<Self> {
        Ok(Self {
            listener: TcpListener::bind(addr)?,
        })
    }

    pub fn accept(&self) -> Result<ServerConn> { self.listener.accept()?.0.try_into() }

    pub fn addr(&self) -> Result<SocketAddr> { self.listener.local_addr() }
}

impl TryFrom<TcpStream> for ServerConn {
    type Error = Error;

    fn try_from(stream: TcpStream) -> std::result::Result<Self, Self::Error> {
        let (_, event_rx) = channel();
        Ok(Self {
            inner: Arc::new(ServerConnInner {
                reader: Mutex::new(stream.try_clone()?),
                writer: Mutex::new(stream),
                closed: AtomicBool::new(false),
                closing: AtomicBool::new(false),
            }),
            event_rx,
            _p: PhantomData,
        })
    }
}

use std::io::BufRead;
use std::io::BufReader;
use std::io::Write;
use std::net::TcpListener;
use std::sync::mpsc;

use wust_socket::WebSocket;

#[test]
fn handshake_and_close() {
    let server = "127.0.0.1:6969";
    let (tx, rx) = mpsc::channel::<()>();

    // dummy server
    std::thread::spawn(move || {
        let listener = TcpListener::bind(server).unwrap();
        // signal server is ready
        tx.send(()).unwrap();
        for stream in listener.incoming() {
            let mut stream = stream.unwrap();
            let mut reader = BufReader::new(&mut stream);
            let mut line = String::new();
            reader.read_line(&mut line).unwrap();

            // parse headers into a HashMap
            let mut headers = std::collections::HashMap::new();
            loop {
                line.clear();
                reader.read_line(&mut line).unwrap();
                let line = line.trim_end();
                if line.is_empty() {
                    break;
                }
                if let Some((k, v)) = line.split_once(':') {
                    headers.insert(k.trim().to_string(), v.trim().to_string());
                }
            }

            // compute Sec-WebSocket-Accept
            use base64::engine::{general_purpose::STANDARD as BASE64, Engine};
            use sha1::{Digest, Sha1};

            let client_key = headers.get("Sec-WebSocket-Key").unwrap();
            let mut hasher = Sha1::new();
            hasher.update(client_key);
            hasher.update("258EAFA5-E914-47DA-95CA-C5AB0DC85B11");
            let accept = BASE64.encode(hasher.finalize());

            // send handshake response
            let resp = format!(
                "HTTP/1.1 101 Switching Protocols\r\n\
                 Upgrade: websocket\r\n\
                 Connection: Upgrade\r\n\
                 Sec-WebSocket-Accept: {accept}\r\n\r\n"
            );
            reader.into_inner().write_all(resp.as_bytes()).unwrap();
        }
    });

    // receive signal that server is ready
    rx.recv().unwrap();

    // client side, just handshake and close cleanly
    WebSocket::connect(server).unwrap().close().unwrap();
}

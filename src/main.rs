#![warn(clippy::all, clippy::pedantic)]

use std::io::Result;
use std::thread;
use std::time::Duration;

use wust_socket::WebSocket;

fn main() -> Result<()> {
    let mut ws = WebSocket::connect("127.0.0.1:8765")?;

    for _ in 0..3 {
        thread::sleep(Duration::from_secs(10));
        ws.send_text("hello from wust-socket!")?;
        let x = ws.recv().unwrap();
        println!("Received message: {x:?}");
        println!("latency: {:?}", ws.latency());
    }

    println!("Waiting for message for 2 secs...");
    let x = ws.recv_timeout(Duration::from_secs(2));
    println!("{x:?}");

    ws.close()?;

    Ok(())
}

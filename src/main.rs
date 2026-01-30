#![allow(dead_code, unused)]
use std::{
    fs::File,
    io::{Read, Result},
    thread,
    time::Duration,
};

use wust_socket::{Event, WebSocketClient, WebSocketServer};

static LOREM: &str = "Lorem ipsum dolor sit amet consectetur adipiscing elit quisque faucibus ex sapien vitae pellentesque sem placerat in id cursus mi pretium tellus duis convallis tempus leo eu aenean sed diam urna tempor pulvinar vivamus fringilla lacus nec metus bibendum egestas iaculis massa nisl malesuada lacinia integer nunc posuere ut hendrerit semper vel class aptent taciti sociosqu ad litora torquent per conubia nostra inceptos himenaeos orci varius natoque penatibus et magnis dis parturient montes nascetur ridiculus mus donec rhoncus eros lobortis nulla molestie mattis scelerisque maximus eget fermentum odio phasellus non purus est efficitur laoreet mauris pharetra vestibulum fusce dictum risus.";

fn main() -> Result<()> {
    let server = "127.0.0.1:8765";
    let mut ws = WebSocketClient::connect(server)?;

    ws.send_text("\n")?;
    ws.send_text(LOREM)?;

    println!("{:?}", ws.recv());
    println!("{:?}", ws.recv());
    println!("{:?}", ws.recv());

    // for i in 0..5 {
    //     let msg = format!("hello from wust_socket {i}!");
    //     if let Err(e) = ws.send_text(&msg) {
    //         eprintln!("send failed: {e}");
    //         break;
    //     }
    // }

    // let data = {
    //     let mut s = String::new();
    //     let mut f = File::open("/usr/share/cracklib/cracklib-small")?;
    //     f.read_to_string(&mut s)?;
    //     s
    // };

    // for word in LOREM.split_ascii_whitespace() {
    //     ws.send_text(word)?;
    //     println!("{:?}", ws.recv());
    // }

    // while let Some(e) = ws.recv_timeout(Duration::from_secs(2)) {
    //     match e {
    //         Event::Closed => {
    //             println!("connection closed");
    //             break;
    //         }
    //         Event::Pong(n) => println!("Pong: {n}ms"),
    //         Event::Message(m) => println!("Message: {}", m.as_str().unwrap()),
    //         Event::Error(e) => println!("Error: {e}"),
    //     }
    // }

    ws.close()?;
    // wait for close/tcp fin
    thread::sleep(Duration::from_millis(50));

    let server = WebSocketServer::bind("127.0.0.1:0")?;
    println!("listening: {}", server.addr()?);

    Ok(())
}

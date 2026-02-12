use std::{
    fs::File,
    io::{Read, Write},
    time::Duration,
};

use clap::Parser;
use tokio::sync::mpsc::error::SendError;
use tracing_subscriber::EnvFilter;
use wust_socket::{Event, Message, WebSocketClient};

static LOREM: &str = "Lorem ipsum dolor sit amet consectetur adipiscing elit quisque faucibus ex sapien vitae pellentesque sem placerat in id cursus mi pretium tellus duis convallis tempus leo eu aenean sed diam urna tempor pulvinar vivamus fringilla lacus nec metus bibendum egestas iaculis massa nisl malesuada lacinia integer nunc posuere ut hendrerit semper vel class aptent taciti sociosqu ad litora torquent per conubia nostra inceptos himenaeos orci varius natoque penatibus et magnis dis parturient montes nascetur ridiculus mus donec rhoncus eros lobortis nulla molestie mattis scelerisque maximus eget fermentum odio phasellus non purus est efficitur laoreet mauris pharetra vestibulum fusce dictum risus.";

#[derive(Parser)]
#[command(author, version, about)]
struct Args {
    /// Server address to connect to
    #[arg(short, long, default_value = "127.0.0.1")]
    addr: String,

    /// Port to connect to
    #[arg(short, long, default_value_t = 9001)]
    port: u16,
}

impl Args {
    fn as_url(&self) -> String { format!("ws://{}:{}", self.addr, self.port) }
}

#[tokio::main]
async fn main() -> Result<(), SendError<Vec<u8>>> {
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::from_default_env().add_directive("wust_socket=info".parse().unwrap()),
        )
        .with_target(false)
        .compact()
        .init();

    let url = Args::parse().as_url();

    let mut ws = WebSocketClient::connect(&url, true).await.expect("connect");

    // --- Send file
    let mut data = Vec::new();
    let mut f = File::open("lena512.bmp").unwrap();
    f.read_to_end(&mut data).unwrap();
    for chunk in data.chunks(32 << 10) {
        ws.send_bytes(chunk).await?
    }

    ws.send_text(LOREM).await?;

    let mut f = File::create("out.bmp").unwrap();
    while let Some(e) = ws.recv_timeout(Duration::from_secs(1)).await {
        match e {
            Event::Closed => {
                println!("connection closed");
                break;
            }
            Event::Pong(n) => println!("PONG: {n}ms"),
            Event::Message(m) => match m {
                Message::Text(s) => {
                    let l = s.ceil_char_boundary(200);
                    println!("got message T {} {:?}", s.len(), &s[..l]);
                }
                Message::Binary(b) => {
                    let l = b.len().min(10);
                    f.write_all(&b).unwrap();
                    println!("got messsage B {} {:?}", b.len(), &b[..l]);
                }
            },

            Event::Error(e) => println!("ERR: {e}"),
        }
    }
    ws.close().await;

    Ok(())
}

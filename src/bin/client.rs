use std::{
    fs::File,
    io::{Read, Result},
    thread,
    time::Duration,
};

use clap::Parser;
use tracing_subscriber::EnvFilter;
use wust_socket::{Event, WebSocketClient};

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

fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::from_default_env().add_directive("wust_socket=info".parse().unwrap()),
        )
        .with_target(false)
        .compact()
        .init();

    let data = {
        let mut s = String::new();
        let mut f = File::open("/usr/share/cracklib/cracklib-small")?;
        f.read_to_string(&mut s)?;
        s
    };

    let args = Args::parse();

    for _ in 0..2 {
        let mut ws = WebSocketClient::connect((args.addr.as_str(), args.port)).expect("connect");

        ws.send_text(&data)?;
        ws.send_text(LOREM)?;

        while let Some(e) = ws.recv_timeout(Duration::from_millis(50)) {
            match e {
                Event::Closed => {
                    println!("CLI connection closed");
                    break;
                }
                Event::Pong(n) => println!("CLI PONG: {n}ms"),
                Event::Message(m) => println!("CLI MESSAGE: {}", m.as_str().unwrap().len()),
                Event::Error(e) => println!("CLI ERR: {e}"),
            }
        }

        ws.close()?;
        // wait for close/tcp fin
        thread::sleep(Duration::from_millis(100));
    }

    Ok(())
}

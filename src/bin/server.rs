use clap::Parser;
use tokio::sync::mpsc::error::SendError;
use tracing_subscriber::EnvFilter;
use wust_socket::{MessageHandler, ServerConn, UpgradeError, WebSocketServer};

#[derive(Parser)]
#[command(author, version, about)]
struct Args {
    /// Server address to connect to
    #[arg(short, long, default_value = "127.0.0.1")]
    addr: String,

    /// Port to connect to
    #[arg(short, long, default_value_t = 0)]
    port: u16,
}

#[derive(Clone, Copy)]
struct Handler;
impl MessageHandler for Handler {
    fn on_text<'a>(&self, s: &'a str) -> Option<&'a str> {
        let l = s.ceil_char_boundary(10);
        println!("got message T {} {}", s.len(), &s[..l]);
        Some(s)
    }

    fn on_binary<'a>(&self, b: &'a [u8]) -> Option<&'a [u8]> {
        let l = b.len().min(10);
        println!("got messsage B {} {:?}", b.len(), &b[..l]);
        Some(b)
    }

    fn on_close(&self) -> Option<impl AsRef<[u8]>> {
        println!("client closed");
        None::<String>
    }

    fn on_error(&self, e: &[u8]) -> Option<impl AsRef<[u8]>> {
        eprintln!("client error {e:?}");
        None::<String>
    }

    fn on_pong(&self, latency: u16) -> Option<impl AsRef<[u8]>> {
        println!("pong latency {latency}ms");
        None::<String>
    }
}

#[tokio::main]
async fn main() -> Result<(), UpgradeError> {
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::from_default_env().add_directive("wust_socket=info".parse().unwrap()),
        )
        .with_target(false)
        .compact()
        .init();

    let args = Args::parse();

    let server = WebSocketServer::bind((args.addr.as_str(), args.port)).await?;
    server.run(Handler).await;

    Ok(())
}

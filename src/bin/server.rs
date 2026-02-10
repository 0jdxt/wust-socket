use clap::Parser;
use tracing_subscriber::EnvFilter;
use wust_socket::{MessageHandler, UpgradeError, WebSocketServer, WsMessage};

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

    WebSocketServer::bind((args.addr.as_str(), args.port), true, true)
        .await?
        .run(EchoHandler)
        .await;

    Ok(())
}

struct EchoHandler;
#[async_trait::async_trait]
impl MessageHandler for EchoHandler {
    async fn on_text(&self, s: String) -> Option<WsMessage> {
        let l = s.ceil_char_boundary(10);
        println!("got message T {} {:?}", s.len(), &s[..l]);
        Some(WsMessage::Text(s))
    }

    async fn on_binary(&self, b: Vec<u8>) -> Option<WsMessage> {
        let l = b.len().min(10);
        println!("got messsage B {} {:?}", b.len(), &b[..l]);
        Some(WsMessage::Binary(b))
    }

    async fn on_close(&self) {
        println!("client closed");
    }

    async fn on_error(&self, e: Vec<u8>) {
        eprintln!("client error {e:?}");
    }

    async fn on_pong(&self, latency: u16) {
        println!("pong latency {latency}ms");
    }
}

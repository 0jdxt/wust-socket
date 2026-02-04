use std::io::Result;

use clap::Parser;
use tracing_subscriber::EnvFilter;
use wust_socket::WebSocketServer;

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
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::from_default_env().add_directive("wust_socket=info".parse().unwrap()),
        )
        .with_target(false)
        .compact()
        .init();

    let args = Args::parse();

    let server = WebSocketServer::bind((args.addr.as_str(), args.port)).await?;
    server.run().await?;

    Ok(())
}

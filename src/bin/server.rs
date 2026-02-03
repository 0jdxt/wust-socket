use std::io::Result;

use tracing_subscriber::EnvFilter;
use wust_socket::WebSocketServer;

fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::from_default_env().add_directive("wust_socket=info".parse().unwrap()),
        )
        .with_target(false)
        .compact()
        .init();

    let server = WebSocketServer::bind("127.0.0.1:0")?;
    server.run()?;

    Ok(())
}

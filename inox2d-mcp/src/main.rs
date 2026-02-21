mod renderer;
mod server;

use rmcp::ServiceExt;
use tracing_subscriber::{filter::LevelFilter, fmt, prelude::*};

use server::InoxMcpServer;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
	// Set up tracing to stderr (stdout is used by MCP stdio transport)
	tracing_subscriber::registry()
		.with(fmt::layer().with_writer(std::io::stderr))
		.with(LevelFilter::INFO)
		.init();

	tracing::info!("Starting inox2d MCP server");

	let server = InoxMcpServer::new();
	let service = server.serve(rmcp::transport::io::stdio()).await?;

	service.waiting().await?;

	Ok(())
}

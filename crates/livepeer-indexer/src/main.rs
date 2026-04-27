use anyhow::Result;
use clap::Parser;
use tracing::info;
use tracing_subscriber::{fmt, prelude::*, EnvFilter};

const SERVICE: &str = "livepeer-indexer";

#[derive(Parser, Debug)]
#[command(name = SERVICE, about = "Pulls logs from RPC, decodes against the ABI registry, writes raw_protocol_events.")]
struct Args {}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::registry()
        .with(EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")))
        .with(fmt::layer().json())
        .init();

    let _ = Args::parse();

    info!(service = SERVICE, "skeleton — not implemented");
    info!(service = SERVICE, "see docs/product-specs/v1-livepeer-indexer.md §3.1, §6, §13 for the implementation contract");
    Ok(())
}

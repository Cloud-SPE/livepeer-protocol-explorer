use anyhow::Result;
use clap::Parser;
use tracing::info;
use tracing_subscriber::{fmt, prelude::*, EnvFilter};

const SERVICE: &str = "livepeer-valuator";

#[derive(Parser, Debug)]
#[command(name = SERVICE, about = "Prices finalized events; writes immutable rows to event_valuations under a named valuation_version.")]
struct Args {}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::registry()
        .with(EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")))
        .with(fmt::layer().json())
        .init();

    let _ = Args::parse();

    info!(service = SERVICE, "skeleton — not implemented");
    info!(service = SERVICE, "see docs/product-specs/v1-livepeer-indexer.md §7, §10 for the implementation contract");
    Ok(())
}

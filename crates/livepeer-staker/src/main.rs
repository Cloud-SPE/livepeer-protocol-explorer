use anyhow::Result;
use clap::Parser;
use tracing::info;
use tracing_subscriber::{fmt, prelude::*, EnvFilter};

const SERVICE: &str = "livepeer-staker";

#[derive(Parser, Debug)]
#[command(name = SERVICE, about = "Computes and persists delegator stake balances at event-touching blocks (Scope 2).")]
struct Args {}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::registry()
        .with(EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")))
        .with(fmt::layer().json())
        .init();

    let _ = Args::parse();

    info!(service = SERVICE, "skeleton — not implemented");
    info!(service = SERVICE, "see docs/product-specs/v1-livepeer-indexer.md §11.10, §11.11 for the implementation contract");
    Ok(())
}

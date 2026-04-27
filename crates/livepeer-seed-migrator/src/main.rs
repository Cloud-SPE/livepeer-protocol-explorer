use anyhow::Result;
use clap::Parser;
use std::path::PathBuf;
use tracing::info;
use tracing_subscriber::{fmt, prelude::*, EnvFilter};

const SERVICE: &str = "livepeer-seed-migrator";

#[derive(Parser, Debug)]
#[command(name = SERVICE, about = "One-shot. Imports trusted historical prices from SQLite into seeded_event_prices.")]
struct Args {
    /// Path to the source SQLite (e.g. /path/to/sqlite-4.0.db)
    #[arg(long)]
    source_sqlite: Option<PathBuf>,
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::registry()
        .with(EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")))
        .with(fmt::layer().json())
        .init();

    let args = Args::parse();
    info!(service = SERVICE, source = ?args.source_sqlite, "skeleton — not implemented");
    info!(service = SERVICE, "see docs/product-specs/v1-livepeer-indexer.md §8 for the implementation contract");
    Ok(())
}

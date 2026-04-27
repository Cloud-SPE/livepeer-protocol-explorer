use anyhow::{Context, Result};
use clap::Parser;
use livepeer_core::{config::Config, db, tracing_init};
use sqlx::sqlite::{SqliteConnectOptions, SqlitePoolOptions};
use std::path::PathBuf;
use std::str::FromStr;
use tracing::info;

const SERVICE: &str = "livepeer-seed-migrator";

#[derive(Parser, Debug)]
#[command(name = SERVICE, about = "One-shot. Imports trusted historical prices from SQLite into seeded_event_prices.")]
struct Args {
    /// Path to the source SQLite (e.g. /path/to/sqlite-4.0.db)
    #[arg(long, env = "SOURCE_SQLITE")]
    source_sqlite: PathBuf,

    /// Path to the static config (e.g. config/arbitrum.yaml)
    #[arg(long, env = "STATIC_CONFIG", default_value = "config/arbitrum.yaml")]
    static_config: PathBuf,

    /// Path to the env-specific config (e.g. config/env/dev.yaml)
    #[arg(long, env = "ENV_CONFIG", default_value = "config/env/dev.yaml")]
    env_config: PathBuf,
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_init::init("info");

    let args = Args::parse();
    info!(
        service = SERVICE,
        source_sqlite = %args.source_sqlite.display(),
        static_config = %args.static_config.display(),
        env_config = %args.env_config.display(),
        "starting"
    );

    let cfg = Config::load(&args.static_config, &args.env_config)
        .context("loading config")?;
    info!(
        chain = %cfg.static_.chain.name,
        chain_id = cfg.static_.chain.chain_id,
        livepeer_genesis_block = cfg.static_.chain.livepeer_arbitrum_genesis_block,
        "config loaded"
    );

    let database_url = cfg.database_url().context("resolving DATABASE_URL")?;
    let pool = db::connect(&database_url, cfg.env.postgres.pool_max_connections)
        .await
        .context("connecting to Postgres")?;
    info!(pool_size = cfg.env.postgres.pool_max_connections, "connected to Postgres");

    let schema_version: i64 = sqlx::query_scalar(
        "SELECT COUNT(*)::BIGINT FROM information_schema.tables WHERE table_schema = 'public'",
    )
    .fetch_one(&pool)
    .await
    .context("probing schema")?;
    info!(public_table_count = schema_version, "schema probe");

    if !args.source_sqlite.exists() {
        anyhow::bail!("source SQLite not found at {}", args.source_sqlite.display());
    }
    let sqlite_url = format!("sqlite:{}", args.source_sqlite.display());
    let sqlite_opts = SqliteConnectOptions::from_str(&sqlite_url)?
        .read_only(true)
        .immutable(true);
    let sqlite_pool = SqlitePoolOptions::new()
        .max_connections(1)
        .connect_with(sqlite_opts)
        .await
        .with_context(|| format!("opening source SQLite at {}", args.source_sqlite.display()))?;

    let payout_count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM payout")
        .fetch_one(&sqlite_pool)
        .await?;
    let reward_count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM reward")
        .fetch_one(&sqlite_pool)
        .await?;
    let events_count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM events")
        .fetch_one(&sqlite_pool)
        .await?;
    info!(
        payout_count,
        reward_count,
        events_count,
        "source SQLite opened — read-only"
    );

    info!(service = SERVICE, "foundation slice complete — actual import logic lands in S5");
    Ok(())
}

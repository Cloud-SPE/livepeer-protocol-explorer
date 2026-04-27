use anyhow::{Context, Result};
use clap::{Parser, Subcommand};
use livepeer_core::{
    abi::{self, AbiRegistration},
    config::Config,
    db, tracing_init,
};
use sqlx::sqlite::{SqliteConnectOptions, SqlitePoolOptions};
use std::path::PathBuf;
use std::str::FromStr;
use tracing::info;

const SERVICE: &str = "livepeer-seed-migrator";

#[derive(Parser, Debug)]
#[command(name = SERVICE, about = "One-shot bootstrap tool: ABI registry seed + SQLite price import.")]
struct Cli {
    /// Path to the static config (e.g. config/arbitrum.yaml)
    #[arg(long, env = "STATIC_CONFIG", default_value = "config/arbitrum.yaml", global = true)]
    static_config: PathBuf,

    /// Path to the env-specific config (e.g. config/env/dev.yaml)
    #[arg(long, env = "ENV_CONFIG", default_value = "config/env/dev.yaml", global = true)]
    env_config: PathBuf,

    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand, Debug)]
enum Command {
    /// Seed `contract_abi_registry` from vendored ABI JSON files. Idempotent.
    SeedAbiRegistry {
        #[arg(long, env = "ABI_DIR", default_value = "abi")]
        abi_dir: PathBuf,
    },
    /// Probe — boot-readiness check: schema present, ABI hashes match, SQLite reachable.
    Probe {
        #[arg(long, env = "SOURCE_SQLITE")]
        source_sqlite: PathBuf,
        #[arg(long, env = "ABI_DIR", default_value = "abi")]
        abi_dir: PathBuf,
    },
    /// (S5) Import seeded prices from the source SQLite into seeded_event_prices.
    Import {
        #[arg(long, env = "SOURCE_SQLITE")]
        source_sqlite: PathBuf,
    },
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_init::init("info");

    let cli = Cli::parse();
    let cfg = Config::load(&cli.static_config, &cli.env_config).context("loading config")?;
    let database_url = cfg.database_url().context("resolving DATABASE_URL")?;
    let pg = db::connect(&database_url, cfg.env.postgres.pool_max_connections)
        .await
        .context("connecting to Postgres")?;
    info!(
        chain = %cfg.static_.chain.name,
        chain_id = cfg.static_.chain.chain_id,
        "config + db ready"
    );

    match cli.command {
        Command::SeedAbiRegistry { abi_dir } => seed_abi_registry(&pg, &abi_dir, &cfg).await?,
        Command::Probe { source_sqlite, abi_dir } => probe(&pg, &source_sqlite, &abi_dir).await?,
        Command::Import { source_sqlite } => {
            anyhow::bail!(
                "Import not yet implemented (S5). Source SQLite would be: {}",
                source_sqlite.display()
            )
        }
    }
    Ok(())
}

async fn seed_abi_registry(pg: &sqlx::PgPool, abi_dir: &PathBuf, cfg: &Config) -> Result<()> {
    let genesis = cfg.static_.chain.livepeer_arbitrum_genesis_block as i64;

    // For v1 the registry is populated with the current Delta-version ABIs covering
    // [genesis, NULL]. SPEC §5.4. Per-block-range upgrade history is a v2 concern.
    let entries: &[(&str, &str, &str, bool)] = &[
        // (contract_name, proxy_address, abi_filename, strict_decode)
        ("Controller",     &cfg.static_.contracts.controller,      "Controller.json",       false),
        ("BondingManager", &cfg.static_.contracts.bonding_manager, "BondingManager.json",   true),
        ("TicketBroker",   &cfg.static_.contracts.ticket_broker,   "TicketBroker.json",     true),
        ("RoundsManager",  &cfg.static_.contracts.rounds_manager,  "RoundsManager.json",    false),
        ("LivepeerToken",  &cfg.static_.contracts.livepeer_token,  "LivepeerToken.json",    true),
        ("Minter",         &cfg.static_.contracts.minter,          "Minter.json",           false),
        ("Governor",       &cfg.static_.contracts.governor,        "LivepeerGovernor.json", false),
    ];

    let mut inserted = 0usize;
    let mut already = 0usize;
    for (name, proxy, fname, strict) in entries {
        let abi_path = abi_dir.join(fname);
        let abi_hash = abi::hash_file(&abi_path)?;
        // Target == proxy in v1 (Controller-resolved at boot would refine this; for the
        // initial seed we record proxy as both — refined when boot validation runs).
        let reg = AbiRegistration {
            contract_name: name.to_string(),
            proxy_address: proxy.to_lowercase(),
            target_address: proxy.to_lowercase(),
            from_block: genesis,
            to_block: None,
            abi_path: fname.to_string(),
            abi_hash,
            strict_decode: *strict,
        };
        // Detect "would have inserted" vs "already there"
        let exists: bool = sqlx::query_scalar(
            "SELECT EXISTS(SELECT 1 FROM contract_abi_registry WHERE contract_name=$1 AND from_block=$2)",
        )
        .bind(&reg.contract_name)
        .bind(reg.from_block)
        .fetch_one(pg)
        .await?;
        abi::upsert(pg, &reg).await?;
        if exists { already += 1 } else { inserted += 1 }
        info!(
            contract = %reg.contract_name,
            proxy = %reg.proxy_address,
            from_block = reg.from_block,
            strict = reg.strict_decode,
            abi_hash = %reg.abi_hash,
            already_present = exists,
            "registered ABI",
        );
    }
    info!(inserted, already, "abi registry seed complete");
    Ok(())
}

async fn probe(pg: &sqlx::PgPool, source_sqlite: &PathBuf, abi_dir: &PathBuf) -> Result<()> {
    let public_table_count: i64 = sqlx::query_scalar(
        "SELECT COUNT(*)::BIGINT FROM information_schema.tables WHERE table_schema = 'public'",
    )
    .fetch_one(pg)
    .await?;
    info!(public_table_count, "schema probe");

    // SPEC §5.5: every loaded ABI's hash is recomputed and must match the registry.
    let verified = livepeer_core::abi::verify_against_registry(pg, abi_dir).await?;
    info!(abi_count = verified.len(), "abi hash verification passed");

    if !source_sqlite.exists() {
        anyhow::bail!("source SQLite not found at {}", source_sqlite.display());
    }
    let sqlite_url = format!("sqlite:{}", source_sqlite.display());
    let sqlite_opts = SqliteConnectOptions::from_str(&sqlite_url)?
        .read_only(true)
        .immutable(true);
    let sqlite_pool = SqlitePoolOptions::new()
        .max_connections(1)
        .connect_with(sqlite_opts)
        .await
        .with_context(|| format!("opening source SQLite at {}", source_sqlite.display()))?;
    let payout: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM payout")
        .fetch_one(&sqlite_pool)
        .await?;
    let reward: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM reward")
        .fetch_one(&sqlite_pool)
        .await?;
    let events: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM events")
        .fetch_one(&sqlite_pool)
        .await?;
    info!(payout, reward, events, "source SQLite read-only");
    Ok(())
}

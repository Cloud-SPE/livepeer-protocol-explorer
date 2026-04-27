mod import;

use anyhow::{Context, Result};
use clap::{Parser, Subcommand};
use livepeer_core::{
    abi::{self, AbiRegistration},
    config::Config,
    db,
    rpc::{cross_check, Provider},
    tracing_init,
};
use sqlx::sqlite::{SqliteConnectOptions, SqlitePoolOptions};
use std::path::PathBuf;
use std::str::FromStr;
use tracing::{info, warn};

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
    /// Verify both RPC providers + cross-check + cache write. SPEC §13.2, §7.6, §16.2.
    VerifyRpc,
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
        Command::Import { source_sqlite } => run_import(&pg, &source_sqlite).await?,
        Command::VerifyRpc => verify_rpc(&pg, &cfg).await?,
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

async fn verify_rpc(pg: &sqlx::PgPool, cfg: &Config) -> Result<()> {
    let archive_url = cfg.archive_rpc_url().context("CHAINSTACK_RPC_URL")?;
    let secondary_url = cfg.secondary_rpc_url().context("SECONDARY_RPC_URL")?;
    let archive = Provider::new("chainstack", archive_url)?;
    let secondary = Provider::new("liveinfraspe", secondary_url)?;

    // 1. eth_chainId on both — must match the configured chain.
    let expected_chain = cfg.static_.chain.chain_id;
    let chain_a = archive.eth_chain_id().await?;
    let chain_b = secondary.eth_chain_id().await?;
    if chain_a != expected_chain || chain_b != expected_chain {
        anyhow::bail!(
            "chain_id mismatch: expected {expected_chain}, archive={chain_a}, secondary={chain_b}"
        );
    }
    info!(chain_id = expected_chain, "both providers report expected chain");

    // 2. eth_blockNumber on both. Heads can differ by 1–2 blocks (sequencing variance).
    let head_a = archive.eth_block_number().await?;
    let head_b = secondary.eth_block_number().await?;
    let delta = head_a.abs_diff(head_b);
    info!(
        archive_head = head_a,
        secondary_head = head_b,
        delta_blocks = delta,
        "block heads"
    );
    if delta > 5 {
        warn!(
            delta_blocks = delta,
            "providers > 5 blocks apart — investigate before backfill"
        );
    }

    // 3. Pin a recent block N — `min(both heads) - 32` for finality margin.
    let pin = head_a.min(head_b).saturating_sub(32);
    info!(pin_block = pin, "pinning cross-check block");

    // 4. Cross-check the block hash at the pinned block. Per the design note above
    //    cross_check_block_hash, raw-bytes compare on full headers fails on
    //    provider-specific optional-null fields (Chainstack emits requestsHash/withdrawals
    //    as null; liveinfraspe omits them). The load-bearing invariant is .hash equality.
    let canonical_hash =
        cross_check::cross_check_block_hash(pg, &archive, &secondary, pin).await?;
    info!(
        block = pin,
        canonical_hash = %canonical_hash,
        "cross-check passed: providers agree on block hash"
    );

    // 5. Single-source archive call: Chainlink ETH/USD latestRoundData() at the pinned block.
    let chainlink = &cfg.static_.pricing.chainlink_eth_usd_aggregator;
    let chainlink_outcome = cross_check::single_call_cached(
        pg,
        &archive,
        "eth_call",
        &serde_json::json!([
            { "to": chainlink, "data": "0xfeaf968c" },  // latestRoundData()
            format!("0x{:x}", pin),
        ]),
        Some(pin as i64),
    )
    .await?;
    let chainlink_result =
        std::str::from_utf8(&chainlink_outcome.response_bytes).unwrap_or_default();
    info!(
        call_hash = %chainlink_outcome.call_hash,
        response_hash = %chainlink_outcome.response_hash,
        result_preview = %&chainlink_result[..chainlink_result.len().min(80)],
        "archive-only: Chainlink ETH/USD latestRoundData() cached"
    );

    // 6. Single-source archive call: UniswapV3 LPT/WETH pool slot0() at the pinned block.
    let pool = &cfg.static_.pricing.uniswap_v3_lpt_weth_pool;
    let pool_outcome = cross_check::single_call_cached(
        pg,
        &archive,
        "eth_call",
        &serde_json::json!([
            { "to": pool, "data": "0x3850c7bd" },  // slot0()
            format!("0x{:x}", pin),
        ]),
        Some(pin as i64),
    )
    .await?;
    let cardinality = parse_slot0_cardinality(&pool_outcome.response_bytes);
    let required = cfg.static_.pricing.required_observation_cardinality;
    info!(
        call_hash = %pool_outcome.call_hash,
        response_hash = %pool_outcome.response_hash,
        observation_cardinality = cardinality,
        required,
        sufficient_for_twap = cardinality >= required,
        "archive-only: pool slot0() cached"
    );

    // Cache row count + divergence count for observability.
    let cache_rows: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM rpc_call_cache")
        .fetch_one(pg)
        .await?;
    let divergence_rows: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM rpc_divergence_failures WHERE resolved_at IS NULL",
    )
    .fetch_one(pg)
    .await?;
    info!(cache_rows, unresolved_divergences = divergence_rows, "verify-rpc complete");
    Ok(())
}

async fn run_import(pg: &sqlx::PgPool, source_sqlite: &PathBuf) -> Result<()> {
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
    info!(source = %source_sqlite.display(), "source SQLite open");

    let summary = import::run(pg, &sqlite_pool).await?;

    let payouts_total: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM seeded_event_prices WHERE event_type_hint = 'payout'",
    )
    .fetch_one(pg)
    .await?;
    let rewards_total: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM seeded_event_prices WHERE event_type_hint = 'reward'",
    )
    .fetch_one(pg)
    .await?;
    info!(
        payouts_seen = summary.payouts_seen,
        payouts_inserted_this_run = summary.payouts_inserted,
        payouts_total_in_postgres = payouts_total,
        rewards_seen = summary.rewards_seen,
        rewards_inserted_this_run = summary.rewards_inserted,
        rewards_total_in_postgres = rewards_total,
        "seed import complete"
    );
    Ok(())
}

/// Decode the `observationCardinality` field from a packed `slot0()` return value.
/// Layout: sqrtPriceX96 (uint160 → 32B), tick (int24 → 32B), observationIndex (uint16 → 32B),
/// observationCardinality (uint16 → 32B), ... Index 192..256 of the hex (after 0x).
fn parse_slot0_cardinality(response_bytes: &[u8]) -> u32 {
    let s = std::str::from_utf8(response_bytes).unwrap_or_default();
    let hex_str = s.trim_matches('"').trim_start_matches("0x");
    if hex_str.len() < 256 {
        return 0;
    }
    u32::from_str_radix(&hex_str[192..256], 16).unwrap_or(0)
}

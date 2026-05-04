use crate::{compare, import};
use anyhow::{Context, Result};
use livepeer_core::{
    abi::{self, AbiRegistration},
    config::Config,
    rpc::{cross_check, Provider},
};
use sqlx::PgPool;
use std::path::Path;

pub async fn seed_abi_registry(pg: &PgPool, abi_dir: &Path, cfg: &Config) -> Result<()> {
    let genesis = cfg.static_.chain.livepeer_arbitrum_genesis_block as i64;
    let entries: &[(&str, &str, &str, bool)] = &[
        ("Controller",     &cfg.static_.contracts.controller,      "Controller.json",       false),
        ("BondingManager", &cfg.static_.contracts.bonding_manager, "BondingManager.json",   true),
        ("TicketBroker",   &cfg.static_.contracts.ticket_broker,   "TicketBroker.json",     true),
        ("RoundsManager",  &cfg.static_.contracts.rounds_manager,  "RoundsManager.json",    false),
        ("LivepeerToken",  &cfg.static_.contracts.livepeer_token,  "LivepeerToken.json",    true),
        ("Minter",         &cfg.static_.contracts.minter,          "Minter.json",           false),
        ("Governor",       &cfg.static_.contracts.governor,        "LivepeerGovernor.json", false),
    ];
    for (name, proxy, fname, strict) in entries {
        let abi_path = abi_dir.join(fname);
        let abi_hash = abi::hash_file(&abi_path)?;
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
        abi::upsert(pg, &reg).await?;
    }
    Ok(())
}

pub async fn open_sqlite(path: &Path) -> Result<sqlx::sqlite::SqlitePool> {
    compare::open_sqlite(path).await
}

pub async fn run_import(pg: &PgPool, source_sqlite: &Path) -> Result<import::ImportSummary> {
    let sqlite_pool = open_sqlite(source_sqlite).await?;
    import::run(pg, &sqlite_pool).await
}

pub async fn run_cross_check(
    pg: &PgPool,
    source_sqlite: &Path,
) -> Result<compare::CrossCheckReport> {
    let sqlite = open_sqlite(source_sqlite).await?;
    compare::run_cross_check(pg, &sqlite).await
}

pub async fn probe(pg: &PgPool, source_sqlite: &Path, abi_dir: &Path) -> Result<()> {
    let _public_table_count: i64 = sqlx::query_scalar(
        "SELECT COUNT(*)::BIGINT FROM information_schema.tables WHERE table_schema = 'public'",
    )
    .fetch_one(pg)
    .await?;
    let _verified = livepeer_core::abi::verify_against_registry(pg, abi_dir).await?;
    let _sqlite = open_sqlite(source_sqlite).await?;
    Ok(())
}

pub async fn verify_rpc(pg: &PgPool, cfg: &Config) -> Result<()> {
    let archive_url = cfg.archive_rpc_url().context("CHAINSTACK_RPC_URL")?;
    let secondary_url = cfg.secondary_rpc_url().context("SECONDARY_RPC_URL")?;
    let archive = Provider::new("chainstack", archive_url)?;
    let secondary = Provider::new("liveinfraspe", secondary_url)?;

    let expected_chain = cfg.static_.chain.chain_id;
    let chain_a = archive.eth_chain_id().await?;
    let chain_b = secondary.eth_chain_id().await?;
    if chain_a != expected_chain || chain_b != expected_chain {
        anyhow::bail!(
            "chain_id mismatch: expected {expected_chain}, archive={chain_a}, secondary={chain_b}"
        );
    }

    let head_a = archive.eth_block_number().await?;
    let head_b = secondary.eth_block_number().await?;
    let pin = head_a.min(head_b).saturating_sub(32);
    let _canonical_hash =
        cross_check::cross_check_block_hash(pg, &archive, &secondary, pin).await?;

    let l2_pool = cfg.static_.pricing.uniswap_v3_lpt_weth_pool.to_lowercase();
    let required = cfg.static_.pricing.required_observation_cardinality;
    let slot0_params = serde_json::json!([
        {
            "to": l2_pool,
            "data": "0x3850c7bd"
        },
        format!("0x{:x}", pin),
    ]);
    let _pool_outcome =
        cross_check::single_call_cached(pg, &archive, "eth_call", &slot0_params, Some(pin as i64))
            .await?;
    let _cache_rows: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM rpc_call_cache")
        .fetch_one(pg)
        .await?;
    let _divergence_rows: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM rpc_divergence_failures WHERE resolved_at IS NULL",
    )
    .fetch_one(pg)
    .await?;
    let _ = required;
    Ok(())
}

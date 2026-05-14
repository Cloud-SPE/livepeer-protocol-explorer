//! Backfill ETH/USD into token_prices_by_block for historical LPT-priced blocks.
//!
//! The on-chain LPT pricing path reads Chainlink ETH/USD as an intermediate
//! step but, prior to the `price_lpt_amount` ETH-mirror change, did not persist
//! that read. For every block where we already have an `event_valuations` row
//! with `asset='LPT'` but no matching `token_prices_by_block` row for
//! `(ETH, USD, block)`, this pass re-reads Chainlink at that block and writes
//! the cache row. Idempotent: re-runs only touch blocks still missing a row.
//!
//! Determinism: reads go through `cross_check::batch_call_cached`, so the
//! second run for the same block hits `rpc_call_cache` (SPEC §1.4 / §13.5).

use crate::onchain::{
    chainlink_audit, decode_round_outcome, AggregatorV3, CHAINLINK_DECIMALS, STALENESS_FAIL_SECS,
};
use crate::persist::ARBITRUM_CHAIN_ID;
use alloy::sol_types::SolCall;
use anyhow::{Context, Result};
use bigdecimal::BigDecimal;
use livepeer_core::{config::Config, rpc::cross_check, rpc::Provider};
use sqlx::{PgPool, Row};
use std::str::FromStr;
use tracing::{info, warn};

const LOG_EVERY: usize = 1_000;

#[derive(Debug, Default)]
pub struct BackfillEthPricesSummary {
    pub blocks_considered: u64,
    pub priced: u64,
    pub failed_sequencer_outage: u64,
    pub failed_missing_oracle: u64,
}

#[derive(Debug)]
struct MissingBlock {
    block_number: i64,
    block_hash: String,
    block_timestamp: chrono::DateTime<chrono::Utc>,
}

pub async fn run(
    pg: &PgPool,
    archive: &Provider,
    cfg: &Config,
) -> Result<BackfillEthPricesSummary> {
    let chainlink = cfg.static_.pricing.chainlink_eth_usd_aggregator.clone();
    let sequencer = cfg.static_.pricing.l2_sequencer_uptime_feed.clone();
    let missing = fetch_missing_blocks(pg).await?;
    info!(
        candidates = missing.len(),
        chainlink = %chainlink,
        sequencer = %sequencer,
        "ETH/USD backfill: scanning blocks with LPT valuations but no ETH/USD cache row",
    );

    let mut summary = BackfillEthPricesSummary {
        blocks_considered: missing.len() as u64,
        ..Default::default()
    };

    for (i, blk) in missing.iter().enumerate() {
        if i > 0 && i % LOG_EVERY == 0 {
            info!(
                processed = i,
                total = missing.len(),
                priced = summary.priced,
                "backfill progress"
            );
        }
        match price_eth_at_block(pg, archive, &sequencer, &chainlink, blk).await? {
            BlockOutcome::Priced { price, raw_audit } => {
                insert_price(pg, blk, &chainlink, &price, raw_audit).await?;
                summary.priced += 1;
            }
            BlockOutcome::SequencerOutage => {
                warn!(block = blk.block_number, "skipping: sequencer outage");
                summary.failed_sequencer_outage += 1;
            }
            BlockOutcome::MissingOracle => {
                warn!(block = blk.block_number, "skipping: missing/stale oracle");
                summary.failed_missing_oracle += 1;
            }
        }
    }

    info!(?summary, "ETH/USD backfill complete");
    Ok(summary)
}

enum BlockOutcome {
    Priced {
        price: BigDecimal,
        raw_audit: serde_json::Value,
    },
    SequencerOutage,
    MissingOracle,
}

async fn price_eth_at_block(
    pg: &PgPool,
    archive: &Provider,
    sequencer: &str,
    chainlink: &str,
    blk: &MissingBlock,
) -> Result<BlockOutcome> {
    let round_calldata = AggregatorV3::latestRoundDataCall {}.abi_encode();
    let round_data_hex = format!("0x{}", alloy::hex::encode(&round_calldata));
    let block_param = format!("0x{:x}", blk.block_number);
    let batch = vec![
        (
            "eth_call".to_string(),
            serde_json::json!([{ "to": sequencer, "data": round_data_hex }, block_param.clone()]),
            Some(blk.block_number),
        ),
        (
            "eth_call".to_string(),
            serde_json::json!([{ "to": chainlink, "data": round_data_hex }, block_param.clone()]),
            Some(blk.block_number),
        ),
    ];
    let outcomes = cross_check::batch_call_cached(pg, archive, &batch).await?;
    // Per-call RPC errors (e.g. `execution reverted` — Chainlink reverts at
    // blocks before the aggregator's first round was published) collapse to
    // MissingOracle, the same way empty bytes do. Without this, a single
    // reverted call kills the whole backfill instead of skipping the block.
    if let Err(e) = outcomes[0].as_ref() {
        warn!(block = blk.block_number, error = %e, "skipping: sequencer eth_call errored");
        return Ok(BlockOutcome::MissingOracle);
    }
    if let Err(e) = outcomes[1].as_ref() {
        warn!(block = blk.block_number, error = %e, "skipping: chainlink eth_call errored");
        return Ok(BlockOutcome::MissingOracle);
    }
    let seq_res = decode_round_outcome(outcomes[0].as_ref())?;
    let cl_res = decode_round_outcome(outcomes[1].as_ref())?;

    let Some(seq) = seq_res else {
        return Ok(BlockOutcome::MissingOracle);
    };
    if seq.answer != "0" {
        return Ok(BlockOutcome::SequencerOutage);
    }
    let Some(cl) = cl_res else {
        return Ok(BlockOutcome::MissingOracle);
    };
    let round_id_u128: u128 = cl.round_id.parse().unwrap_or(0);
    let answered_u128: u128 = cl.answered_in_round.parse().unwrap_or(0);
    if answered_u128 < round_id_u128 {
        return Ok(BlockOutcome::MissingOracle);
    }
    let updated_at_secs: i64 = cl.updated_at.parse().unwrap_or(0);
    let block_ts = blk.block_timestamp.timestamp();
    if block_ts - updated_at_secs > STALENESS_FAIL_SECS {
        return Ok(BlockOutcome::MissingOracle);
    }
    let answer_int = i128::from_str(&cl.answer).context("decoding chainlink answer")?;
    if answer_int <= 0 {
        return Ok(BlockOutcome::MissingOracle);
    }
    let price = BigDecimal::from(answer_int) / BigDecimal::from(10u128.pow(CHAINLINK_DECIMALS));
    Ok(BlockOutcome::Priced {
        price,
        raw_audit: chainlink_audit(&cl),
    })
}

async fn insert_price(
    pg: &PgPool,
    blk: &MissingBlock,
    chainlink: &str,
    price: &BigDecimal,
    raw_audit: serde_json::Value,
) -> Result<()> {
    sqlx::query(
        r#"INSERT INTO token_prices_by_block
              (chain_id, asset, quote, block_number, block_hash, block_timestamp,
               price, source, pool_address, oracle_address, raw)
           VALUES ($1, 'ETH', 'USD', $2, $3, $4, $5, 'chainlink_eth_usd', NULL, $6, $7)
           ON CONFLICT (chain_id, asset, quote, block_number, source) DO NOTHING"#,
    )
    .bind(ARBITRUM_CHAIN_ID)
    .bind(blk.block_number)
    .bind(&blk.block_hash)
    .bind(blk.block_timestamp)
    .bind(price)
    .bind(chainlink)
    .bind(raw_audit)
    .execute(pg)
    .await
    .context("inserting backfilled ETH/USD price")?;
    Ok(())
}

async fn fetch_missing_blocks(pg: &PgPool) -> Result<Vec<MissingBlock>> {
    let rows = sqlx::query(
        r#"SELECT DISTINCT r.block_number, r.block_hash, r.block_timestamp
             FROM event_valuations v
             JOIN raw_protocol_events r ON r.id = v.event_id
            WHERE r.chain_id = $1
              AND v.asset = 'LPT'
              AND NOT EXISTS (
                SELECT 1 FROM token_prices_by_block tp
                 WHERE tp.chain_id = r.chain_id
                   AND tp.asset = 'ETH'
                   AND tp.quote = 'USD'
                   AND tp.block_number = r.block_number
              )
            ORDER BY r.block_number"#,
    )
    .bind(ARBITRUM_CHAIN_ID)
    .fetch_all(pg)
    .await?;
    Ok(rows
        .iter()
        .map(|r| MissingBlock {
            block_number: r.get(0),
            block_hash: r.get(1),
            block_timestamp: r.get(2),
        })
        .collect())
}

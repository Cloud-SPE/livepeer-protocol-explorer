//! Multi-asset valuation per SPEC §6.8 / §11.7. EarningsClaimed carries both an LPT
//! `rewards` portion and an ETH `fees` portion; raw_protocol_events holds one row
//! per log with `asset = NULL` and the breakdown in `raw_event.decoded`. The
//! valuator splits each into TWO `event_valuations` rows: one `asset='LPT'` and
//! one `asset='ETH'`, both keyed to the same `event_id`.
//!
//! Pricing per portion uses the same on-chain helpers as the single-asset paths
//! (`price_lpt_amount`, `price_eth_amount`). Sequencer + Chainlink reads are cached
//! across the two portions of a single event because `single_call_cached` keys on
//! `(method, params, block)` — both portions share the same block.

use crate::onchain::{
    price_eth_amount, price_lpt_amount, LptOutcome, PricingOutcome, DEGRADED_VERSION_SUFFIX,
};
use crate::persist::{insert_attempt, insert_valuation, ARBITRUM_CHAIN_ID, STATUS_PRICED};
use anyhow::{Context, Result};
use bigdecimal::{BigDecimal, Zero};
use livepeer_core::{config::Config, rpc::Provider};
use sqlx::{PgPool, Row};
use std::str::FromStr;
use tracing::{debug, info, warn};

const PRICING_METHOD_ETH: &str = "chainlink_eth_usd";
const SOURCE_ETH: &str = "chainlink_dual_rpc";
const PRICING_METHOD_LPT_TWAP: &str = "uniswap_v3_twap_30min_x_chainlink_eth";
const PRICING_METHOD_LPT_SPOT: &str = "uniswap_v3_spot_x_chainlink_eth";
const SOURCE_LPT: &str = "uniswap_v3_dual_rpc";
const LPT_DECIMALS: u32 = 18;
const ETH_DECIMALS: u32 = 18;

#[derive(Debug, Default)]
pub struct MultiAssetSummary {
    pub events_considered: u64,
    pub lpt_rows_priced: u64,
    pub eth_rows_priced: u64,
    pub lpt_zero_amount_rows: u64,
    pub eth_zero_amount_rows: u64,
    pub failures: u64,
}

#[derive(Debug)]
struct MultiAssetCandidate {
    event_id: i64,
    block_number: i64,
    block_timestamp: chrono::DateTime<chrono::Utc>,
    rewards_wei: String, // u256 as decimal string
    fees_wei: String,    // u256 as decimal string
}

pub async fn run_multi_asset_pass(
    pg: &PgPool,
    archive: &Provider,
    cfg: &Config,
    valuation_version: &str,
    include_tentative: bool,
) -> Result<MultiAssetSummary> {
    let candidates = fetch_candidates(pg, valuation_version, include_tentative).await?;
    info!(
        candidates = candidates.len(),
        valuation_version, include_tentative, "multi-asset pass starting"
    );

    let mut summary = MultiAssetSummary {
        events_considered: candidates.len() as u64,
        ..Default::default()
    };

    let pool = cfg.static_.pricing.uniswap_v3_lpt_weth_pool.clone();
    let chainlink = cfg.static_.pricing.chainlink_eth_usd_aggregator.clone();
    let sequencer = cfg.static_.pricing.l2_sequencer_uptime_feed.clone();

    for ev in &candidates {
        let block = ev.block_number as u64;
        let block_ts = ev.block_timestamp.timestamp();

        // Parse the wei amounts. BigDecimal handles arbitrary-precision integers.
        let rewards_wei = BigDecimal::from_str(&ev.rewards_wei).unwrap_or_default();
        let fees_wei = BigDecimal::from_str(&ev.fees_wei).unwrap_or_default();
        let rewards_lpt = &rewards_wei / BigDecimal::from(10u128.pow(LPT_DECIMALS));
        let fees_eth = &fees_wei / BigDecimal::from(10u128.pow(ETH_DECIMALS));

        // --- LPT portion (rewards) ---
        if rewards_lpt.is_zero() {
            // Per SPEC §6.8 we still record the row so the event has a complete
            // valuation set under this version. Skip the on-chain cost; price=0,
            // amount_usd=0. Source noted as "no_amount" so audit shows we didn't
            // hit the pool/oracle for it.
            insert_zero_row(
                pg, ev.event_id, valuation_version, "LPT",
                "no_amount", "no_amount", ev.block_number,
                serde_json::json!({"reason": "EarningsClaimed.rewards == 0"}),
            )
            .await?;
            summary.lpt_zero_amount_rows += 1;
        } else {
            match price_lpt_amount(pg, archive, &pool, &chainlink, &sequencer, block, block_ts, &rewards_lpt).await {
                Ok(LptOutcome::PricedTwap { native_usd_price, amount_usd, pricing_chain, version }) => {
                    commit(pg, ev.event_id, &version, "LPT", PRICING_METHOD_LPT_TWAP, SOURCE_LPT,
                           ev.block_number, &rewards_lpt, &native_usd_price, &amount_usd, &pricing_chain).await?;
                    summary.lpt_rows_priced += 1;
                    debug!(event_id = ev.event_id, rewards_lpt = %rewards_lpt, amount_usd = %amount_usd, "EarningsClaimed.rewards priced via TWAP");
                }
                Ok(LptOutcome::PricedDegraded { native_usd_price, amount_usd, pricing_chain, version }) => {
                    commit(pg, ev.event_id, &version, "LPT", PRICING_METHOD_LPT_SPOT, SOURCE_LPT,
                           ev.block_number, &rewards_lpt, &native_usd_price, &amount_usd, &pricing_chain).await?;
                    summary.lpt_rows_priced += 1;
                    warn!(event_id = ev.event_id, "EarningsClaimed.rewards priced via DEGRADED spot");
                }
                Ok(LptOutcome::SequencerOutage { detail }) => {
                    attempt(pg, ev.event_id, valuation_version, "LPT", "failed_sequencer_outage", Some(detail)).await?;
                    summary.failures += 1;
                }
                Ok(LptOutcome::MissingOracle { detail }) => {
                    attempt(pg, ev.event_id, valuation_version, "LPT", "failed_missing_oracle", Some(detail)).await?;
                    summary.failures += 1;
                }
                Ok(LptOutcome::MissingPool { detail }) => {
                    attempt(pg, ev.event_id, valuation_version, "LPT", "failed_missing_pool", Some(detail)).await?;
                    summary.failures += 1;
                }
                Err(e) => {
                    summary.failures += 1;
                    warn!(event_id = ev.event_id, error = %e, "EarningsClaimed.rewards pricing errored");
                }
            }
        }

        // --- ETH portion (fees) ---
        if fees_eth.is_zero() {
            insert_zero_row(
                pg, ev.event_id, valuation_version, "ETH",
                "no_amount", "no_amount", ev.block_number,
                serde_json::json!({"reason": "EarningsClaimed.fees == 0"}),
            )
            .await?;
            summary.eth_zero_amount_rows += 1;
        } else {
            match price_eth_amount(pg, archive, cfg, block, block_ts, &fees_eth).await {
                Ok(PricingOutcome::Priced { native_usd_price, amount_usd, pricing_chain }) => {
                    commit(pg, ev.event_id, valuation_version, "ETH", PRICING_METHOD_ETH, SOURCE_ETH,
                           ev.block_number, &fees_eth, &native_usd_price, &amount_usd, &pricing_chain).await?;
                    summary.eth_rows_priced += 1;
                    debug!(event_id = ev.event_id, fees_eth = %fees_eth, amount_usd = %amount_usd, "EarningsClaimed.fees priced via Chainlink");
                }
                Ok(PricingOutcome::SequencerOutage { detail }) => {
                    attempt(pg, ev.event_id, valuation_version, "ETH", "failed_sequencer_outage", Some(detail)).await?;
                    summary.failures += 1;
                }
                Ok(PricingOutcome::MissingOracle { detail }) => {
                    attempt(pg, ev.event_id, valuation_version, "ETH", "failed_missing_oracle", Some(detail)).await?;
                    summary.failures += 1;
                }
                Err(e) => {
                    summary.failures += 1;
                    warn!(event_id = ev.event_id, error = %e, "EarningsClaimed.fees pricing errored");
                }
            }
        }
    }

    info!(?summary, "multi-asset pass complete");
    Ok(summary)
}

async fn fetch_candidates(
    pg: &PgPool,
    valuation_version: &str,
    include_tentative: bool,
) -> Result<Vec<MultiAssetCandidate>> {
    let finality_filter = if include_tentative {
        ""
    } else {
        "AND r.finality = 'finalized'"
    };
    // Multi-asset events have asset=NULL on raw_protocol_events. Each event needs TWO
    // valuations (LPT + ETH); we consider it "complete" only when both rows exist.
    let degraded = format!("v1{DEGRADED_VERSION_SUFFIX}");
    let sql = format!(
        r#"WITH coverage AS (
              SELECT event_id,
                     bool_or(asset = 'LPT' AND valuation_version IN ($2, $3)) AS has_lpt,
                     bool_or(asset = 'ETH' AND valuation_version IN ($2, $3)) AS has_eth
                FROM event_valuations
               GROUP BY event_id
            )
            SELECT r.id, r.block_number, r.block_timestamp,
                   COALESCE(r.raw_event -> 'decoded' ->> 'rewards', '0') AS rewards_wei,
                   COALESCE(r.raw_event -> 'decoded' ->> 'fees',    '0') AS fees_wei
              FROM raw_protocol_events r
              LEFT JOIN coverage c ON c.event_id = r.id
             WHERE r.chain_id      = $1
               AND r.is_valuable   = TRUE
               AND r.is_canonical  = TRUE
               AND r.event_name    = 'EarningsClaimed'
               AND r.asset         IS NULL
               {finality_filter}
               AND (c.has_lpt IS NOT TRUE OR c.has_eth IS NOT TRUE)
             ORDER BY r.block_number, r.log_index"#
    );
    let rows = sqlx::query(&sql)
        .bind(ARBITRUM_CHAIN_ID)
        .bind(valuation_version)
        .bind(&degraded)
        .fetch_all(pg)
        .await?;
    Ok(rows
        .into_iter()
        .map(|r| MultiAssetCandidate {
            event_id: r.get(0),
            block_number: r.get(1),
            block_timestamp: r.get(2),
            rewards_wei: r.try_get(3).unwrap_or_else(|_| "0".to_string()),
            fees_wei: r.try_get(4).unwrap_or_else(|_| "0".to_string()),
        })
        .collect())
}

#[allow(clippy::too_many_arguments)]
async fn commit(
    pg: &PgPool,
    event_id: i64,
    valuation_version: &str,
    asset: &str,
    pricing_method: &str,
    source: &str,
    block_number: i64,
    amount_native: &BigDecimal,
    native_usd_price: &BigDecimal,
    amount_usd: &BigDecimal,
    pricing_chain: &serde_json::Value,
) -> Result<()> {
    let mut tx = pg.begin().await?;
    insert_valuation(
        &mut tx, event_id, valuation_version, asset,
        pricing_method, source, STATUS_PRICED,
        block_number, amount_native, native_usd_price, amount_usd, pricing_chain,
    )
    .await
    .with_context(|| format!("insert_valuation event_id={event_id} asset={asset}"))?;
    insert_attempt(&mut tx, event_id, valuation_version, asset, STATUS_PRICED, None).await?;
    tx.commit().await?;
    Ok(())
}

#[allow(clippy::too_many_arguments)]
async fn insert_zero_row(
    pg: &PgPool,
    event_id: i64,
    valuation_version: &str,
    asset: &str,
    pricing_method: &str,
    source: &str,
    block_number: i64,
    pricing_chain: serde_json::Value,
) -> Result<()> {
    let zero = BigDecimal::from(0u64);
    commit(
        pg, event_id, valuation_version, asset, pricing_method, source,
        block_number, &zero, &zero, &zero, &pricing_chain,
    ).await
}

async fn attempt(
    pg: &PgPool,
    event_id: i64,
    valuation_version: &str,
    asset: &str,
    result_status: &str,
    error_detail: Option<serde_json::Value>,
) -> Result<()> {
    let mut tx = pg.begin().await?;
    insert_attempt(&mut tx, event_id, valuation_version, asset, result_status, error_detail).await?;
    tx.commit().await?;
    Ok(())
}

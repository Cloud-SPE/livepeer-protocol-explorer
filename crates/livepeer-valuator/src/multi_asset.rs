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

use crate::bulk::BulkBuffers;
use crate::onchain::{
    permanent_lpt_failure_detail, price_eth_amount, price_lpt_amount, LptOutcome, PricingOutcome,
    DEGRADED_VERSION_SUFFIX,
};
use crate::persist::{
    ARBITRUM_CHAIN_ID, STATUS_FAILED_MISSING_ORACLE, STATUS_FAILED_MISSING_POOL,
    STATUS_FAILED_SEQUENCER_OUTAGE, STATUS_PRICED,
};
use anyhow::Result;
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
    block_hash: String,
    block_timestamp: chrono::DateTime<chrono::Utc>,
    rewards_wei: String, // u256 as decimal string
    fees_wei: String,    // u256 as decimal string
    finalized_at: Option<chrono::DateTime<chrono::Utc>>,
}

pub async fn run_multi_asset_pass(
    pg: &PgPool,
    archive: &Provider,
    cfg: &Config,
    valuation_version: &str,
    include_tentative: bool,
) -> Result<MultiAssetSummary> {
    let cursor_key = crate::cursor::pass_key(valuation_version, "MULTI");
    let floor = crate::cursor::scan_floor(
        pg,
        valuation_version,
        &cursor_key,
        crate::cursor::DEFAULT_LOOKBACK_SECS,
        include_tentative,
    )
    .await?;
    let candidates = fetch_candidates(pg, valuation_version, include_tentative, floor).await?;
    info!(
        candidates = candidates.len(),
        valuation_version, include_tentative, "multi-asset pass starting"
    );

    let mut summary = MultiAssetSummary {
        events_considered: candidates.len() as u64,
        ..Default::default()
    };
    // Oldest finalized_at left unresolved via a transient failure on either half.
    let mut min_unresolved: Option<chrono::DateTime<chrono::Utc>> = None;

    let pool = cfg.static_.pricing.uniswap_v3_lpt_weth_pool.clone();
    let chainlink = cfg.static_.pricing.chainlink_eth_usd_aggregator.clone();
    let sequencer = cfg.static_.pricing.l2_sequencer_uptime_feed.clone();
    let mut buffers = BulkBuffers::new();

    for ev in &candidates {
        let block = ev.block_number as u64;
        // Pins the cursor if either half hits a *transient* (retryable) failure.
        let mut ev_transient = false;

        // Parse the wei amounts. BigDecimal handles arbitrary-precision integers.
        let rewards_wei = BigDecimal::from_str(&ev.rewards_wei).unwrap_or_default();
        let fees_wei = BigDecimal::from_str(&ev.fees_wei).unwrap_or_default();
        let rewards_lpt = &rewards_wei / BigDecimal::from(10u128.pow(LPT_DECIMALS));
        let fees_eth = &fees_wei / BigDecimal::from(10u128.pow(ETH_DECIMALS));

        // --- LPT portion (rewards) ---
        if rewards_lpt.is_zero() {
            // SPEC §6.8: still record a row so the event has a complete valuation
            // set; price=0, amount_usd=0, source noted.
            push_zero_row(
                &mut buffers,
                ev.event_id,
                valuation_version,
                "LPT",
                "no_amount",
                "no_amount",
                ev.block_number,
                serde_json::json!({"reason": "EarningsClaimed.rewards == 0"}),
            );
            summary.lpt_zero_amount_rows += 1;
        } else {
            match price_lpt_amount(
                pg,
                archive,
                &pool,
                &chainlink,
                &sequencer,
                block,
                &ev.block_hash,
                ev.block_timestamp,
                &rewards_lpt,
            )
            .await
            {
                Ok((
                    LptOutcome::PricedTwap {
                        native_usd_price,
                        amount_usd,
                        pricing_chain,
                        version,
                    },
                    prices,
                )) => {
                    for p in prices {
                        buffers.push_price(p);
                    }
                    buffers.push_priced(
                        ARBITRUM_CHAIN_ID,
                        ev.event_id,
                        &version,
                        "LPT",
                        PRICING_METHOD_LPT_TWAP,
                        SOURCE_LPT,
                        ev.block_number,
                        &rewards_lpt,
                        &native_usd_price,
                        &amount_usd,
                        &pricing_chain,
                        STATUS_PRICED,
                    );
                    summary.lpt_rows_priced += 1;
                    debug!(event_id = ev.event_id, rewards_lpt = %rewards_lpt, amount_usd = %amount_usd, "EarningsClaimed.rewards priced via TWAP");
                }
                Ok((
                    LptOutcome::PricedDegraded {
                        native_usd_price,
                        amount_usd,
                        pricing_chain,
                        version,
                    },
                    prices,
                )) => {
                    for p in prices {
                        buffers.push_price(p);
                    }
                    buffers.push_priced(
                        ARBITRUM_CHAIN_ID,
                        ev.event_id,
                        &version,
                        "LPT",
                        PRICING_METHOD_LPT_SPOT,
                        SOURCE_LPT,
                        ev.block_number,
                        &rewards_lpt,
                        &native_usd_price,
                        &amount_usd,
                        &pricing_chain,
                        STATUS_PRICED,
                    );
                    summary.lpt_rows_priced += 1;
                    warn!(
                        event_id = ev.event_id,
                        "EarningsClaimed.rewards priced via DEGRADED spot"
                    );
                }
                Ok((LptOutcome::SequencerOutage { detail }, _)) => {
                    buffers.push_failed_outcome(
                        ARBITRUM_CHAIN_ID,
                        ev.event_id,
                        valuation_version,
                        "LPT",
                        PRICING_METHOD_LPT_TWAP,
                        SOURCE_LPT,
                        ev.block_number,
                        &rewards_lpt,
                        STATUS_FAILED_SEQUENCER_OUTAGE,
                        detail,
                    );
                    summary.failures += 1;
                }
                Ok((LptOutcome::MissingOracle { detail }, _)) => {
                    buffers.push_failed_outcome(
                        ARBITRUM_CHAIN_ID,
                        ev.event_id,
                        valuation_version,
                        "LPT",
                        PRICING_METHOD_LPT_TWAP,
                        SOURCE_LPT,
                        ev.block_number,
                        &rewards_lpt,
                        STATUS_FAILED_MISSING_ORACLE,
                        detail,
                    );
                    summary.failures += 1;
                }
                Ok((LptOutcome::MissingPool { detail }, _)) => {
                    buffers.push_failed_outcome(
                        ARBITRUM_CHAIN_ID,
                        ev.event_id,
                        valuation_version,
                        "LPT",
                        PRICING_METHOD_LPT_TWAP,
                        SOURCE_LPT,
                        ev.block_number,
                        &rewards_lpt,
                        STATUS_FAILED_MISSING_POOL,
                        detail,
                    );
                    summary.failures += 1;
                }
                Err(e) => {
                    if let Some(detail) = permanent_lpt_failure_detail(&e) {
                        buffers.push_failed_outcome(
                            ARBITRUM_CHAIN_ID,
                            ev.event_id,
                            valuation_version,
                            "LPT",
                            PRICING_METHOD_LPT_TWAP,
                            SOURCE_LPT,
                            ev.block_number,
                            &rewards_lpt,
                            STATUS_FAILED_MISSING_POOL,
                            detail,
                        );
                    } else {
                        ev_transient = true;
                    }
                    summary.failures += 1;
                    warn!(event_id = ev.event_id, error = %e, "EarningsClaimed.rewards pricing errored");
                }
            }
        }

        // --- ETH portion (fees) ---
        if fees_eth.is_zero() {
            push_zero_row(
                &mut buffers,
                ev.event_id,
                valuation_version,
                "ETH",
                "no_amount",
                "no_amount",
                ev.block_number,
                serde_json::json!({"reason": "EarningsClaimed.fees == 0"}),
            );
            summary.eth_zero_amount_rows += 1;
        } else {
            match price_eth_amount(
                pg,
                archive,
                cfg,
                block,
                &ev.block_hash,
                ev.block_timestamp,
                &fees_eth,
            )
            .await
            {
                Ok((
                    PricingOutcome::Priced {
                        native_usd_price,
                        amount_usd,
                        pricing_chain,
                    },
                    prices,
                )) => {
                    for p in prices {
                        buffers.push_price(p);
                    }
                    buffers.push_priced(
                        ARBITRUM_CHAIN_ID,
                        ev.event_id,
                        valuation_version,
                        "ETH",
                        PRICING_METHOD_ETH,
                        SOURCE_ETH,
                        ev.block_number,
                        &fees_eth,
                        &native_usd_price,
                        &amount_usd,
                        &pricing_chain,
                        STATUS_PRICED,
                    );
                    summary.eth_rows_priced += 1;
                    debug!(event_id = ev.event_id, fees_eth = %fees_eth, amount_usd = %amount_usd, "EarningsClaimed.fees priced via Chainlink");
                }
                Ok((PricingOutcome::SequencerOutage { detail }, _)) => {
                    buffers.push_failed_outcome(
                        ARBITRUM_CHAIN_ID,
                        ev.event_id,
                        valuation_version,
                        "ETH",
                        PRICING_METHOD_ETH,
                        SOURCE_ETH,
                        ev.block_number,
                        &fees_eth,
                        STATUS_FAILED_SEQUENCER_OUTAGE,
                        detail,
                    );
                    summary.failures += 1;
                }
                Ok((PricingOutcome::MissingOracle { detail }, _)) => {
                    buffers.push_failed_outcome(
                        ARBITRUM_CHAIN_ID,
                        ev.event_id,
                        valuation_version,
                        "ETH",
                        PRICING_METHOD_ETH,
                        SOURCE_ETH,
                        ev.block_number,
                        &fees_eth,
                        STATUS_FAILED_MISSING_ORACLE,
                        detail,
                    );
                    summary.failures += 1;
                }
                Err(e) => {
                    summary.failures += 1;
                    ev_transient = true;
                    warn!(event_id = ev.event_id, error = %e, "EarningsClaimed.fees pricing errored");
                }
            }
        }

        if ev_transient {
            if let Some(fa) = ev.finalized_at {
                min_unresolved = Some(min_unresolved.map_or(fa, |m| m.min(fa)));
            }
        }
        buffers.maybe_flush(pg).await?;
    }
    buffers.flush(pg).await?;

    let frontier = crate::cursor::frontier_multi(pg, ARBITRUM_CHAIN_ID).await?;
    crate::cursor::advance(pg, &cursor_key, min_unresolved, frontier, include_tentative).await?;

    info!(?summary, "multi-asset pass complete");
    Ok(summary)
}

async fn fetch_candidates(
    pg: &PgPool,
    valuation_version: &str,
    include_tentative: bool,
    floor: chrono::DateTime<chrono::Utc>,
) -> Result<Vec<MultiAssetCandidate>> {
    // The finalized_at floor ($4) is applied only when NOT include_tentative.
    let (finality_filter, floor_filter) = if include_tentative {
        ("", "")
    } else {
        ("AND r.finality = 'finalized'", "AND r.finalized_at >= $4")
    };
    // Multi-asset events have asset=NULL on raw_protocol_events. Each event needs TWO
    // valuations (LPT + ETH); we consider it "complete" only when both rows exist.
    let degraded = format!("v1{DEGRADED_VERSION_SUFFIX}");
    let sql = format!(
        r#"SELECT r.id, r.block_number, r.block_hash, r.block_timestamp,
                   COALESCE(r.raw_event -> 'decoded' ->> 'rewards', '0') AS rewards_wei,
                   COALESCE(r.raw_event -> 'decoded' ->> 'fees',    '0') AS fees_wei,
                   r.finalized_at
              FROM raw_protocol_events r
             WHERE r.chain_id      = $1
               AND r.is_valuable   = TRUE
               AND r.is_canonical  = TRUE
               AND r.event_name    = 'EarningsClaimed'
               AND r.asset         IS NULL
               {finality_filter}
               {floor_filter}
               AND (
                    (
                        NOT EXISTS (
                            SELECT 1
                              FROM event_valuations v
                             WHERE v.event_id          = r.id
                               AND v.asset             = 'LPT'
                               AND v.valuation_version IN ($2, $3)
                        )
                        AND NOT EXISTS (
                            SELECT 1
                              FROM valuation_attempts a
                             WHERE a.event_id          = r.id
                               AND a.asset             = 'LPT'
                               AND a.valuation_version IN ($2, $3)
                               AND a.result_status     LIKE 'failed_%'
                        )
                    )
                    OR (
                        NOT EXISTS (
                            SELECT 1
                              FROM event_valuations v
                             WHERE v.event_id          = r.id
                               AND v.asset             = 'ETH'
                               AND v.valuation_version IN ($2, $3)
                        )
                        AND NOT EXISTS (
                            SELECT 1
                              FROM valuation_attempts a
                             WHERE a.event_id          = r.id
                               AND a.asset             = 'ETH'
                               AND a.valuation_version IN ($2, $3)
                               AND a.result_status     LIKE 'failed_%'
                        )
                    )
               )
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
            block_hash: r.get(2),
            block_timestamp: r.get(3),
            rewards_wei: r.try_get(4).unwrap_or_else(|_| "0".to_string()),
            fees_wei: r.try_get(5).unwrap_or_else(|_| "0".to_string()),
        })
        .collect())
}

#[allow(clippy::too_many_arguments)]
fn push_zero_row(
    buffers: &mut BulkBuffers,
    event_id: i64,
    valuation_version: &str,
    asset: &str,
    pricing_method: &str,
    source: &str,
    block_number: i64,
    pricing_chain: serde_json::Value,
) {
    let zero = BigDecimal::from(0u64);
    buffers.push_priced(
        ARBITRUM_CHAIN_ID,
        event_id,
        valuation_version,
        asset,
        pricing_method,
        source,
        block_number,
        &zero,
        &zero,
        &zero,
        &pricing_chain,
        STATUS_PRICED,
    );
}

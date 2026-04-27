//! On-chain pricing path. SPEC §7.3.
//!
//! S8.2.a covers ETH-valued events: read Chainlink ETH/USD aggregator + sequencer
//! uptime feed at the event block, validate, write `event_valuations`. Reads go
//! through `cross_check::single_call_cached` so the deterministic-replay invariant
//! (SPEC §1.4 / §13.5) holds — second run reads from `rpc_call_cache`.
//!
//! S8.2.b will add LPT-valued events via Uniswap V3 TWAP (`observe([1800, 0])`)
//! plus the cardinality precheck and the `v1_degraded_spot_pre_cardinality`
//! fallback per Q-OD-9 (~17K events in the pre-cardinality window need it).

use crate::persist::{insert_attempt, insert_valuation, ARBITRUM_CHAIN_ID, STATUS_PRICED};
use crate::tick_math;
use alloy::primitives::U256;
use alloy::sol;
use alloy::sol_types::SolCall;
use anyhow::{Context, Result};
use bigdecimal::BigDecimal;
use livepeer_core::{
    config::Config,
    rpc::{cross_check, BlockTag, Provider},
};
use sqlx::{PgPool, Row};
use std::str::FromStr;
use tracing::{debug, info, warn};

const CHAINLINK_DECIMALS: u32 = 8;
const PRICING_METHOD_ETH: &str = "chainlink_eth_usd";
const SOURCE_ETH: &str = "chainlink_dual_rpc";

const PRICING_METHOD_LPT_TWAP: &str = "uniswap_v3_twap_30min_x_chainlink_eth";
const PRICING_METHOD_LPT_SPOT: &str = "uniswap_v3_spot_x_chainlink_eth";
const SOURCE_LPT: &str = "uniswap_v3_dual_rpc";

const TWAP_WINDOW_SECS: u32 = 1_800;
const REQUIRED_CARDINALITY: u32 = 144;

// Hardcoded sentinels per SPEC §7.3.3 / §7.3.4.
const STALENESS_FAIL_SECS: i64 = 86_400;
const STALENESS_WARN_SECS: i64 = 14_400;

// 2^192 as a decimal string for use in BigDecimal math (sqrtPriceX96^2 / 2^192 → price).
const TWO_POW_192_DECIMAL: &str =
    "6277101735386680763835789423207666416102355444464034512896";

/// Degraded-version stamp for events priced before pool cardinality crossed 144.
/// Q-OD-9: the LPT/WETH pool stayed at cardinality 1–2 from deployment through
/// block ~33M, so events in that window cannot use 30-min TWAP. We still produce a
/// valuation, but the version makes the downgrade queryable.
pub const DEGRADED_VERSION_SUFFIX: &str = "_degraded_spot_pre_cardinality";

// Both Chainlink AggregatorV3 and the sequencer-uptime feed share the
// `latestRoundData()` selector and ABI.
sol! {
    #[allow(missing_docs)]
    interface AggregatorV3 {
        function latestRoundData()
            external view
            returns (uint80 roundId, int256 answer, uint256 startedAt, uint256 updatedAt, uint80 answeredInRound);
    }
}

sol! {
    #[allow(missing_docs)]
    interface UniswapV3Pool {
        function slot0() external view returns (
            uint160 sqrtPriceX96, int24 tick, uint16 observationIndex,
            uint16 observationCardinality, uint16 observationCardinalityNext,
            uint8 feeProtocol, bool unlocked
        );
        function observe(uint32[] calldata secondsAgos)
            external view returns (
                int56[] memory tickCumulatives,
                uint160[] memory secondsPerLiquidityCumulativeX128
            );
    }
}

#[derive(Debug, Default)]
pub struct OnChainRunSummary {
    pub events_considered: u64,
    pub priced: u64,
    pub failed_sequencer_outage: u64,
    pub failed_missing_oracle: u64,
    pub other_skipped: u64,
}

#[derive(Debug)]
struct CandidateEvent {
    event_id: i64,
    block_number: i64,
    block_timestamp: chrono::DateTime<chrono::Utc>,
    asset: Option<String>,
    amount_normalized: Option<BigDecimal>,
}

/// Walk all unvalued, valuable, canonical, ETH-valued events at the requested
/// `valuation_version` and price each via Chainlink.
pub async fn run_onchain_pass_eth(
    pg: &PgPool,
    archive: &Provider,
    cfg: &Config,
    valuation_version: &str,
    include_tentative: bool,
) -> Result<OnChainRunSummary> {
    let candidates = fetch_eth_candidates(pg, valuation_version, include_tentative).await?;
    info!(
        candidates = candidates.len(),
        valuation_version, include_tentative, "on-chain ETH pass starting"
    );

    let mut summary = OnChainRunSummary {
        events_considered: candidates.len() as u64,
        ..Default::default()
    };

    for ev in &candidates {
        let asset = ev.asset.as_deref().unwrap_or_default();
        if asset != "ETH" {
            summary.other_skipped += 1;
            continue;
        }
        let Some(amount_native) = ev.amount_normalized.clone() else {
            summary.other_skipped += 1;
            continue;
        };

        match price_eth_event(pg, archive, cfg, ev, &amount_native).await {
            Ok(PricingOutcome::Priced { native_usd_price, amount_usd, pricing_chain }) => {
                let mut tx = pg.begin().await?;
                let inserted = insert_valuation(
                    &mut tx,
                    ev.event_id,
                    valuation_version,
                    asset,
                    PRICING_METHOD_ETH,
                    SOURCE_ETH,
                    STATUS_PRICED,
                    ev.block_number,
                    &amount_native,
                    &native_usd_price,
                    &amount_usd,
                    &pricing_chain,
                )
                .await?;
                insert_attempt(&mut tx, ev.event_id, valuation_version, asset, STATUS_PRICED, None).await?;
                tx.commit().await?;
                if inserted {
                    summary.priced += 1;
                    debug!(
                        event_id = ev.event_id,
                        block = ev.block_number,
                        amount_native = %amount_native,
                        native_usd_price = %native_usd_price,
                        amount_usd = %amount_usd,
                        "priced via Chainlink"
                    );
                }
            }
            Ok(PricingOutcome::SequencerOutage { detail }) => {
                let mut tx = pg.begin().await?;
                insert_attempt(
                    &mut tx,
                    ev.event_id,
                    valuation_version,
                    asset,
                    "failed_sequencer_outage",
                    Some(detail),
                )
                .await?;
                tx.commit().await?;
                summary.failed_sequencer_outage += 1;
                warn!(event_id = ev.event_id, block = ev.block_number, "failed_sequencer_outage");
            }
            Ok(PricingOutcome::MissingOracle { detail }) => {
                let mut tx = pg.begin().await?;
                insert_attempt(
                    &mut tx,
                    ev.event_id,
                    valuation_version,
                    asset,
                    "failed_missing_oracle",
                    Some(detail),
                )
                .await?;
                tx.commit().await?;
                summary.failed_missing_oracle += 1;
                warn!(event_id = ev.event_id, block = ev.block_number, "failed_missing_oracle");
            }
            Err(e) => {
                summary.other_skipped += 1;
                warn!(event_id = ev.event_id, error = %e, "on-chain pricing failed; will retry next run");
            }
        }
    }

    info!(?summary, "on-chain ETH pass complete");
    Ok(summary)
}

pub(crate) enum PricingOutcome {
    Priced {
        native_usd_price: BigDecimal,
        amount_usd: BigDecimal,
        pricing_chain: serde_json::Value,
    },
    SequencerOutage {
        detail: serde_json::Value,
    },
    MissingOracle {
        detail: serde_json::Value,
    },
}

async fn price_eth_event(
    pg: &PgPool,
    archive: &Provider,
    cfg: &Config,
    ev: &CandidateEvent,
    amount_native: &BigDecimal,
) -> Result<PricingOutcome> {
    price_eth_amount(
        pg,
        archive,
        cfg,
        ev.block_number as u64,
        ev.block_timestamp.timestamp(),
        amount_native,
    )
    .await
}

/// Pure ETH-on-chain pricing helper: same Chainlink+sequencer reads as the
/// per-event flow, parameterized by `(block, block_ts, amount_native)` so the
/// multi-asset path can call it for the ETH portion of an EarningsClaimed.
pub(crate) async fn price_eth_amount(
    pg: &PgPool,
    archive: &Provider,
    cfg: &Config,
    block: u64,
    block_ts: i64,
    amount_native: &BigDecimal,
) -> Result<PricingOutcome> {

    // 1. Sequencer uptime — read at event block. answer == 0 means UP.
    let seq_round = read_round(pg, archive, &cfg.static_.pricing.l2_sequencer_uptime_feed, block).await?;
    if seq_round.answer != "0" {
        return Ok(PricingOutcome::SequencerOutage {
            detail: serde_json::json!({
                "feed":       cfg.static_.pricing.l2_sequencer_uptime_feed,
                "block":      block,
                "answer":     seq_round.answer,
                "startedAt":  seq_round.started_at,
                "note":       "sequencer answer != 0; per SPEC §7.3.4 do not trust on-chain prices in this period"
            }),
        });
    }

    // 2. Chainlink ETH/USD at event block.
    let cl = read_round(pg, archive, &cfg.static_.pricing.chainlink_eth_usd_aggregator, block).await?;

    // 3. Mandatory checks (§7.3.3): answeredInRound >= roundId, staleness ≤ 86400s.
    let round_id_u128: u128 = cl.round_id.parse().unwrap_or(0);
    let answered_u128: u128 = cl.answered_in_round.parse().unwrap_or(0);
    if answered_u128 < round_id_u128 {
        return Ok(PricingOutcome::MissingOracle {
            detail: serde_json::json!({
                "reason": "answeredInRound < roundId",
                "roundId": cl.round_id,
                "answeredInRound": cl.answered_in_round,
                "block": block,
            }),
        });
    }
    let updated_at_secs: i64 = cl.updated_at.parse().unwrap_or(0);
    let staleness = block_ts - updated_at_secs;
    if staleness > STALENESS_FAIL_SECS {
        return Ok(PricingOutcome::MissingOracle {
            detail: serde_json::json!({
                "reason": "staleness > 24h",
                "staleness_secs": staleness,
                "updatedAt": updated_at_secs,
                "block_ts": block_ts,
                "block": block,
            }),
        });
    }
    if staleness > STALENESS_WARN_SECS {
        warn!(staleness, block, "Chainlink staleness > 4h");
    }

    // 4. Decode price. Chainlink ETH/USD has 8 decimals.
    let answer_int = i128::from_str(&cl.answer).context("decoding chainlink answer")?;
    if answer_int <= 0 {
        return Ok(PricingOutcome::MissingOracle {
            detail: serde_json::json!({
                "reason": "non-positive answer",
                "answer": cl.answer,
            }),
        });
    }
    let native_usd_price = BigDecimal::from(answer_int)
        / BigDecimal::from(10u128.pow(CHAINLINK_DECIMALS));
    let amount_usd = amount_native * &native_usd_price;

    // 5. pricing_chain provenance per SPEC §7.5.
    let pricing_chain = serde_json::json!({
        "steps": [
            {
                "asset": "ETH",
                "quote": "USD",
                "price": native_usd_price.to_string(),
                "source": "chainlink",
                "oracle": cfg.static_.pricing.chainlink_eth_usd_aggregator,
                "block_number": block,
                "raw_round": {
                    "roundId":         cl.round_id,
                    "answer":          cl.answer,
                    "startedAt":       cl.started_at,
                    "updatedAt":       cl.updated_at,
                    "answeredInRound": cl.answered_in_round,
                },
            }
        ],
        "result": {
            "asset": "ETH",
            "quote": "USD",
            "price": native_usd_price.to_string(),
            "amount_native": amount_native.to_string(),
            "amount_usd":    amount_usd.to_string(),
        },
        "checks": {
            "sequencer_up":            true,
            "answered_in_round_ok":    true,
            "staleness_secs":          staleness,
            "staleness_warn":          staleness > STALENESS_WARN_SECS,
        },
    });

    Ok(PricingOutcome::Priced {
        native_usd_price,
        amount_usd,
        pricing_chain,
    })
}

#[derive(Debug)]
struct DecodedRound {
    round_id: String,
    answer: String,
    started_at: String,
    updated_at: String,
    answered_in_round: String,
}

async fn read_round(
    pg: &PgPool,
    archive: &Provider,
    aggregator: &str,
    block: u64,
) -> Result<DecodedRound> {
    let calldata = AggregatorV3::latestRoundDataCall {};
    let data = format!("0x{}", alloy::hex::encode(calldata.abi_encode()));
    let outcome = cross_check::single_call_cached(
        pg,
        archive,
        "eth_call",
        &serde_json::json!([{ "to": aggregator, "data": data }, format!("0x{:x}", block)]),
        Some(block as i64),
    )
    .await?;
    // response_bytes is a JSON-encoded "0x..." string; strip quotes + 0x, hex-decode.
    let s = std::str::from_utf8(&outcome.response_bytes).unwrap_or_default();
    let hex_str = s.trim_matches('"').trim_start_matches("0x");
    let raw = alloy::hex::decode(hex_str).context("decoding eth_call return hex")?;
    let ret = AggregatorV3::latestRoundDataCall::abi_decode_returns(&raw, true)
        .context("ABI-decoding latestRoundData return tuple")?;
    Ok(DecodedRound {
        round_id: ret.roundId.to_string(),
        answer: ret.answer.to_string(),
        started_at: ret.startedAt.to_string(),
        updated_at: ret.updatedAt.to_string(),
        answered_in_round: ret.answeredInRound.to_string(),
    })
}

async fn fetch_eth_candidates(
    pg: &PgPool,
    valuation_version: &str,
    include_tentative: bool,
) -> Result<Vec<CandidateEvent>> {
    let finality_filter = if include_tentative {
        ""
    } else {
        "AND r.finality = 'finalized'"
    };
    let sql = format!(
        r#"SELECT r.id, r.block_number, r.block_timestamp, r.asset, r.amount_normalized
             FROM raw_protocol_events r
             LEFT JOIN event_valuations v
               ON v.event_id          = r.id
              AND v.valuation_version = $2
              AND (v.asset = r.asset OR (v.asset IS NULL AND r.asset IS NULL))
            WHERE r.chain_id      = $1
              AND r.is_valuable   = TRUE
              AND r.is_canonical  = TRUE
              AND r.asset         = 'ETH'
              {finality_filter}
              AND v.event_id IS NULL
            ORDER BY r.block_number, r.log_index"#
    );
    let rows = sqlx::query(&sql)
        .bind(ARBITRUM_CHAIN_ID)
        .bind(valuation_version)
        .fetch_all(pg)
        .await?;
    Ok(rows
        .into_iter()
        .map(|r| CandidateEvent {
            event_id: r.get(0),
            block_number: r.get(1),
            block_timestamp: r.get(2),
            asset: r.get(3),
            amount_normalized: r.get(4),
        })
        .collect())
}

// Suppress any future unused-import warnings.
#[allow(dead_code)]
fn _block_tag_assert(b: BlockTag) -> BlockTag {
    b
}

// ---------------------------------------------------------------------------
// S8.2.b — LPT path (Uniswap V3 TWAP × Chainlink, with degraded-spot fallback)
// ---------------------------------------------------------------------------

#[derive(Debug, Default)]
pub struct LptRunSummary {
    pub events_considered: u64,
    pub priced_twap: u64,
    pub priced_degraded: u64,
    pub failed_sequencer_outage: u64,
    pub failed_missing_oracle: u64,
    pub failed_missing_pool: u64,
    pub other_skipped: u64,
}

pub async fn run_onchain_pass_lpt(
    pg: &PgPool,
    archive: &Provider,
    cfg: &Config,
    valuation_version: &str,
    include_tentative: bool,
) -> Result<LptRunSummary> {
    let mut summary = LptRunSummary::default();

    // Walk all unvalued LPT events (TWAP version + degraded version both gated by
    // separate keys in event_valuations, so a degraded run later won't conflict).
    let candidates = fetch_lpt_candidates(pg, valuation_version, include_tentative).await?;
    summary.events_considered = candidates.len() as u64;
    info!(
        candidates = candidates.len(),
        valuation_version, include_tentative, "on-chain LPT pass starting"
    );

    let pool = cfg.static_.pricing.uniswap_v3_lpt_weth_pool.clone();
    let chainlink = cfg.static_.pricing.chainlink_eth_usd_aggregator.clone();
    let sequencer = cfg.static_.pricing.l2_sequencer_uptime_feed.clone();

    for ev in &candidates {
        let asset = ev.asset.as_deref().unwrap_or_default();
        if asset != "LPT" {
            summary.other_skipped += 1;
            continue;
        }
        let Some(amount_native) = ev.amount_normalized.clone() else {
            summary.other_skipped += 1;
            continue;
        };

        match price_lpt_event(pg, archive, &pool, &chainlink, &sequencer, ev, &amount_native).await {
            Ok(LptOutcome::PricedTwap { native_usd_price, amount_usd, pricing_chain, version }) => {
                commit_priced(
                    pg,
                    ev.event_id,
                    &version,
                    asset,
                    PRICING_METHOD_LPT_TWAP,
                    SOURCE_LPT,
                    ev.block_number,
                    &amount_native,
                    &native_usd_price,
                    &amount_usd,
                    &pricing_chain,
                )
                .await?;
                summary.priced_twap += 1;
                debug!(
                    event_id = ev.event_id,
                    amount_usd = %amount_usd,
                    native_usd_price = %native_usd_price,
                    "priced LPT via TWAP"
                );
            }
            Ok(LptOutcome::PricedDegraded { native_usd_price, amount_usd, pricing_chain, version }) => {
                commit_priced(
                    pg,
                    ev.event_id,
                    &version,
                    asset,
                    PRICING_METHOD_LPT_SPOT,
                    SOURCE_LPT,
                    ev.block_number,
                    &amount_native,
                    &native_usd_price,
                    &amount_usd,
                    &pricing_chain,
                )
                .await?;
                summary.priced_degraded += 1;
                warn!(
                    event_id = ev.event_id,
                    amount_usd = %amount_usd,
                    "priced LPT via DEGRADED spot (cardinality < 144)"
                );
            }
            Ok(LptOutcome::SequencerOutage { detail }) => {
                attempt_only(pg, ev.event_id, valuation_version, asset, "failed_sequencer_outage", Some(detail)).await?;
                summary.failed_sequencer_outage += 1;
            }
            Ok(LptOutcome::MissingOracle { detail }) => {
                attempt_only(pg, ev.event_id, valuation_version, asset, "failed_missing_oracle", Some(detail)).await?;
                summary.failed_missing_oracle += 1;
            }
            Ok(LptOutcome::MissingPool { detail }) => {
                attempt_only(pg, ev.event_id, valuation_version, asset, "failed_missing_pool", Some(detail)).await?;
                summary.failed_missing_pool += 1;
            }
            Err(e) => {
                summary.other_skipped += 1;
                warn!(event_id = ev.event_id, error = %e, "LPT pricing failed; will retry next run");
            }
        }
    }

    info!(?summary, "on-chain LPT pass complete");
    Ok(summary)
}

pub(crate) enum LptOutcome {
    PricedTwap {
        native_usd_price: BigDecimal,
        amount_usd: BigDecimal,
        pricing_chain: serde_json::Value,
        version: String,
    },
    PricedDegraded {
        native_usd_price: BigDecimal,
        amount_usd: BigDecimal,
        pricing_chain: serde_json::Value,
        version: String,
    },
    SequencerOutage { detail: serde_json::Value },
    MissingOracle { detail: serde_json::Value },
    MissingPool { detail: serde_json::Value },
}

async fn price_lpt_event(
    pg: &PgPool,
    archive: &Provider,
    pool: &str,
    chainlink: &str,
    sequencer: &str,
    ev: &CandidateEvent,
    amount_native: &BigDecimal,
) -> Result<LptOutcome> {
    price_lpt_amount(
        pg,
        archive,
        pool,
        chainlink,
        sequencer,
        ev.block_number as u64,
        ev.block_timestamp.timestamp(),
        amount_native,
    )
    .await
}

/// Pure LPT-on-chain pricing helper for the multi-asset path. Takes raw
/// `(block, block_ts, amount_native)` instead of a CandidateEvent.
#[allow(clippy::too_many_arguments)]
pub(crate) async fn price_lpt_amount(
    pg: &PgPool,
    archive: &Provider,
    pool: &str,
    chainlink: &str,
    sequencer: &str,
    block: u64,
    block_ts: i64,
    amount_native: &BigDecimal,
) -> Result<LptOutcome> {

    // 1. Sequencer up?
    let seq = read_round(pg, archive, sequencer, block).await?;
    if seq.answer != "0" {
        return Ok(LptOutcome::SequencerOutage {
            detail: serde_json::json!({
                "feed": sequencer, "block": block,
                "answer": seq.answer, "startedAt": seq.started_at,
            }),
        });
    }

    // 2. Pool slot0 — check cardinality & whether the pool exists yet at this block.
    let slot0 = match read_pool_slot0(pg, archive, pool, block).await? {
        Some(s) => s,
        None => {
            return Ok(LptOutcome::MissingPool {
                detail: serde_json::json!({
                    "pool": pool, "block": block,
                    "reason": "slot0() returned empty bytes — pool not yet deployed",
                }),
            });
        }
    };

    let cardinality = slot0.observation_cardinality;
    let chain_for_log = serde_json::json!({
        "pool": pool, "block": block,
        "sqrtPriceX96": slot0.sqrt_price_x96.to_string(),
        "tick": slot0.tick,
        "observationCardinality": cardinality,
    });

    // 3. Chainlink ETH/USD — same path as ETH events.
    let cl = read_round(pg, archive, chainlink, block).await?;
    let round_id_u128: u128 = cl.round_id.parse().unwrap_or(0);
    let answered_u128: u128 = cl.answered_in_round.parse().unwrap_or(0);
    if answered_u128 < round_id_u128 {
        return Ok(LptOutcome::MissingOracle {
            detail: serde_json::json!({
                "reason": "answeredInRound < roundId",
                "roundId": cl.round_id, "answeredInRound": cl.answered_in_round,
            }),
        });
    }
    let updated_at_secs: i64 = cl.updated_at.parse().unwrap_or(0);
    let staleness = block_ts - updated_at_secs;
    if staleness > STALENESS_FAIL_SECS {
        return Ok(LptOutcome::MissingOracle {
            detail: serde_json::json!({
                "reason": "chainlink staleness > 24h",
                "staleness_secs": staleness, "updatedAt": updated_at_secs, "block_ts": block_ts,
            }),
        });
    }
    let answer_int = i128::from_str(&cl.answer).context("decoding chainlink answer")?;
    if answer_int <= 0 {
        return Ok(LptOutcome::MissingOracle {
            detail: serde_json::json!({"reason": "non-positive answer", "answer": cl.answer}),
        });
    }
    let eth_usd_price = BigDecimal::from(answer_int) / BigDecimal::from(10u128.pow(CHAINLINK_DECIMALS));

    // 4. Pick TWAP or degraded path.
    let two_pow_192 = BigDecimal::from_str(TWO_POW_192_DECIMAL).unwrap();

    if cardinality < REQUIRED_CARDINALITY {
        // Degraded: spot from sqrtPriceX96.
        let lpt_per_weth = sqrt_price_x96_to_price(&slot0.sqrt_price_x96, &two_pow_192);
        let lpt_usd = &lpt_per_weth * &eth_usd_price;
        let amount_usd = amount_native * &lpt_usd;
        let pricing_chain = serde_json::json!({
            "steps": [
                { "asset": "LPT", "quote": "WETH",
                  "price": lpt_per_weth.to_string(),
                  "source": "uniswap_v3_spot",
                  "pool": pool, "block_number": block,
                  "raw_slot0": chain_for_log,
                  "note": "DEGRADED — pool cardinality below required 144 (Q-OD-9)" },
                { "asset": "WETH", "quote": "USD",
                  "price": eth_usd_price.to_string(),
                  "source": "chainlink", "oracle": chainlink, "block_number": block,
                  "raw_round": chainlink_audit(&cl) },
            ],
            "result": {
                "asset": "LPT", "quote": "USD",
                "price": lpt_usd.to_string(),
                "amount_native": amount_native.to_string(),
                "amount_usd": amount_usd.to_string(),
            },
            "checks": {
                "sequencer_up": true,
                "answered_in_round_ok": true,
                "staleness_secs": staleness,
                "cardinality": cardinality,
                "cardinality_required": REQUIRED_CARDINALITY,
                "twap_window_secs": TWAP_WINDOW_SECS,
            },
        });
        let version = format!(
            "{}{}",
            // Strip any prefix the operator may pass; the degraded suffix replaces the
            // canonical TWAP qualifier.
            "v1",
            DEGRADED_VERSION_SUFFIX
        );
        return Ok(LptOutcome::PricedDegraded {
            native_usd_price: lpt_usd,
            amount_usd,
            pricing_chain,
            version,
        });
    }

    // TWAP path.
    let twap = match read_pool_observe(pg, archive, pool, block, TWAP_WINDOW_SECS).await? {
        Some(t) => t,
        None => {
            return Ok(LptOutcome::MissingPool {
                detail: serde_json::json!({
                    "pool": pool, "block": block,
                    "reason": "observe() failed — pool may lack observations spanning the TWAP window",
                }),
            });
        }
    };
    let avg_tick = uniswap_arithmetic_mean_tick(twap.cumulative_now - twap.cumulative_then, TWAP_WINDOW_SECS as i64);
    let avg_tick_i32: i32 = i32::try_from(avg_tick).context("avg_tick out of i32 range")?;
    let sqrt_price_x96 = tick_math::get_sqrt_ratio_at_tick(avg_tick_i32)?;
    let lpt_per_weth = sqrt_price_x96_to_price(&sqrt_price_x96, &two_pow_192);
    let lpt_usd = &lpt_per_weth * &eth_usd_price;
    let amount_usd = amount_native * &lpt_usd;

    let pricing_chain = serde_json::json!({
        "steps": [
            { "asset": "LPT", "quote": "WETH",
              "price": lpt_per_weth.to_string(),
              "source": "uniswap_v3_twap_30min",
              "pool": pool, "block_number": block,
              "raw_observe": {
                  "secondsAgos": [TWAP_WINDOW_SECS, 0],
                  "tickCumulativeThen": twap.cumulative_then.to_string(),
                  "tickCumulativeNow":  twap.cumulative_now.to_string(),
                  "delta":              (twap.cumulative_now - twap.cumulative_then).to_string(),
                  "arithmeticMeanTick": avg_tick,
                  "sqrtPriceX96":       sqrt_price_x96.to_string(),
              } },
            { "asset": "WETH", "quote": "USD",
              "price": eth_usd_price.to_string(),
              "source": "chainlink", "oracle": chainlink, "block_number": block,
              "raw_round": chainlink_audit(&cl) },
        ],
        "result": {
            "asset": "LPT", "quote": "USD",
            "price": lpt_usd.to_string(),
            "amount_native": amount_native.to_string(),
            "amount_usd": amount_usd.to_string(),
        },
        "checks": {
            "sequencer_up": true,
            "answered_in_round_ok": true,
            "staleness_secs": staleness,
            "cardinality": cardinality,
            "twap_window_secs": TWAP_WINDOW_SECS,
        },
    });

    let version = "v1_lpt_weth_twap_30min_x_chainlink_eth".to_string();
    Ok(LptOutcome::PricedTwap {
        native_usd_price: lpt_usd,
        amount_usd,
        pricing_chain,
        version,
    })
}

fn chainlink_audit(cl: &DecodedRound) -> serde_json::Value {
    serde_json::json!({
        "roundId":         cl.round_id,
        "answer":          cl.answer,
        "startedAt":       cl.started_at,
        "updatedAt":       cl.updated_at,
        "answeredInRound": cl.answered_in_round,
    })
}

/// price (token1/token0) = sqrtPriceX96^2 / 2^192 in BigDecimal terms.
/// Token0=LPT, token1=WETH at the LPT/WETH pool, both 18 decimals → no decimal correction.
fn sqrt_price_x96_to_price(sqrt: &U256, two_pow_192: &BigDecimal) -> BigDecimal {
    let s = BigDecimal::from_str(&sqrt.to_string()).expect("U256 to BigDecimal");
    (&s * &s) / two_pow_192
}

/// Uniswap V3 OracleLibrary's arithmetic-mean tick: floor (toward -∞) division.
/// Solidity `int / int` rounds toward 0; for negative deltas not evenly divisible
/// we subtract 1 to match floor semantics.
fn uniswap_arithmetic_mean_tick(delta: i128, seconds: i64) -> i64 {
    let secs = seconds as i128;
    let mut q = delta / secs;
    if delta < 0 && delta % secs != 0 {
        q -= 1;
    }
    q as i64
}

#[derive(Debug)]
struct PoolSlot0 {
    sqrt_price_x96: U256,
    tick: i32,
    observation_cardinality: u32,
}

async fn read_pool_slot0(
    pg: &PgPool,
    archive: &Provider,
    pool: &str,
    block: u64,
) -> Result<Option<PoolSlot0>> {
    let calldata = UniswapV3Pool::slot0Call {};
    let data = format!("0x{}", alloy::hex::encode(calldata.abi_encode()));
    let outcome = cross_check::single_call_cached(
        pg, archive, "eth_call",
        &serde_json::json!([{ "to": pool, "data": data }, format!("0x{:x}", block)]),
        Some(block as i64),
    ).await?;
    let s = std::str::from_utf8(&outcome.response_bytes).unwrap_or_default();
    let hex_str = s.trim_matches('"').trim_start_matches("0x");
    if hex_str.is_empty() {
        return Ok(None); // pool not yet deployed at this block
    }
    let raw = alloy::hex::decode(hex_str).context("decoding slot0() return hex")?;
    let ret = UniswapV3Pool::slot0Call::abi_decode_returns(&raw, true)
        .context("ABI-decoding slot0() return tuple")?;
    let tick_i32: i32 = ret.tick.to_string().parse().context("tick to i32")?;
    Ok(Some(PoolSlot0 {
        sqrt_price_x96: U256::from(ret.sqrtPriceX96),
        tick: tick_i32,
        observation_cardinality: ret.observationCardinality as u32,
    }))
}

#[derive(Debug)]
struct PoolObservation {
    cumulative_then: i128,
    cumulative_now: i128,
}

async fn read_pool_observe(
    pg: &PgPool,
    archive: &Provider,
    pool: &str,
    block: u64,
    seconds_ago: u32,
) -> Result<Option<PoolObservation>> {
    let calldata = UniswapV3Pool::observeCall {
        secondsAgos: vec![seconds_ago, 0],
    };
    let data = format!("0x{}", alloy::hex::encode(calldata.abi_encode()));
    let outcome = cross_check::single_call_cached(
        pg, archive, "eth_call",
        &serde_json::json!([{ "to": pool, "data": data }, format!("0x{:x}", block)]),
        Some(block as i64),
    ).await?;
    let s = std::str::from_utf8(&outcome.response_bytes).unwrap_or_default();
    let hex_str = s.trim_matches('"').trim_start_matches("0x");
    if hex_str.is_empty() {
        return Ok(None);
    }
    let raw = alloy::hex::decode(hex_str).context("decoding observe() return hex")?;
    let ret = match UniswapV3Pool::observeCall::abi_decode_returns(&raw, true) {
        Ok(r) => r,
        Err(_) => return Ok(None), // pool may revert (OLD) for windows it can't serve
    };
    if ret.tickCumulatives.len() < 2 {
        return Ok(None);
    }
    let cumulative_then: i128 = ret.tickCumulatives[0].to_string().parse().context("tickCumulative[0]")?;
    let cumulative_now: i128 = ret.tickCumulatives[1].to_string().parse().context("tickCumulative[1]")?;
    Ok(Some(PoolObservation { cumulative_then, cumulative_now }))
}

async fn fetch_lpt_candidates(
    pg: &PgPool,
    valuation_version: &str,
    include_tentative: bool,
) -> Result<Vec<CandidateEvent>> {
    let finality_filter = if include_tentative {
        ""
    } else {
        "AND r.finality = 'finalized'"
    };
    // Match either the canonical version OR its degraded sibling — once an event has
    // been priced under either, it's done.
    let degraded = format!("v1{DEGRADED_VERSION_SUFFIX}");
    let sql = format!(
        r#"SELECT r.id, r.block_number, r.block_timestamp, r.asset, r.amount_normalized
             FROM raw_protocol_events r
             LEFT JOIN event_valuations v
               ON v.event_id  = r.id
              AND v.asset     = r.asset
              AND v.valuation_version IN ($2, $3)
            WHERE r.chain_id     = $1
              AND r.is_valuable  = TRUE
              AND r.is_canonical = TRUE
              AND r.asset        = 'LPT'
              {finality_filter}
              AND v.event_id IS NULL
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
        .map(|r| CandidateEvent {
            event_id: r.get(0),
            block_number: r.get(1),
            block_timestamp: r.get(2),
            asset: r.get(3),
            amount_normalized: r.get(4),
        })
        .collect())
}

#[allow(clippy::too_many_arguments)]
async fn commit_priced(
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
    .await?;
    insert_attempt(&mut tx, event_id, valuation_version, asset, STATUS_PRICED, None).await?;
    tx.commit().await?;
    Ok(())
}

async fn attempt_only(
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

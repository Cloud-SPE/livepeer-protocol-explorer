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

// Hardcoded sentinels per SPEC §7.3.3 / §7.3.4.
const STALENESS_FAIL_SECS: i64 = 86_400;
const STALENESS_WARN_SECS: i64 = 14_400;

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

enum PricingOutcome {
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
    let block = ev.block_number as u64;
    let block_ts = ev.block_timestamp.timestamp();

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
        warn!(staleness, event_id = ev.event_id, "Chainlink staleness > 4h");
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

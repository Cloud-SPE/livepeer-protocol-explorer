//! SQLite → Postgres trusted-price import. Implements SPEC §8 + design-doc rules.
//!
//! Per Q-OD-1 (lossy LPT precision): we record SQLite f64 values as-is via
//! BigDecimal::from_f64. Per SPEC §8.7 the valuator re-derives `amount_native` from
//! RPC at valuation time; only `*_usd` and `*_price` from SQLite are load-bearing.
//!
//! Per Q-OD-2 (transaction_id unique within payout / reward): seeded rows use
//! `log_index = -1` sentinel. PK collisions on re-run are no-ops (`ON CONFLICT DO NOTHING`).

use anyhow::{Context, Result};
use bigdecimal::{BigDecimal, FromPrimitive};
use serde::Serialize;
use sqlx::sqlite::SqlitePool;
use sqlx::{PgPool, QueryBuilder, Row};
use tracing::info;

pub const ARBITRUM_CHAIN_ID: i64 = 42161;
pub const SEED_LOG_INDEX_SENTINEL: i32 = -1;
pub const BATCH_SIZE: i64 = 1000;

/// One imported row's reportable counts, post-import.
#[derive(Debug)]
pub struct ImportSummary {
    pub payouts_seen: i64,
    pub payouts_inserted: u64,
    pub rewards_seen: i64,
    pub rewards_inserted: u64,
}

/// Import payouts and rewards. Each table is one transaction. Idempotent.
pub async fn run(pg: &PgPool, sqlite: &SqlitePool) -> Result<ImportSummary> {
    let (payouts_seen, payouts_inserted) = import_payouts(pg, sqlite).await?;
    info!(payouts_seen, payouts_inserted, "payout import complete");

    let (rewards_seen, rewards_inserted) = import_rewards(pg, sqlite).await?;
    info!(rewards_seen, rewards_inserted, "reward import complete");

    Ok(ImportSummary {
        payouts_seen,
        payouts_inserted,
        rewards_seen,
        rewards_inserted,
    })
}

#[derive(Debug, Serialize, Clone)]
struct PayoutRow {
    transaction_id: String,
    timestamp: Option<String>,
    face_value: f64,
    face_value_usd: f64,
    recipient_id: Option<String>,
    eth_price: f64,
    orch_commission: Option<f64>,
    orch_commission_usd: Option<f64>,
    fee_cut: Option<f64>,
    transaction_fee: Option<f64>,
    sender_id: Option<String>,
}

#[derive(Debug, Serialize, Clone)]
struct RewardRow {
    transaction_id: String,
    eth_address: Option<String>,
    timestamp: Option<String>,
    total_tokens: f64,
    orch_tokens: Option<f64>,
    orch_tokens_usd: f64,
    reward_cut: Option<f64>,
    transaction_fee: Option<f64>,
    transaction_fee_usd: Option<f64>,
    eth_price: Option<f64>,
    lpt_price: f64,
}

async fn import_payouts(pg: &PgPool, sqlite: &SqlitePool) -> Result<(i64, u64)> {
    let total: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM payout")
        .fetch_one(sqlite)
        .await
        .context("counting payouts")?;
    info!(total, batch_size = BATCH_SIZE, "starting payout import");

    let mut tx = pg.begin().await?;
    let mut inserted = 0u64;
    let mut offset = 0i64;

    while offset < total {
        // CAST every numeric column to REAL — SQLite stores whole-number values as
        // INTEGER affinity even when the column is declared NUMBER, and sqlx rejects
        // INTEGER↔f64 decoding. This forces normalization to REAL on the SQLite side.
        let rows = sqlx::query(
            r#"SELECT transaction_id,
                       timestamp,
                       CAST(face_value         AS REAL) AS face_value,
                       CAST(face_value_usd     AS REAL) AS face_value_usd,
                       recipient_id,
                       CAST(eth_price          AS REAL) AS eth_price,
                       CAST(orch_commission    AS REAL) AS orch_commission,
                       CAST(orch_commission_usd AS REAL) AS orch_commission_usd,
                       CAST(fee_cut            AS REAL) AS fee_cut,
                       CAST(transaction_fee    AS REAL) AS transaction_fee,
                       sender_id
                 FROM payout
                ORDER BY transaction_id
                LIMIT ?1 OFFSET ?2"#,
        )
        .bind(BATCH_SIZE)
        .bind(offset)
        .fetch_all(sqlite)
        .await
        .with_context(|| format!("payout fetch at offset {offset}"))?;

        if rows.is_empty() {
            break;
        }

        let payouts: Vec<PayoutRow> = rows
            .into_iter()
            .map(|r| PayoutRow {
                transaction_id: r.get::<String, _>("transaction_id"),
                timestamp: r.try_get("timestamp").ok(),
                face_value: r.get("face_value"),
                face_value_usd: r.get("face_value_usd"),
                recipient_id: r.try_get("recipient_id").ok(),
                eth_price: r.get("eth_price"),
                orch_commission: r.try_get("orch_commission").ok(),
                orch_commission_usd: r.try_get("orch_commission_usd").ok(),
                fee_cut: r.try_get("fee_cut").ok(),
                transaction_fee: r.try_get("transaction_fee").ok(),
                sender_id: r.try_get("sender_id").ok(),
            })
            .collect();

        let mut qb = QueryBuilder::new(
            "INSERT INTO seeded_event_prices \
             (chain_id, tx_hash, log_index, event_type_hint, asset, \
              amount_native, amount_usd, asset_usd_price, source, raw) ",
        );
        qb.push_values(payouts.iter(), |mut b, row| {
            b.push_bind(ARBITRUM_CHAIN_ID);
            b.push_bind(row.transaction_id.to_lowercase());
            b.push_bind(SEED_LOG_INDEX_SENTINEL);
            b.push_bind("payout");
            b.push_bind("ETH");
            b.push_bind(BigDecimal::from_f64(row.face_value).unwrap_or_default());
            b.push_bind(BigDecimal::from_f64(row.face_value_usd).unwrap_or_default());
            b.push_bind(BigDecimal::from_f64(row.eth_price).unwrap_or_default());
            b.push_bind("trusted_historical_seed_v1");
            b.push_bind(serde_json::to_value(row).unwrap());
        });
        qb.push(" ON CONFLICT (chain_id, tx_hash, log_index, asset) DO NOTHING");

        let result = qb.build().execute(&mut *tx).await?;
        inserted += result.rows_affected();
        offset += BATCH_SIZE;

        if (offset / BATCH_SIZE) % 25 == 0 {
            info!(progress_offset = offset, total, "payout progress");
        }
    }

    tx.commit().await?;
    Ok((total, inserted))
}

async fn import_rewards(pg: &PgPool, sqlite: &SqlitePool) -> Result<(i64, u64)> {
    let total: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM reward")
        .fetch_one(sqlite)
        .await
        .context("counting rewards")?;
    info!(total, batch_size = BATCH_SIZE, "starting reward import");

    let mut tx = pg.begin().await?;
    let mut inserted = 0u64;
    let mut offset = 0i64;

    while offset < total {
        let rows = sqlx::query(
            r#"SELECT transaction_id,
                       eth_address,
                       timestamp,
                       CAST(total_tokens         AS REAL) AS total_tokens,
                       CAST(orch_tokens          AS REAL) AS orch_tokens,
                       CAST(orch_tokens_usd      AS REAL) AS orch_tokens_usd,
                       CAST(reward_cut           AS REAL) AS reward_cut,
                       CAST(transaction_fee      AS REAL) AS transaction_fee,
                       CAST(transaction_fee_usd  AS REAL) AS transaction_fee_usd,
                       CAST(eth_price            AS REAL) AS eth_price,
                       CAST(lpt_price            AS REAL) AS lpt_price
                 FROM reward
                ORDER BY transaction_id
                LIMIT ?1 OFFSET ?2"#,
        )
        .bind(BATCH_SIZE)
        .bind(offset)
        .fetch_all(sqlite)
        .await
        .with_context(|| format!("reward fetch at offset {offset}"))?;

        if rows.is_empty() {
            break;
        }

        let rewards: Vec<RewardRow> = rows
            .into_iter()
            .map(|r| RewardRow {
                transaction_id: r.get::<String, _>("transaction_id"),
                eth_address: r.try_get("eth_address").ok(),
                timestamp: r.try_get("timestamp").ok(),
                total_tokens: r.get("total_tokens"),
                orch_tokens: r.try_get("orch_tokens").ok(),
                orch_tokens_usd: r.get("orch_tokens_usd"),
                reward_cut: r.try_get("reward_cut").ok(),
                transaction_fee: r.try_get("transaction_fee").ok(),
                transaction_fee_usd: r.try_get("transaction_fee_usd").ok(),
                eth_price: r.try_get("eth_price").ok(),
                lpt_price: r.get("lpt_price"),
            })
            .collect();

        let mut qb = QueryBuilder::new(
            "INSERT INTO seeded_event_prices \
             (chain_id, tx_hash, log_index, event_type_hint, asset, \
              amount_native, amount_usd, asset_usd_price, source, raw) ",
        );
        qb.push_values(rewards.iter(), |mut b, row| {
            b.push_bind(ARBITRUM_CHAIN_ID);
            b.push_bind(row.transaction_id.to_lowercase());
            b.push_bind(SEED_LOG_INDEX_SENTINEL);
            b.push_bind("reward");
            b.push_bind("LPT");
            b.push_bind(BigDecimal::from_f64(row.total_tokens).unwrap_or_default());
            b.push_bind(BigDecimal::from_f64(row.orch_tokens_usd).unwrap_or_default());
            b.push_bind(BigDecimal::from_f64(row.lpt_price).unwrap_or_default());
            b.push_bind("trusted_historical_seed_v1");
            b.push_bind(serde_json::to_value(row).unwrap());
        });
        qb.push(" ON CONFLICT (chain_id, tx_hash, log_index, asset) DO NOTHING");

        let result = qb.build().execute(&mut *tx).await?;
        inserted += result.rows_affected();
        offset += BATCH_SIZE;

        if (offset / BATCH_SIZE) % 25 == 0 {
            info!(progress_offset = offset, total, "reward progress");
        }
    }

    tx.commit().await?;
    Ok((total, inserted))
}

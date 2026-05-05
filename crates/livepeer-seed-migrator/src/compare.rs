//! TD-004 / SPEC §24.1 — seed/canonical event cross-check.
//!
//! Walks every (tx_hash, log_index) present in EITHER store within the indexed
//! window, and reports discrepancies:
//!   - missing_in_indexer: the SQLite seed has the log but raw_protocol_events doesn't
//!   - missing_in_seed:    raw_protocol_events has the log but SQLite seed doesn't
//!   - block_number_mismatch: both stores have it but block_number differs
//!   - block_hash_mismatch:   both stores have it but block_hash differs
//!
//! Per the S6.1 finding: the SQLite `events` table has duplicate rows for some
//! on-chain logs. We dedupe by `(tx_hash, payload.log_index)` before joining.

use anyhow::{Context, Result};
use serde::Serialize;
use sqlx::sqlite::SqlitePool;
use sqlx::{PgPool, Row};
use std::collections::HashMap;
use tracing::{info, warn};

#[derive(Debug, Default, Serialize)]
pub struct CrossCheckReport {
    pub indexer_event_count: u64,
    pub seed_event_count_in_window: u64,
    pub matched: u64,
    pub missing_in_indexer: u64,
    pub missing_in_seed: u64,
    pub block_number_mismatches: u64,
    pub block_hash_mismatches: u64,
    pub samples: ReportSamples,
}

#[derive(Debug, Default, Serialize)]
pub struct ReportSamples {
    pub missing_in_indexer: Vec<String>,
    pub missing_in_seed: Vec<String>,
    pub block_number_mismatches: Vec<String>,
    pub block_hash_mismatches: Vec<String>,
}

const SAMPLE_LIMIT: usize = 10;

#[derive(Debug, Clone)]
struct LogKey {
    tx_hash: String,
    log_index: i32,
}

#[derive(Debug, Clone)]
struct LogValue {
    block_number: i64,
    block_hash: String,
}

pub async fn run_cross_check(pg: &PgPool, sqlite: &SqlitePool) -> Result<CrossCheckReport> {
    // Determine the indexer's block window — only compare what we indexed.
    let (min_block, max_block): (Option<i64>, Option<i64>) =
        sqlx::query_as("SELECT MIN(block_number), MAX(block_number) FROM raw_protocol_events")
            .fetch_one(pg)
            .await?;
    let (min_block, max_block) = match (min_block, max_block) {
        (Some(a), Some(b)) => (a, b),
        _ => {
            warn!("raw_protocol_events is empty; nothing to cross-check");
            return Ok(CrossCheckReport::default());
        }
    };
    info!(min_block, max_block, "cross-check window from indexer");

    // Pull indexer events in the window.
    let indexer_rows = sqlx::query(
        r#"SELECT tx_hash, log_index, block_number, block_hash
             FROM raw_protocol_events
            WHERE block_number BETWEEN $1 AND $2
              AND is_canonical = TRUE"#,
    )
    .bind(min_block)
    .bind(max_block)
    .fetch_all(pg)
    .await?;
    let mut indexer_map: HashMap<(String, i32), LogValue> = HashMap::new();
    for r in &indexer_rows {
        let tx: String = r.get(0);
        let log_index: i32 = r.get(1);
        let block_number: i64 = r.get(2);
        let block_hash: String = r.get(3);
        indexer_map.insert(
            (tx.to_lowercase(), log_index),
            LogValue {
                block_number,
                block_hash: block_hash.to_lowercase(),
            },
        );
    }
    let indexer_event_count = indexer_map.len() as u64;
    info!(indexer_event_count, "indexer events loaded");

    // Pull SQLite events in the window. payload is JSON; extract log_index, block_number,
    // block_hash. Dedupe by (tx_hash, log_index).
    let seed_rows = sqlx::query(
        r#"SELECT DISTINCT
                  lower(transaction_id) AS tx_hash,
                  CAST(json_extract(payload, '$.log_index') AS TEXT) AS log_index_hex,
                  CAST(json_extract(payload, '$.block_number') AS TEXT) AS block_number_hex,
                  json_extract(payload, '$.block_hash') AS block_hash
             FROM events
            WHERE block_number BETWEEN ?1 AND ?2"#,
    )
    .bind(min_block)
    .bind(max_block)
    .fetch_all(sqlite)
    .await?;
    let mut seed_map: HashMap<(String, i32), LogValue> = HashMap::new();
    for r in &seed_rows {
        let tx: String = r.get::<String, _>(0);
        let log_index_hex: Option<String> = r.try_get(1).ok();
        let block_number_hex: Option<String> = r.try_get(2).ok();
        let block_hash: Option<String> = r.try_get(3).ok();
        let Some(log_index_hex) = log_index_hex else {
            continue;
        };
        let Some(block_number_hex) = block_number_hex else {
            continue;
        };
        let Some(block_hash) = block_hash else {
            continue;
        };
        let Ok(log_index) = i32::from_str_radix(log_index_hex.trim_start_matches("0x"), 16) else {
            continue;
        };
        let Ok(block_number) = i64::from_str_radix(block_number_hex.trim_start_matches("0x"), 16)
        else {
            continue;
        };
        seed_map.insert(
            (tx.to_lowercase(), log_index),
            LogValue {
                block_number,
                block_hash: block_hash.to_lowercase(),
            },
        );
    }
    let seed_event_count_in_window = seed_map.len() as u64;
    info!(seed_event_count_in_window, "seed events loaded");

    let mut report = CrossCheckReport {
        indexer_event_count,
        seed_event_count_in_window,
        ..Default::default()
    };

    // Walk indexer side; compare to seed.
    for (key, idx_v) in &indexer_map {
        let label = format!("{}#{}", key.0, key.1);
        match seed_map.get(key) {
            Some(seed_v) => {
                let mut matched = true;
                if seed_v.block_number != idx_v.block_number {
                    report.block_number_mismatches += 1;
                    push_sample(
                        &mut report.samples.block_number_mismatches,
                        format!(
                            "{label}: seed_block={} indexer_block={}",
                            seed_v.block_number, idx_v.block_number
                        ),
                    );
                    matched = false;
                }
                if seed_v.block_hash != idx_v.block_hash {
                    report.block_hash_mismatches += 1;
                    push_sample(
                        &mut report.samples.block_hash_mismatches,
                        format!(
                            "{label}: seed_hash={} indexer_hash={}",
                            &seed_v.block_hash[..10],
                            &idx_v.block_hash[..10]
                        ),
                    );
                    matched = false;
                }
                if matched {
                    report.matched += 1;
                }
            }
            None => {
                report.missing_in_seed += 1;
                push_sample(&mut report.samples.missing_in_seed, label);
            }
        }
    }
    // Walk seed side for events the indexer didn't capture.
    for (key, _) in &seed_map {
        if !indexer_map.contains_key(key) {
            report.missing_in_indexer += 1;
            push_sample(
                &mut report.samples.missing_in_indexer,
                format!("{}#{}", key.0, key.1),
            );
        }
    }

    info!(?report, "cross-check complete");
    Ok(report)
}

fn push_sample(target: &mut Vec<String>, s: String) {
    if target.len() < SAMPLE_LIMIT {
        target.push(s);
    }
}

pub async fn open_sqlite(path: &std::path::Path) -> Result<SqlitePool> {
    use sqlx::sqlite::{SqliteConnectOptions, SqlitePoolOptions};
    use std::str::FromStr;
    let url = format!("sqlite:{}", path.display());
    let opts = SqliteConnectOptions::from_str(&url)?
        .read_only(true)
        .immutable(true);
    let pool = SqlitePoolOptions::new()
        .max_connections(1)
        .connect_with(opts)
        .await
        .with_context(|| format!("opening source SQLite at {}", path.display()))?;
    Ok(pool)
}

use anyhow::{Context, Result};
use livepeer_core::rpc::{BlockTag, Provider};
use sqlx::{PgPool, Row};
use tracing::{error, info, warn};

pub const SERVICE: &str = "livepeer-reorg-watcher";
pub const ARBITRUM_CHAIN_ID: i64 = 42161;
pub const WALK_DEPTH: u64 = 7_500;

#[derive(Debug, Default)]
pub struct ReorgIterationSummary {
    pub head: u64,
    pub window_start: u64,
    pub blocks_checked: usize,
    pub divergences: u64,
}

pub async fn run_once(pg: &PgPool, secondary: &Provider) -> Result<ReorgIterationSummary> {
    let head = secondary.eth_block_number().await?;
    let window_start = head.saturating_sub(WALK_DEPTH);

    let stored: Vec<(i64, String)> = sqlx::query(
        r#"SELECT DISTINCT block_number, block_hash
             FROM raw_protocol_events
            WHERE chain_id      = $1
              AND finality      = 'tentative'
              AND is_canonical  = TRUE
              AND block_number BETWEEN $2 AND $3
            ORDER BY block_number"#,
    )
    .bind(ARBITRUM_CHAIN_ID)
    .bind(window_start as i64)
    .bind(head as i64)
    .fetch_all(pg)
    .await?
    .into_iter()
    .map(|r| (r.get::<i64, _>(0), r.get::<String, _>(1)))
    .collect();

    let blocks_checked = stored.len();
    let mut divergences = 0u64;
    for (block_number, stored_hash) in stored {
        let n = block_number as u64;
        let header = secondary.eth_get_block_header(BlockTag::Number(n)).await?;
        let chain_hash = header
            .get("hash")
            .and_then(|v| v.as_str())
            .map(|s| s.to_lowercase())
            .context("block header missing .hash")?;
        let stored_hash = stored_hash.to_lowercase();
        if chain_hash != stored_hash {
            divergences += 1;
            handle_divergence(pg, n, &stored_hash, &chain_hash).await?;
        }
    }

    Ok(ReorgIterationSummary {
        head,
        window_start,
        blocks_checked,
        divergences,
    })
}

async fn handle_divergence(
    pg: &PgPool,
    block_number: u64,
    old_hash: &str,
    new_hash: &str,
) -> Result<()> {
    let mut tx = pg.begin().await?;

    let affected: u64 = sqlx::query(
        r#"UPDATE raw_protocol_events
              SET is_canonical = FALSE
            WHERE chain_id     = $1
              AND block_number = $2
              AND is_canonical = TRUE"#,
    )
    .bind(ARBITRUM_CHAIN_ID)
    .bind(block_number as i64)
    .execute(&mut *tx)
    .await?
    .rows_affected();

    let depth = 1;
    sqlx::query(
        r#"INSERT INTO reorg_events
              (chain_id, divergence_block, depth, old_block_hashes,
               new_block_hashes, affected_event_count, notes)
           VALUES ($1, $2, $3, $4, $5, $6, $7)"#,
    )
    .bind(ARBITRUM_CHAIN_ID)
    .bind(block_number as i64)
    .bind(depth as i32)
    .bind(vec![old_hash.to_string()])
    .bind(vec![new_hash.to_string()])
    .bind(affected as i32)
    .bind("v1 reorg-watcher: events-only walk; non-canonical marker only (TD-005)")
    .execute(&mut *tx)
    .await?;

    tx.commit().await?;

    let level = severity_level(depth);
    match level {
        Severity::Info => {
            info!(block_number, old_hash, new_hash, affected, "reorg detected (depth <= 2)")
        }
        Severity::Warn => {
            warn!(block_number, old_hash, new_hash, affected, "reorg detected (depth 3-50)")
        }
        Severity::Critical => {
            error!(block_number, old_hash, new_hash, affected, "REORG detected (depth > 50) - CRITICAL")
        }
    }
    Ok(())
}

enum Severity {
    Info,
    Warn,
    Critical,
}

fn severity_level(depth: u32) -> Severity {
    match depth {
        0..=2 => Severity::Info,
        3..=50 => Severity::Warn,
        _ => Severity::Critical,
    }
}

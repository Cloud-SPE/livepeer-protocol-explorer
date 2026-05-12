//! One-shot historical backfill of `orch_stake_by_round.latest_reward_cut_percent`,
//! `.latest_fee_share_percent`, and `.latest_fee_cut_percent` using chain truth
//! (`BondingManager.getTranscoder()` at each row's snapshot block).
//!
//! Context: prior to the fix in `livepeer-staker::profile::read_active_transcoder_cuts`,
//! the staker derived these columns from the latest `TranscoderUpdate` event in
//! `raw_protocol_events`. That event carries the *pending* values set by
//! `transcoder(rewardCut, feeShare)`. The protocol only copies pending → active
//! when the orch calls `reward()` in a subsequent round, and that copy emits no
//! event. So historical rows for any orch that requested a change but hadn't
//! earned since carry the pending value, which diverges from chain-truth active.
//!
//! This subcommand iterates every row in `orch_stake_by_round`, re-reads
//! `getTranscoder` at the row's `block_number`, and overwrites the three cut
//! columns when they differ. It is idempotent (re-running is a no-op once
//! converged) and never deletes data. Rows where chain returns the
//! never-registered zero state (lastRewardRound == 0) are skipped so we don't
//! overwrite a legitimate value with chain-side noise.
//!
//! Streaming model: each row's UPDATE + CSV-line is committed *as it's
//! computed*, not batched at the end. Survives mid-flight kills — restarting
//! re-scans rows but the `Outcome::Unchanged` path skips anything the previous
//! run already corrected (because the DB row now matches chain), so progress
//! is effectively resumable. Progress is logged every 5000 rows.

use crate::Runtime;
use anyhow::{Context, Result};
use bigdecimal::BigDecimal;
use futures::stream::{self, StreamExt};
use livepeer_staker::profile::fetch_active_transcoder_state;
use sqlx::types::chrono::Utc;
use sqlx::Row;
use std::io::Write;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use tokio::sync::Mutex;
use tracing::{info, warn};

const PROGRESS_LOG_EVERY: u64 = 5_000;

#[derive(Debug)]
pub struct BackfillCutsOpts {
    pub concurrency: usize,
    pub csv_out: Option<PathBuf>,
    pub dry_run: bool,
    pub address: Option<String>,
}

#[derive(Debug, Default)]
pub struct BackfillCutsSummary {
    pub rows_scanned: u64,
    pub rows_changed: u64,
    pub rows_unchanged: u64,
    pub rows_skipped_unregistered: u64,
    pub errors: u64,
}

#[derive(Debug, Clone)]
struct BackfillRow {
    chain_id: i64,
    address: String,
    round: i64,
    block_number: i64,
    old_reward_cut: BigDecimal,
    old_fee_share: BigDecimal,
    old_fee_cut: BigDecimal,
}

pub async fn run(rt: &Runtime, opts: BackfillCutsOpts) -> Result<BackfillCutsSummary> {
    let bonding_manager = rt.cfg.static_.contracts.bonding_manager.to_lowercase();
    let pg = &rt.pg;
    let archive = &rt.archive;

    let rows = fetch_rows(pg, opts.address.as_deref()).await?;
    let total = rows.len() as u64;

    // Open CSV in append mode for resumability. Default path is timestamped;
    // when the caller passes --csv-out we re-open the same file across runs.
    let csv_path = opts.csv_out.clone().unwrap_or_else(|| {
        PathBuf::from(format!("/tmp/cuts-backfill-{}.csv", Utc::now().timestamp()))
    });
    let csv_file = open_csv_with_header(&csv_path)?;
    let csv_file = Arc::new(Mutex::new(csv_file));

    info!(
        scanned = total,
        concurrency = opts.concurrency,
        dry_run = opts.dry_run,
        csv = %csv_path.display(),
        "backfill-cuts: starting (streaming/resumable)"
    );

    // Atomic counters shared across the concurrent fanout.
    let scanned = AtomicU64::new(0);
    let changed = AtomicU64::new(0);
    let unchanged = AtomicU64::new(0);
    let skipped = AtomicU64::new(0);
    let errors = AtomicU64::new(0);

    let bm_ref = bonding_manager.as_str();
    let scanned_ref = &scanned;
    let changed_ref = &changed;
    let unchanged_ref = &unchanged;
    let skipped_ref = &skipped;
    let errors_ref = &errors;
    let csv_file_ref = &csv_file;
    let dry_run = opts.dry_run;

    stream::iter(rows.into_iter().map(|row| async move {
        process_row(pg, archive, bm_ref, csv_file_ref, dry_run, row).await
    }))
    .buffer_unordered(opts.concurrency)
    .for_each(|outcome| async move {
        match outcome {
            RowOutcome::Changed => {
                changed_ref.fetch_add(1, Ordering::Relaxed);
            }
            RowOutcome::Unchanged => {
                unchanged_ref.fetch_add(1, Ordering::Relaxed);
            }
            RowOutcome::SkippedUnregistered => {
                skipped_ref.fetch_add(1, Ordering::Relaxed);
            }
            RowOutcome::Error => {
                errors_ref.fetch_add(1, Ordering::Relaxed);
            }
        }
        let s = scanned_ref.fetch_add(1, Ordering::Relaxed) + 1;
        if s.is_multiple_of(PROGRESS_LOG_EVERY) || s == total {
            info!(
                scanned = s,
                total,
                changed = changed_ref.load(Ordering::Relaxed),
                unchanged = unchanged_ref.load(Ordering::Relaxed),
                skipped = skipped_ref.load(Ordering::Relaxed),
                errors = errors_ref.load(Ordering::Relaxed),
                "backfill-cuts: progress"
            );
        }
    })
    .await;

    // Flush CSV one last time.
    {
        let mut f = csv_file.lock().await;
        f.flush()?;
    }

    let summary = BackfillCutsSummary {
        rows_scanned: total,
        rows_changed: changed.load(Ordering::Relaxed),
        rows_unchanged: unchanged.load(Ordering::Relaxed),
        rows_skipped_unregistered: skipped.load(Ordering::Relaxed),
        errors: errors.load(Ordering::Relaxed),
    };
    info!(
        rows_scanned = summary.rows_scanned,
        rows_changed = summary.rows_changed,
        rows_unchanged = summary.rows_unchanged,
        rows_skipped_unregistered = summary.rows_skipped_unregistered,
        errors = summary.errors,
        csv = %csv_path.display(),
        "backfill-cuts: complete"
    );
    Ok(summary)
}

#[derive(Debug)]
enum RowOutcome {
    Changed,
    Unchanged,
    SkippedUnregistered,
    Error,
}

async fn process_row(
    pg: &sqlx::PgPool,
    archive: &livepeer_core::rpc::Provider,
    bonding_manager: &str,
    csv_file: &Arc<Mutex<std::fs::File>>,
    dry_run: bool,
    row: BackfillRow,
) -> RowOutcome {
    let state = match fetch_active_transcoder_state(
        pg,
        archive,
        bonding_manager,
        &row.address,
        row.block_number,
    )
    .await
    {
        Ok(s) => s,
        Err(e) => {
            warn!(address = %row.address, round = row.round, error = %e, "backfill-cuts: row error");
            return RowOutcome::Error;
        }
    };

    if state.last_reward_round == 0 {
        warn!(address = %row.address, round = row.round, "backfill-cuts: skipping never-registered row");
        return RowOutcome::SkippedUnregistered;
    }

    let unchanged = state.reward_cut_percent == row.old_reward_cut
        && state.fee_share_percent == row.old_fee_share
        && state.fee_cut_percent == row.old_fee_cut;
    if unchanged {
        return RowOutcome::Unchanged;
    }

    // Append CSV line first so the audit log captures the intended change
    // even if the subsequent UPDATE errors mid-run.
    {
        let mut f = csv_file.lock().await;
        if let Err(e) = writeln!(
            f,
            "{},{},{},{},{},{},{},{},{},{}",
            row.chain_id,
            row.address,
            row.round,
            row.block_number,
            row.old_reward_cut,
            state.reward_cut_percent,
            row.old_fee_share,
            state.fee_share_percent,
            row.old_fee_cut,
            state.fee_cut_percent,
        ) {
            warn!(address = %row.address, round = row.round, error = %e, "backfill-cuts: csv write failed");
            return RowOutcome::Error;
        }
    }

    if !dry_run {
        let res = sqlx::query(
            r#"UPDATE orch_stake_by_round
                  SET latest_reward_cut_percent = $1,
                      latest_fee_share_percent  = $2,
                      latest_fee_cut_percent    = $3
                WHERE chain_id = $4 AND address = $5 AND round = $6"#,
        )
        .bind(&state.reward_cut_percent)
        .bind(&state.fee_share_percent)
        .bind(&state.fee_cut_percent)
        .bind(row.chain_id)
        .bind(&row.address)
        .bind(row.round)
        .execute(pg)
        .await;
        if let Err(e) = res {
            warn!(address = %row.address, round = row.round, error = %e, "backfill-cuts: update failed");
            return RowOutcome::Error;
        }
    }

    RowOutcome::Changed
}

fn open_csv_with_header(path: &std::path::Path) -> Result<std::fs::File> {
    let exists_and_nonempty = std::fs::metadata(path)
        .map(|m| m.len() > 0)
        .unwrap_or(false);
    let mut f = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
        .with_context(|| format!("opening csv at {}", path.display()))?;
    if !exists_and_nonempty {
        writeln!(
            f,
            "chain_id,address,round,block_number,old_reward_cut,new_reward_cut,old_fee_share,new_fee_share,old_fee_cut,new_fee_cut"
        )?;
    }
    Ok(f)
}

async fn fetch_rows(pg: &sqlx::PgPool, address: Option<&str>) -> Result<Vec<BackfillRow>> {
    let q = match address {
        Some(addr) => sqlx::query(
            r#"SELECT chain_id, address, round, block_number,
                      latest_reward_cut_percent, latest_fee_share_percent,
                      latest_fee_cut_percent
                 FROM orch_stake_by_round
                WHERE lower(address) = lower($1)
                ORDER BY round, address"#,
        )
        .bind(addr),
        None => sqlx::query(
            r#"SELECT chain_id, address, round, block_number,
                      latest_reward_cut_percent, latest_fee_share_percent,
                      latest_fee_cut_percent
                 FROM orch_stake_by_round
                ORDER BY round, address"#,
        ),
    };
    let rows = q.fetch_all(pg).await?;
    let out = rows
        .into_iter()
        .map(|r| BackfillRow {
            chain_id: r.get::<i64, _>("chain_id"),
            address: r.get::<String, _>("address"),
            round: r.get::<i64, _>("round"),
            block_number: r.get::<i64, _>("block_number"),
            old_reward_cut: r.get::<BigDecimal, _>("latest_reward_cut_percent"),
            old_fee_share: r.get::<BigDecimal, _>("latest_fee_share_percent"),
            old_fee_cut: r.get::<BigDecimal, _>("latest_fee_cut_percent"),
        })
        .collect();
    Ok(out)
}

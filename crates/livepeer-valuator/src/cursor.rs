//! Per-pass `finalized_at` high-water mark for incremental candidate detection.
//!
//! The valuator used to re-scan the whole finalized history every cycle. This
//! cursor lets each pass scan only the recently-finalized tail. It is keyed on
//! `finalized_at` (NOT block_number/id) because the indexer can backfill old
//! block ranges at any time; those rows finalize with small block numbers but a
//! recent `finalized_at`, so a block/id watermark would skip them forever.
//!
//! Safety: the candidate anti-join predicates stay in every query and remain
//! the correctness backstop (no double-pricing). This cursor only narrows the
//! scan, so a cursor bug degrades to "slower / stuck-low", never "skips work".

use anyhow::Result;
use chrono::{DateTime, Duration, Utc};
use sqlx::PgPool;

/// Default lookback margin subtracted from the stored watermark, absorbing
/// valuator/finality-watcher clock skew + concurrent finalization at the frontier.
pub const DEFAULT_LOOKBACK_SECS: i64 = 600;

pub fn pass_key(version: &str, pass: &str) -> String {
    format!("valuator_{version}_{pass}")
}

fn epoch() -> DateTime<Utc> {
    DateTime::from_timestamp(0, 0).expect("unix epoch is valid")
}

/// Finalized_at floor for a candidate scan. Returns `epoch` (scan-all / cold
/// start) when: `include_tentative` is set (tentative rows have NULL
/// finalized_at, so the cursor is disabled), OR no cursor is stored yet, OR no
/// `event_valuations` exist for the version (post-truncate/replay — derive
/// cold-start from truncatable state, never trust a stale watermark).
pub async fn scan_floor(
    pg: &PgPool,
    version: &str,
    key: &str,
    lookback_secs: i64,
    include_tentative: bool,
) -> Result<DateTime<Utc>> {
    if include_tentative {
        return Ok(epoch());
    }
    let wm: Option<DateTime<Utc>> =
        sqlx::query_scalar("SELECT watermark FROM valuator_cursors WHERE pass_key = $1")
            .bind(key)
            .fetch_optional(pg)
            .await?;
    let Some(wm) = wm else { return Ok(epoch()) };

    let has_valuations: bool = sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM event_valuations WHERE valuation_version = $1)",
    )
    .bind(version)
    .fetch_one(pg)
    .await?;
    if has_valuations {
        Ok(wm - Duration::seconds(lookback_secs))
    } else {
        Ok(epoch())
    }
}

/// Advance the pass watermark after a completed pass.
/// - `frontier`: newest `finalized_at` in the pass's candidate universe.
/// - `min_unresolved`: oldest `finalized_at` this cycle left unresolved (a
///   *transient* failure that will retry) — pins the cursor so it is re-scanned.
///
/// The new watermark is `min(frontier, min_unresolved)`, so the cursor never
/// advances past an event that still needs work. No-op under `include_tentative`.
pub async fn advance(
    pg: &PgPool,
    key: &str,
    min_unresolved: Option<DateTime<Utc>>,
    frontier: Option<DateTime<Utc>>,
    include_tentative: bool,
) -> Result<()> {
    if include_tentative {
        return Ok(());
    }
    let Some(frontier) = frontier else {
        return Ok(());
    };
    let new_wm = next_watermark(min_unresolved, frontier);
    sqlx::query(
        "INSERT INTO valuator_cursors (pass_key, watermark, updated_at)
             VALUES ($1, $2, now())
         ON CONFLICT (pass_key)
             DO UPDATE SET watermark = EXCLUDED.watermark, updated_at = now()",
    )
    .bind(key)
    .bind(new_wm)
    .execute(pg)
    .await?;
    Ok(())
}

/// Newest `finalized_at` over the asset-scoped candidate universe (index-only
/// MAX). None if nothing is finalized for that asset yet.
pub async fn frontier_for_asset(
    pg: &PgPool,
    chain_id: i64,
    asset: &str,
) -> Result<Option<DateTime<Utc>>> {
    Ok(sqlx::query_scalar(
        "SELECT MAX(finalized_at) FROM raw_protocol_events
          WHERE chain_id = $1 AND is_valuable AND is_canonical
            AND finality = 'finalized' AND asset = $2",
    )
    .bind(chain_id)
    .bind(asset)
    .fetch_one(pg)
    .await?)
}

/// The advance rule: move the watermark to the frontier, but never past the
/// oldest unresolved (transient-failure) event, so it is re-scanned next cycle.
pub(crate) fn next_watermark(
    min_unresolved: Option<DateTime<Utc>>,
    frontier: DateTime<Utc>,
) -> DateTime<Utc> {
    match min_unresolved {
        Some(m) => m.min(frontier),
        None => frontier,
    }
}

/// Newest `finalized_at` over the multi-asset (EarningsClaimed, asset IS NULL)
/// candidate universe.
pub async fn frontier_multi(pg: &PgPool, chain_id: i64) -> Result<Option<DateTime<Utc>>> {
    Ok(sqlx::query_scalar(
        "SELECT MAX(finalized_at) FROM raw_protocol_events
          WHERE chain_id = $1 AND is_valuable AND is_canonical
            AND finality = 'finalized' AND event_name = 'EarningsClaimed' AND asset IS NULL",
    )
    .bind(chain_id)
    .fetch_one(pg)
    .await?)
}

#[cfg(test)]
mod tests {
    use super::next_watermark;
    use chrono::{DateTime, Utc};

    fn ts(secs: i64) -> DateTime<Utc> {
        DateTime::from_timestamp(secs, 0).unwrap()
    }

    #[test]
    fn advances_to_frontier_when_all_resolved() {
        // No unresolved work → move all the way to the finalized frontier.
        assert_eq!(next_watermark(None, ts(1000)), ts(1000));
    }

    #[test]
    fn pins_at_oldest_unresolved() {
        // A transient failure at an older finalized_at must pin the cursor there
        // so it is re-scanned, even though newer events resolved.
        assert_eq!(next_watermark(Some(ts(400)), ts(1000)), ts(400));
    }

    #[test]
    fn never_advances_past_frontier_even_if_unresolved_is_newer() {
        // Defensive: unresolved can't be beyond the frontier, but if it were,
        // we still cap at the frontier.
        assert_eq!(next_watermark(Some(ts(2000)), ts(1000)), ts(1000));
    }
}

//! Aggregations endpoint. SPEC §14.3.6.
//!
//! Replaces the legacy /api/payout/{daily,weekly,monthly}/:date and
//! /api/payout/tickets/daily/:start/:end routes. One uniform endpoint with
//! a `bucket` parameter.
//!
//! TD-018 Phase 1: broad time-window queries are now served by the
//! `event_metrics_daily` rollup so the endpoint no longer scans
//! `raw_protocol_events` for every request. Address-filtered queries and
//! non-UTC timezone requests fall back to the original per-event scan
//! because their semantics aren't representable at the daily-rollup grain.

use crate::{error::ApiError, state::AppState};
use axum::{
    extract::{Query, State},
    Json,
};
use chrono::{DateTime, NaiveDate, Utc};
use serde::{Deserialize, Serialize};
use sqlx::Row;
use utoipa::{IntoParams, ToSchema};

const MAX_BUCKETS: i64 = 1_000;

#[derive(Debug, Deserialize, IntoParams, ToSchema)]
#[schema(description = "Query model for time-bucketed event counts and value aggregations.")]
pub struct AggregationsQuery {
    pub contract: Option<String>,
    pub event_name: Option<String>,
    /// One of `day`, `week`, or `month`.
    pub bucket: String,
    /// ISO date YYYY-MM-DD or block number. Both endpoints accept both forms.
    pub from: Option<String>,
    pub to: Option<String>,
    pub address: Option<String>,
    pub from_address: Option<String>,
    pub to_address: Option<String>,
    pub asset: Option<String>,
    /// One of: count, sum_amount_native, sum_amount_usd, avg_amount_usd
    pub metric: String,
    pub valuation_version: Option<String>,
    /// IANA tz; default UTC. Controls bucket-edge alignment via date_trunc.
    pub tz: Option<String>,
}

#[derive(Debug, Serialize, ToSchema)]
#[schema(description = "Aggregation response over canonical indexed events.")]
pub struct AggregationsResponse {
    pub bucket: String,
    pub tz: String,
    pub metric: String,
    pub valuation_version: Option<String>,
    /// Source the response was served from: "rollup" (event_metrics_daily,
    /// fast path) or "raw_events" (per-event scan, used when filters can't
    /// be answered from the daily rollup).
    pub source: String,
    pub results: Vec<BucketRow>,
}

#[derive(Debug, Serialize, ToSchema)]
#[schema(description = "One bucket in the aggregation result set.")]
pub struct BucketRow {
    pub bucket_start: DateTime<Utc>,
    pub count: String,
    pub value: Option<String>,
}

#[utoipa::path(
    get,
    path = "/aggregations/events",
    tag = "Aggregations",
    params(AggregationsQuery),
    responses(
        (status = 200, description = "Time-bucketed event counts or valuation aggregates over the canonical event set.", body = AggregationsResponse),
        (status = 400, description = "Invalid bucket, metric, or range parameter.", body = crate::error::ErrorEnvelope),
        (status = 500, description = "Unexpected server error.", body = crate::error::ErrorEnvelope)
    )
)]
pub async fn events(
    State(state): State<AppState>,
    Query(q): Query<AggregationsQuery>,
) -> Result<Json<AggregationsResponse>, ApiError> {
    let bucket = match q.bucket.as_str() {
        "day" | "week" | "month" => q.bucket.as_str(),
        other => {
            return Err(ApiError::bad_request(format!(
                "invalid bucket {other:?}; use day | week | month"
            )))
        }
    };
    let metric = match q.metric.as_str() {
        "count" | "sum_amount_native" | "sum_amount_usd" | "avg_amount_usd" => q.metric.as_str(),
        other => return Err(ApiError::bad_request(format!(
            "invalid metric {other:?}; use count | sum_amount_native | sum_amount_usd | avg_amount_usd"
        ))),
    };
    let tz = q.tz.clone().unwrap_or_else(|| "UTC".to_string());
    let needs_valuations = matches!(
        metric,
        "sum_amount_native" | "sum_amount_usd" | "avg_amount_usd"
    );

    let (from_block_opt, from_ts_opt) = parse_from_to(q.from.as_deref())?;
    let (to_block_opt, to_ts_opt) = parse_from_to(q.to.as_deref())?;

    // Decide rollup vs direct scan. The rollup is keyed by (chain, day_utc,
    // contract, event_name, asset, valuation_version) — it cannot answer
    // address-filtered queries or non-UTC timezone bucket alignment.
    //
    // Additional correctness gate: the rollup is only used when its
    // checkpoint covers the requested upper bound. While backfill is in
    // progress (or for "live" queries without an explicit `to`), the rollup
    // would return partial data, so we fall back to the direct scan.
    let mut rollup_eligible =
        q.address.is_none() && q.from_address.is_none() && q.to_address.is_none() && tz == "UTC";
    if rollup_eligible {
        rollup_eligible =
            rollup_covers_range(&state, from_block_opt, to_block_opt, from_ts_opt, to_ts_opt)
                .await?;
    }

    let valuation_version = if needs_valuations {
        Some(
            q.valuation_version
                .clone()
                .unwrap_or_else(|| state.default_version.clone()),
        )
    } else {
        None
    };

    if rollup_eligible {
        let results = aggregate_from_rollup(
            &state,
            &q,
            bucket,
            metric,
            valuation_version.as_deref(),
            from_block_opt,
            to_block_opt,
            from_ts_opt,
            to_ts_opt,
        )
        .await?;
        return Ok(Json(AggregationsResponse {
            bucket: bucket.to_string(),
            tz,
            metric: metric.to_string(),
            valuation_version,
            source: "rollup".to_string(),
            results,
        }));
    }

    let results = aggregate_from_raw_events(
        &state,
        &q,
        bucket,
        metric,
        &tz,
        valuation_version.as_deref(),
        from_block_opt,
        to_block_opt,
        from_ts_opt,
        to_ts_opt,
    )
    .await?;
    Ok(Json(AggregationsResponse {
        bucket: bucket.to_string(),
        tz,
        metric: metric.to_string(),
        valuation_version,
        source: "raw_events".to_string(),
        results,
    }))
}

#[allow(clippy::too_many_arguments)]
async fn aggregate_from_rollup(
    state: &AppState,
    q: &AggregationsQuery,
    bucket: &str,
    metric: &str,
    valuation_version: Option<&str>,
    from_block_opt: Option<i64>,
    to_block_opt: Option<i64>,
    from_ts_opt: Option<DateTime<Utc>>,
    to_ts_opt: Option<DateTime<Utc>>,
) -> Result<Vec<BucketRow>, ApiError> {
    // Map block-based filters to day-based filters when the user passes them
    // — the rollup only knows day_utc. For block ranges, we widen the filter
    // to "any day intersecting the block range" by matching against the
    // earliest/latest event_id observed in those blocks. To keep the rollup
    // path strictly faster than the direct scan, we resolve block→day via a
    // single helper query. If either resolution fails the call falls through
    // to the direct path.
    let (from_day_opt, to_day_opt) =
        resolve_day_window(state, from_block_opt, to_block_opt).await?;
    let from_day_opt = from_day_opt.or_else(|| from_ts_opt.map(|t| t.date_naive()));
    let to_day_opt = to_day_opt.or_else(|| to_ts_opt.map(|t| t.date_naive()));

    let mut clauses: Vec<String> = vec!["chain_id = $1".to_string()];
    let mut binds: Vec<Bind> = vec![Bind::I64(state.chain_id)];
    let mut idx = 2u8;

    macro_rules! filter {
        ($val:expr, $sql:expr) => {
            if let Some(v) = $val {
                clauses.push(format!($sql, idx = idx));
                binds.push(v);
                idx += 1;
            }
        };
    }
    filter!(q.contract.clone().map(Bind::Str), "contract_name = ${idx}");
    filter!(q.event_name.clone().map(Bind::Str), "event_name = ${idx}");
    filter!(
        q.asset.clone().map(|s| Bind::Str(s.to_uppercase())),
        "asset = ${idx}"
    );
    filter!(from_day_opt.map(Bind::Day), "day_utc >= ${idx}");
    filter!(to_day_opt.map(Bind::Day), "day_utc <= ${idx}");

    if let Some(v) = valuation_version {
        clauses.push(format!("valuation_version = ${idx}", idx = idx));
        binds.push(Bind::Str(v.to_string()));
        idx += 1;
    }
    let _ = idx;

    let metric_expr = match metric {
        "count" => "SUM(event_count)::TEXT".to_string(),
        "sum_amount_native" => "COALESCE(SUM(sum_amount_native), 0)::TEXT".to_string(),
        "sum_amount_usd" => "COALESCE(SUM(sum_amount_usd), 0)::TEXT".to_string(),
        "avg_amount_usd" => {
            // weighted avg: total USD ÷ total priced rows
            "(COALESCE(SUM(sum_amount_usd), 0) / NULLIF(SUM(usd_rows_priced), 0))::TEXT".to_string()
        }
        _ => unreachable!(),
    };

    // For week / month, re-bucket the daily rollup with date_trunc. UTC only
    // (rollup_eligible already enforced tz == UTC).
    let bucket_expr = match bucket {
        "day" => "day_utc::TIMESTAMPTZ".to_string(),
        "week" | "month" => format!("date_trunc('{bucket}', day_utc::TIMESTAMPTZ)"),
        _ => unreachable!(),
    };
    let count_expr = "SUM(event_count)::TEXT".to_string();
    let value_expr = if metric == "count" {
        "NULL::TEXT".to_string()
    } else {
        metric_expr
    };

    let sql = format!(
        r#"SELECT {bucket_expr} AS bucket_start,
                  {count_expr} AS count_text,
                  {value_expr} AS value_text
             FROM event_metrics_daily
            WHERE {clauses}
            GROUP BY bucket_start
            ORDER BY bucket_start
            LIMIT {max}"#,
        clauses = clauses.join(" AND "),
        max = MAX_BUCKETS,
    );

    let mut query = sqlx::query(&sql);
    for b in &binds {
        query = match b {
            Bind::I64(v) => query.bind(*v),
            Bind::Str(s) => query.bind(s),
            Bind::Day(d) => query.bind(*d),
            Bind::Ts(t) => query.bind(t),
        };
    }
    let rows = query.fetch_all(&state.pg).await?;
    Ok(rows
        .iter()
        .map(|r| BucketRow {
            bucket_start: r.get(0),
            count: r.get(1),
            value: if metric == "count" {
                None
            } else {
                Some(r.get(2))
            },
        })
        .collect())
}

#[allow(clippy::too_many_arguments)]
async fn aggregate_from_raw_events(
    state: &AppState,
    q: &AggregationsQuery,
    bucket: &str,
    metric: &str,
    tz: &str,
    valuation_version: Option<&str>,
    from_block_opt: Option<i64>,
    to_block_opt: Option<i64>,
    from_ts_opt: Option<DateTime<Utc>>,
    to_ts_opt: Option<DateTime<Utc>>,
) -> Result<Vec<BucketRow>, ApiError> {
    let needs_valuations = matches!(
        metric,
        "sum_amount_native" | "sum_amount_usd" | "avg_amount_usd"
    );

    let mut where_clauses: Vec<String> = vec![
        "r.chain_id = $1".to_string(),
        "r.is_valuable = TRUE".to_string(),
        "r.is_canonical = TRUE".to_string(),
    ];
    let mut binds: Vec<Bind> = Vec::new();
    let mut idx = 2u8;

    macro_rules! filter {
        ($val:expr, $sql:expr) => {
            if let Some(v) = $val {
                where_clauses.push(format!($sql, idx = idx));
                binds.push(v);
                idx += 1;
            }
        };
    }
    filter!(
        q.contract.clone().map(Bind::Str),
        "r.contract_name = ${idx}"
    );
    filter!(q.event_name.clone().map(Bind::Str), "r.event_name = ${idx}");
    filter!(
        q.asset.clone().map(|s| Bind::Str(s.to_uppercase())),
        "r.asset = ${idx}"
    );
    filter!(
        q.from_address.clone().map(|s| Bind::Str(s.to_lowercase())),
        "r.from_address = ${idx}"
    );
    filter!(
        q.to_address.clone().map(|s| Bind::Str(s.to_lowercase())),
        "r.to_address = ${idx}"
    );
    if let Some(addr) = q.address.as_ref() {
        where_clauses.push(format!(
            "(r.from_address = ${idx} OR r.to_address = ${idx})",
            idx = idx
        ));
        binds.push(Bind::Str(addr.to_lowercase()));
        idx += 1;
    }
    filter!(from_block_opt.map(Bind::I64), "r.block_number >= ${idx}");
    filter!(to_block_opt.map(Bind::I64), "r.block_number <= ${idx}");
    filter!(from_ts_opt.map(Bind::Ts), "r.block_timestamp >= ${idx}");
    filter!(to_ts_opt.map(Bind::Ts), "r.block_timestamp <= ${idx}");

    let join_sql = if needs_valuations {
        let v_idx = idx;
        binds.push(Bind::Str(
            valuation_version
                .map(|v| v.to_string())
                .unwrap_or_else(|| state.default_version.clone()),
        ));
        idx += 1;
        format!(
            "LEFT JOIN event_valuations v ON v.event_id = r.id \
             AND v.valuation_version = ${v_idx} \
             AND v.asset IS NOT DISTINCT FROM r.asset"
        )
    } else {
        String::new()
    };

    let metric_expr = match metric {
        "count" => "COUNT(*)::TEXT",
        "sum_amount_native" => "COALESCE(SUM(v.amount_native), 0)::TEXT",
        "sum_amount_usd" => "COALESCE(SUM(v.amount_usd), 0)::TEXT",
        "avg_amount_usd" => "COALESCE(AVG(v.amount_usd), 0)::TEXT",
        _ => unreachable!(),
    };
    let value_expr = if metric == "count" {
        "NULL::TEXT".to_string()
    } else {
        metric_expr.to_string()
    };

    let _ = idx;

    let sql = format!(
        r#"SELECT date_trunc($T_BUCKET, r.block_timestamp AT TIME ZONE $T_TZ) AT TIME ZONE $T_TZ AS bucket_start,
                  COUNT(*)::TEXT AS count_text,
                  {value_expr} AS value_text
             FROM raw_protocol_events r
             {join_sql}
            WHERE {where_clauses}
            GROUP BY bucket_start
            ORDER BY bucket_start
            LIMIT {max}"#,
        where_clauses = where_clauses.join(" AND "),
        max = MAX_BUCKETS,
    );
    let bucket_idx = idx;
    let tz_idx = idx + 1;
    let sql = sql
        .replace("$T_BUCKET", &format!("${bucket_idx}"))
        .replace("$T_TZ", &format!("${tz_idx}"));

    let mut query = sqlx::query(&sql).bind(state.chain_id);
    for b in &binds {
        query = match b {
            Bind::I64(v) => query.bind(*v),
            Bind::Str(s) => query.bind(s),
            Bind::Day(d) => query.bind(*d),
            Bind::Ts(t) => query.bind(t),
        };
    }
    query = query.bind(bucket).bind(tz);

    let rows = query.fetch_all(&state.pg).await?;

    Ok(rows
        .iter()
        .map(|r| BucketRow {
            bucket_start: r.get(0),
            count: r.get(1),
            value: if metric == "count" {
                None
            } else {
                Some(r.get(2))
            },
        })
        .collect())
}

/// Returns true if the event_metrics_daily rollup has processed every event
/// the request would include — i.e. its checkpoint covers the upper bound of
/// the requested range. While the rollup is mid-backfill, queries with an
/// open-ended upper bound or one beyond the checkpoint return partial data,
/// so we fall back to the direct raw_events scan in that case.
async fn rollup_covers_range(
    state: &AppState,
    _from_block: Option<i64>,
    to_block: Option<i64>,
    _from_ts: Option<DateTime<Utc>>,
    to_ts: Option<DateTime<Utc>>,
) -> Result<bool, ApiError> {
    let checkpoint = sqlx::query_scalar::<_, Option<i64>>(
        "SELECT last_processed_block FROM indexer_checkpoints \
         WHERE name = 'rollup_event_metrics_daily'",
    )
    .fetch_optional(&state.pg)
    .await?
    .flatten();
    let Some(ckpt) = checkpoint else {
        return Ok(false);
    };
    let row =
        sqlx::query("SELECT block_number, block_timestamp FROM raw_protocol_events WHERE id = $1")
            .bind(ckpt)
            .fetch_optional(&state.pg)
            .await?;
    let Some(row) = row else {
        return Ok(false);
    };
    let ckpt_block: i64 = row.get(0);
    let ckpt_ts: DateTime<Utc> = row.get(1);

    if let Some(to_block) = to_block {
        if to_block > ckpt_block {
            return Ok(false);
        }
    } else if let Some(to_ts) = to_ts {
        if to_ts > ckpt_ts {
            return Ok(false);
        }
    } else {
        // Open-ended upper bound — caller is asking for "everything to head",
        // which the rollup can't guarantee while live indexing continues.
        return Ok(false);
    }
    Ok(true)
}

async fn resolve_day_window(
    state: &AppState,
    from_block: Option<i64>,
    to_block: Option<i64>,
) -> Result<(Option<NaiveDate>, Option<NaiveDate>), ApiError> {
    let from_day = if let Some(b) = from_block {
        sqlx::query_scalar::<_, Option<DateTime<Utc>>>(
            "SELECT MIN(block_timestamp) FROM raw_protocol_events WHERE chain_id = $1 AND block_number >= $2"
        )
        .bind(state.chain_id)
        .bind(b)
        .fetch_one(&state.pg)
        .await?
        .map(|t| t.date_naive())
    } else {
        None
    };
    let to_day = if let Some(b) = to_block {
        sqlx::query_scalar::<_, Option<DateTime<Utc>>>(
            "SELECT MAX(block_timestamp) FROM raw_protocol_events WHERE chain_id = $1 AND block_number <= $2"
        )
        .bind(state.chain_id)
        .bind(b)
        .fetch_one(&state.pg)
        .await?
        .map(|t| t.date_naive())
    } else {
        None
    };
    Ok((from_day, to_day))
}

enum Bind {
    I64(i64),
    Str(String),
    Day(NaiveDate),
    Ts(DateTime<Utc>),
}

/// `from`/`to` accept either an ISO `YYYY-MM-DD` (treated as UTC midnight) or a block
/// number. Returns `(block_filter, ts_filter)` — at most one will be `Some`.
fn parse_from_to(s: Option<&str>) -> Result<(Option<i64>, Option<DateTime<Utc>>), ApiError> {
    let Some(s) = s else { return Ok((None, None)) };
    if let Ok(n) = s.parse::<i64>() {
        return Ok((Some(n), None));
    }
    let d = NaiveDate::parse_from_str(s, "%Y-%m-%d").map_err(|_| {
        ApiError::bad_request(format!(
            "invalid from/to: {s:?}; use YYYY-MM-DD or a block number"
        ))
    })?;
    let ts = d
        .and_hms_opt(0, 0, 0)
        .ok_or_else(|| ApiError::bad_request("invalid date"))?
        .and_utc();
    Ok((None, Some(ts)))
}

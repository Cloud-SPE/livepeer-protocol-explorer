//! Aggregations endpoint. SPEC §14.3.6.
//!
//! Replaces the legacy /api/payout/{daily,weekly,monthly}/:date and
//! /api/payout/tickets/daily/:start/:end routes. One uniform endpoint with
//! a `bucket` parameter.

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

    // Optional valuations join scoped by version. The join is a LEFT JOIN so
    // rows without a matching valuation aren't dropped from the count metric.
    let join_sql = if needs_valuations {
        let version = q
            .valuation_version
            .clone()
            .unwrap_or_else(|| state.default_version.clone());
        let v_idx = idx;
        binds.push(Bind::Str(version));
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
    // bucket + tz are bound as additional positional params at the end.
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
            Bind::Ts(t) => query.bind(t),
        };
    }
    query = query.bind(bucket).bind(&tz);

    let rows = query.fetch_all(&state.pg).await?;

    let results = rows
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
        .collect();

    Ok(Json(AggregationsResponse {
        bucket: bucket.to_string(),
        tz,
        metric: metric.to_string(),
        valuation_version: if needs_valuations {
            Some(
                q.valuation_version
                    .unwrap_or_else(|| state.default_version.clone()),
            )
        } else {
            None
        },
        results,
    }))
}

enum Bind {
    I64(i64),
    Str(String),
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

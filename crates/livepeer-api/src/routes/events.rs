//! Events endpoints. SPEC §14.3.1.

use crate::{cursor::Cursor, error::ApiError, state::AppState};
use axum::{
    extract::{Path, Query, State},
    Json,
};
use bigdecimal::BigDecimal;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sqlx::Row;
use std::collections::{HashMap, HashSet};
use utoipa::{IntoParams, ToSchema};

const DEFAULT_LIMIT: u32 = 100;
const MAX_LIMIT: u32 = 1_000;

#[derive(Debug, Default, Deserialize, IntoParams, ToSchema)]
#[schema(description = "Query parameters for listing or fetching raw protocol events.")]
pub struct EventsQuery {
    /// Optional lower block bound.
    pub from_block: Option<i64>,
    /// Optional upper block bound.
    pub to_block: Option<i64>,
    /// Contract name filter, such as `BondingManager`.
    pub contract: Option<String>,
    /// Canonical event name filter.
    pub event_name: Option<String>,
    /// Legacy alias for event_name (SPEC §14.3.1).
    pub event_type: Option<String>,
    /// Exact sender address match.
    pub from_address: Option<String>,
    /// Exact receiver/delegate address match.
    pub to_address: Option<String>,
    /// Any-role address match (matches either from_address or to_address).
    pub address: Option<String>,
    /// Exact transaction-hash match. Lowercase-normalized server-side so callers
    /// can pass mixed-case hashes. Honored on the default and `block_*` sorts;
    /// ignored by `sort=amount_usd_desc`. Combine with `event_name` when a tx
    /// emits multiple logs of different kinds.
    pub tx_hash: Option<String>,
    /// Asset symbol filter such as `LPT` or `ETH`.
    pub asset: Option<String>,
    #[serde(default)]
    pub with_valuations: bool,
    #[serde(default)]
    pub include_tentative: bool,
    #[serde(default)]
    pub include_reorged: bool,
    pub sort: Option<String>,
    pub limit: Option<u32>,
    pub cursor: Option<String>,
}

#[derive(Debug, Serialize, ToSchema)]
#[schema(description = "Canonical raw protocol event as stored in raw_protocol_events.")]
pub struct EventRow {
    pub id: String,
    pub chain_id: String,
    pub tx_hash: String,
    pub log_index: u32,
    pub block_number: String,
    pub block_hash: String,
    pub block_timestamp: DateTime<Utc>,
    pub contract_address: String,
    pub contract_name: String,
    pub event_name: String,
    pub event_signature: String,
    pub asset: Option<String>,
    pub amount_native: Option<String>,
    pub is_valuable: bool,
    pub from_address: Option<String>,
    pub to_address: Option<String>,
    pub finality: String,
    pub is_canonical: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub valuations: Option<Vec<ValuationInline>>,
}

#[derive(Debug, Serialize, ToSchema)]
#[schema(description = "Inline valuation outcome attached to an event response when requested.")]
pub struct ValuationInline {
    pub asset: String,
    pub valuation_version: String,
    pub amount_native: String,
    pub native_usd_price: Option<String>,
    pub amount_usd: Option<String>,
    pub source: String,
    pub pricing_method: String,
    pub status: String,
}

#[derive(Debug, Serialize, ToSchema)]
#[schema(
    description = "Paginated event list response with an opaque cursor and finality watermark."
)]
pub struct EventListResponse {
    pub data: Vec<EventRow>,
    pub next_cursor: Option<String>,
    pub last_finalized_block: Option<String>,
}

#[utoipa::path(
    get,
    path = "/events/{id}",
    tag = "Events",
    params(
        ("id" = i64, Path, description = "Primary key of the indexed raw event."),
        EventsQuery
    ),
    responses(
        (status = 200, description = "Single indexed event. Set `with_valuations=true` to inline attached valuation rows.", body = EventRow),
        (status = 404, description = "No event exists for the requested identifier.", body = crate::error::ErrorEnvelope),
        (status = 500, description = "Unexpected server error.", body = crate::error::ErrorEnvelope)
    )
)]
pub async fn get_one(
    State(state): State<AppState>,
    Path(id): Path<i64>,
    Query(q): Query<EventsQuery>,
) -> Result<Json<EventRow>, ApiError> {
    let row = sqlx::query(
        r#"SELECT id, chain_id, tx_hash, log_index, block_number, block_hash, block_timestamp,
                  contract_address, contract_name, event_name, event_signature,
                  asset, amount_normalized, is_valuable, from_address, to_address,
                  finality, is_canonical
             FROM raw_protocol_events
            WHERE id = $1"#,
    )
    .bind(id)
    .fetch_optional(&state.pg)
    .await?;
    let Some(row) = row else {
        return Err(ApiError::not_found(format!("event id {id}")));
    };
    let mut event = row_to_event(&row);
    if q.with_valuations {
        event.valuations = Some(load_valuations(&state, id).await?);
    }
    Ok(Json(event))
}

#[utoipa::path(
    get,
    path = "/events",
    tag = "Events",
    params(EventsQuery),
    responses(
        (status = 200, description = "Paginated raw protocol events with optional valuation inlining.", body = EventListResponse),
        (status = 400, description = "Invalid sort or cursor parameters.", body = crate::error::ErrorEnvelope),
        (status = 500, description = "Unexpected server error.", body = crate::error::ErrorEnvelope)
    )
)]
pub async fn list(
    State(state): State<AppState>,
    Query(q): Query<EventsQuery>,
) -> Result<Json<EventListResponse>, ApiError> {
    let limit = q.limit.unwrap_or(DEFAULT_LIMIT).min(MAX_LIMIT) as i64;
    let sort = q.sort.as_deref().unwrap_or("block_asc");
    if sort == "amount_usd_desc" {
        return list_by_amount_usd_desc(&state, &q, limit).await;
    }
    let (sort_clause, sort_dir_asc) = match sort {
        "block_asc" => ("ORDER BY block_number ASC, log_index ASC", true),
        "block_desc" => ("ORDER BY block_number DESC, log_index DESC", false),
        other => {
            return Err(ApiError::bad_request(format!(
                "unsupported sort: {other}; use block_asc | block_desc | amount_usd_desc"
            )))
        }
    };

    let event_name = q.event_name.clone().or_else(|| q.event_type.clone());
    let cursor = q.cursor.as_deref().map(Cursor::decode).transpose()?;

    // We tuple-compare with cursor for stable paging under the chosen sort.
    let cursor_clause = match cursor {
        Some(_) if sort_dir_asc => "AND (block_number, log_index) > ($CURSOR_BLOCK, $CURSOR_LOG)",
        Some(_) => "AND (block_number, log_index) < ($CURSOR_BLOCK, $CURSOR_LOG)",
        None => "",
    };

    let mut where_clauses: Vec<String> = vec!["chain_id = $1".to_string()];
    if !q.include_reorged {
        where_clauses.push("is_canonical = TRUE".to_string());
    }
    if !q.include_tentative {
        where_clauses.push("finality = 'finalized'".to_string());
    }

    // Filters get bind placeholders starting at $2 and increasing.
    let mut binds: Vec<Bind> = Vec::new();
    let mut next_idx = 2u8;
    macro_rules! add_filter {
        ($val:expr, $sql:expr) => {
            if let Some(v) = $val {
                where_clauses.push(format!($sql, idx = next_idx));
                binds.push(v);
                next_idx += 1;
            }
        };
    }
    add_filter!(q.from_block.map(Bind::I64), "block_number >= ${idx}");
    add_filter!(q.to_block.map(Bind::I64), "block_number <= ${idx}");
    add_filter!(q.contract.map(Bind::Str), "contract_name = ${idx}");
    add_filter!(event_name.map(Bind::Str), "event_name = ${idx}");
    add_filter!(
        q.asset.map(|s| Bind::Str(s.to_uppercase())),
        "asset = ${idx}"
    );
    add_filter!(
        q.from_address.map(|s| Bind::Str(s.to_lowercase())),
        "from_address = ${idx}"
    );
    add_filter!(
        q.to_address.map(|s| Bind::Str(s.to_lowercase())),
        "to_address = ${idx}"
    );
    add_filter!(
        q.tx_hash.map(|s| Bind::Str(s.to_lowercase())),
        "tx_hash = ${idx}"
    );
    if let Some(addr) = q.address.as_ref() {
        let lower = addr.to_lowercase();
        where_clauses.push(format!(
            "(from_address = ${idx} OR to_address = ${idx})",
            idx = next_idx
        ));
        binds.push(Bind::Str(lower));
        next_idx += 1;
    }

    let cursor_clause = cursor_clause
        .replace("$CURSOR_BLOCK", &format!("${}", next_idx))
        .replace("$CURSOR_LOG", &format!("${}", next_idx + 1));
    if cursor.is_some() {
        next_idx += 2;
    }
    let _ = next_idx;

    let sql = format!(
        r#"SELECT id, chain_id, tx_hash, log_index, block_number, block_hash, block_timestamp,
                  contract_address, contract_name, event_name, event_signature,
                  asset, amount_normalized, is_valuable, from_address, to_address,
                  finality, is_canonical
             FROM raw_protocol_events
            WHERE {where_clauses}
              {cursor_clause}
            {sort_clause}
            LIMIT {limit}"#,
        where_clauses = where_clauses.join(" AND "),
    );

    let mut query = sqlx::query(&sql).bind(state.chain_id);
    for b in &binds {
        query = match b {
            Bind::I64(v) => query.bind(*v),
            Bind::Str(s) => query.bind(s),
        };
    }
    if let Some(c) = cursor {
        query = query.bind(c.block_number).bind(c.log_index);
    }
    let rows = query.fetch_all(&state.pg).await?;

    let mut events: Vec<EventRow> = rows.iter().map(row_to_event).collect();

    // Optional inline valuations join.
    if q.with_valuations && !events.is_empty() {
        let ids: Vec<i64> = events.iter().filter_map(|e| e.id.parse().ok()).collect();
        let val_rows = sqlx::query(
            r#"SELECT event_id, asset, valuation_version, amount_native, native_usd_price,
                      amount_usd, source, pricing_method, status
                 FROM event_valuations
                WHERE event_id = ANY($1)"#,
        )
        .bind(&ids)
        .fetch_all(&state.pg)
        .await?;
        use std::collections::HashMap;
        let mut by_id: HashMap<i64, Vec<ValuationInline>> = HashMap::new();
        for r in &val_rows {
            let event_id: i64 = r.get(0);
            by_id.entry(event_id).or_default().push(ValuationInline {
                asset: r.get(1),
                valuation_version: r.get(2),
                amount_native: r.get::<BigDecimal, _>(3).to_string(),
                native_usd_price: r.get::<Option<BigDecimal>, _>(4).map(|v| v.to_string()),
                amount_usd: r.get::<Option<BigDecimal>, _>(5).map(|v| v.to_string()),
                source: r.get(6),
                pricing_method: r.get(7),
                status: r.get(8),
            });
        }
        for ev in events.iter_mut() {
            let id: i64 = ev.id.parse().unwrap_or_default();
            ev.valuations = Some(by_id.remove(&id).unwrap_or_default());
        }
    }

    let next_cursor = if events.len() as i64 == limit {
        events.last().map(|e| {
            Cursor {
                block_number: e.block_number.parse().unwrap_or_default(),
                log_index: e.log_index as i32,
            }
            .encode()
        })
    } else {
        None
    };

    let last_finalized_block: Option<i64> = sqlx::query_scalar(
        "SELECT MAX(block_number) FROM raw_protocol_events WHERE chain_id = $1 AND finality = 'finalized'",
    )
    .bind(state.chain_id)
    .fetch_one(&state.pg)
    .await?;

    Ok(Json(EventListResponse {
        data: events,
        next_cursor,
        last_finalized_block: last_finalized_block.map(|n| n.to_string()),
    }))
}

enum Bind {
    I64(i64),
    Str(String),
}

fn row_to_event(r: &sqlx::postgres::PgRow) -> EventRow {
    let amount_normalized: Option<BigDecimal> = r.get(12);
    EventRow {
        id: r.get::<i64, _>(0).to_string(),
        chain_id: r.get::<i64, _>(1).to_string(),
        tx_hash: r.get(2),
        log_index: r.get::<i32, _>(3) as u32,
        block_number: r.get::<i64, _>(4).to_string(),
        block_hash: r.get(5),
        block_timestamp: r.get(6),
        contract_address: r.get(7),
        contract_name: r.get(8),
        event_name: r.get(9),
        event_signature: r.get(10),
        asset: r.get(11),
        amount_native: amount_normalized.map(|b| b.to_string()),
        is_valuable: r.get(13),
        from_address: r.get(14),
        to_address: r.get(15),
        finality: r.get(16),
        is_canonical: r.get(17),
        valuations: None,
    }
}

async fn load_valuations(
    state: &AppState,
    event_id: i64,
) -> Result<Vec<ValuationInline>, ApiError> {
    let rows = sqlx::query(
        r#"SELECT asset, valuation_version, amount_native, native_usd_price,
                  amount_usd, source, pricing_method, status
             FROM event_valuations
            WHERE event_id = $1"#,
    )
    .bind(event_id)
    .fetch_all(&state.pg)
    .await?;
    Ok(rows
        .iter()
        .map(|r| ValuationInline {
            asset: r.get(0),
            valuation_version: r.get(1),
            amount_native: r.get::<BigDecimal, _>(2).to_string(),
            native_usd_price: r.get::<Option<BigDecimal>, _>(3).map(|v| v.to_string()),
            amount_usd: r.get::<Option<BigDecimal>, _>(4).map(|v| v.to_string()),
            source: r.get(5),
            pricing_method: r.get(6),
            status: r.get(7),
        })
        .collect())
}

/// `sort=amount_usd_desc` path. JOINs raw_protocol_events to event_valuations on
/// the operator's chosen valuation_version (defaults to state.default_version) and
/// orders by amount_usd descending. Multi-asset events have multiple valuation
/// rows per event_id; we DISTINCT on event_id and keep the largest amount_usd.
///
/// No cursor on this sort yet — it ships limit-based; cursor for floating-point-ish
/// amounts is a follow-up (would need stable tie-breakers like (amount_usd, event_id)).
async fn list_by_amount_usd_desc(
    state: &AppState,
    q: &EventsQuery,
    limit: i64,
) -> Result<Json<EventListResponse>, ApiError> {
    let version = q.event_name.as_ref(); // dummy use to silence clippy; we don't filter by event_name on this path yet
    let _ = version;
    let default_version = &state.default_version;

    let mut where_clauses: Vec<String> = vec![
        "r.chain_id = $1".to_string(),
        "r.is_valuable = TRUE".to_string(),
    ];
    if !q.include_reorged {
        where_clauses.push("r.is_canonical = TRUE".to_string());
    }
    if !q.include_tentative {
        where_clauses.push("r.finality = 'finalized'".to_string());
    }

    // We don't take filters on this path beyond the basics + valuation_version. Most
    // callers using amount_usd_desc want "top N most valuable events" — a thin slice.
    //
    // Avoid aggregating the entire valuations table. Instead, walk the top-priced
    // valuation rows in descending order and dedupe event ids in-process. Multi-asset
    // events only create a tiny number of duplicates, so a modest overscan is enough.
    let sql = format!(
        r#"SELECT r.id, r.chain_id, r.tx_hash, r.log_index, r.block_number, r.block_hash,
                  r.block_timestamp, r.contract_address, r.contract_name, r.event_name,
                  r.event_signature, r.asset, r.amount_normalized, r.is_valuable,
                  r.from_address, r.to_address, r.finality, r.is_canonical
             FROM event_valuations v
             JOIN raw_protocol_events r
               ON r.id = v.event_id
            WHERE v.valuation_version = $2
              AND v.amount_usd IS NOT NULL
              AND {where_clauses}
            ORDER BY v.amount_usd DESC, v.event_id DESC
            LIMIT $3
            OFFSET $4"#,
        where_clauses = where_clauses.join(" AND "),
    );

    let batch_size = (limit * 4).clamp(100, 5_000);
    let mut offset = 0i64;
    let mut events: Vec<EventRow> = Vec::with_capacity(limit as usize);
    let mut seen_event_ids: HashSet<i64> = HashSet::with_capacity(limit as usize);

    while (events.len() as i64) < limit {
        let rows = sqlx::query(&sql)
            .bind(state.chain_id)
            .bind(default_version)
            .bind(batch_size)
            .bind(offset)
            .fetch_all(&state.pg)
            .await?;

        if rows.is_empty() {
            break;
        }

        for row in &rows {
            let event_id: i64 = row.get(0);
            if seen_event_ids.insert(event_id) {
                events.push(row_to_event(row));
                if (events.len() as i64) == limit {
                    break;
                }
            }
        }

        if (rows.len() as i64) < batch_size {
            break;
        }
        offset += batch_size;
    }

    if q.with_valuations && !events.is_empty() {
        let ids: Vec<i64> = events.iter().filter_map(|e| e.id.parse().ok()).collect();
        let val_rows = sqlx::query(
            r#"SELECT event_id, asset, valuation_version, amount_native, native_usd_price,
                      amount_usd, source, pricing_method, status
                 FROM event_valuations
                WHERE event_id = ANY($1)"#,
        )
        .bind(&ids)
        .fetch_all(&state.pg)
        .await?;
        let mut by_id: HashMap<i64, Vec<ValuationInline>> = HashMap::new();
        for r in &val_rows {
            let event_id: i64 = r.get(0);
            by_id.entry(event_id).or_default().push(ValuationInline {
                asset: r.get(1),
                valuation_version: r.get(2),
                amount_native: r.get::<bigdecimal::BigDecimal, _>(3).to_string(),
                native_usd_price: r
                    .get::<Option<bigdecimal::BigDecimal>, _>(4)
                    .map(|v| v.to_string()),
                amount_usd: r
                    .get::<Option<bigdecimal::BigDecimal>, _>(5)
                    .map(|v| v.to_string()),
                source: r.get(6),
                pricing_method: r.get(7),
                status: r.get(8),
            });
        }
        for ev in events.iter_mut() {
            let id: i64 = ev.id.parse().unwrap_or_default();
            ev.valuations = Some(by_id.remove(&id).unwrap_or_default());
        }
    }

    Ok(Json(EventListResponse {
        data: events,
        next_cursor: None,
        last_finalized_block: None,
    }))
}

#[cfg(test)]
mod tests {
    use crate::{build_router, metrics::Metrics, state::AppState};
    use axum::{
        body::{to_bytes, Body},
        http::{Request, StatusCode},
    };
    use livepeer_core::{db, rpc::Provider};
    use serde_json::Value;
    use sqlx::PgPool;
    use std::{
        sync::Arc,
        time::{SystemTime, UNIX_EPOCH},
    };
    use tower::util::ServiceExt;

    // Integration test — needs a live Postgres + .env / DATABASE_URL.
    // CI's plain `cargo test --workspace` skips this; run locally with
    // `cargo test -p livepeer-api -- --ignored`.
    #[tokio::test]
    #[ignore = "requires DATABASE_URL or workspace-root .env"]
    async fn tx_hash_filter_returns_canonical_event_with_inline_valuation() {
        let ctx = TestContext::new().await;
        let reward_tx = "0xaaaa000000000000000000000000000000000000000000000000000000000001";
        let ticket_tx = "0xbbbb000000000000000000000000000000000000000000000000000000000002";

        let reward_event_id = seed_event(
            &ctx.pg,
            ctx.chain_id,
            reward_tx,
            "BondingManager",
            "Reward",
            Some("LPT"),
        )
        .await;
        seed_valuation(&ctx.pg, reward_event_id, "LPT", "test-version", "1.5").await;

        let ticket_event_id = seed_event(
            &ctx.pg,
            ctx.chain_id,
            ticket_tx,
            "TicketBroker",
            "WinningTicketRedeemed",
            Some("ETH"),
        )
        .await;
        seed_valuation(&ctx.pg, ticket_event_id, "ETH", "test-version", "3500").await;

        // Exact match: tx_hash → returns the Reward row with its LPT valuation inline.
        let body = ctx
            .get(&format!(
                "/api/v1/events?tx_hash={reward_tx}&with_valuations=true"
            ))
            .await;
        assert_eq!(body["data"].as_array().unwrap().len(), 1);
        assert_eq!(body["data"][0]["event_name"], "Reward");
        assert_eq!(body["data"][0]["tx_hash"], reward_tx);
        assert_eq!(body["data"][0]["valuations"][0]["asset"], "LPT");
        assert_eq!(body["data"][0]["valuations"][0]["native_usd_price"], "1.5");

        // tx_hash + event_name disambiguates when callers know which log they want.
        let body = ctx
            .get(&format!(
                "/api/v1/events?tx_hash={ticket_tx}&event_name=WinningTicketRedeemed&with_valuations=true"
            ))
            .await;
        assert_eq!(body["data"].as_array().unwrap().len(), 1);
        assert_eq!(body["data"][0]["event_name"], "WinningTicketRedeemed");
        assert_eq!(body["data"][0]["valuations"][0]["asset"], "ETH");
        assert_eq!(body["data"][0]["valuations"][0]["native_usd_price"], "3500");

        // Lowercase-normalization: mixed-case hash still matches the stored row.
        let body = ctx
            .get("/api/v1/events?tx_hash=0xAAAA000000000000000000000000000000000000000000000000000000000001")
            .await;
        assert_eq!(body["data"].as_array().unwrap().len(), 1);
        assert_eq!(body["data"][0]["tx_hash"], reward_tx);

        // No-match: bogus hash returns empty data, no next_cursor.
        let body = ctx
            .get("/api/v1/events?tx_hash=0xdeadbeef000000000000000000000000000000000000000000000000000000ff")
            .await;
        assert_eq!(body["data"].as_array().unwrap().len(), 0);
        assert!(body["next_cursor"].is_null());
    }

    struct TestContext {
        app: axum::Router,
        pg: PgPool,
        chain_id: i64,
    }

    impl TestContext {
        async fn new() -> Self {
            let pg = db::connect(&test_database_url(), 5).await.unwrap();
            let chain_id = unique_chain_id();
            let archive = Provider::new("test", "http://127.0.0.1:9").unwrap();
            let state = AppState {
                pg: pg.clone(),
                default_version: "test-version".to_string(),
                chain_id,
                ticket_broker_address: "0x0000000000000000000000000000000000000000".to_string(),
                archive,
                metrics: Arc::new(Metrics::new()),
                avatar_dir: None,
            };
            Self {
                app: build_router(state),
                pg,
                chain_id,
            }
        }

        async fn get(&self, uri: &str) -> Value {
            let response = self
                .app
                .clone()
                .oneshot(Request::builder().uri(uri).body(Body::empty()).unwrap())
                .await
                .unwrap();
            assert_eq!(response.status(), StatusCode::OK, "uri={uri}");
            let bytes = to_bytes(response.into_body(), usize::MAX).await.unwrap();
            serde_json::from_slice(&bytes).unwrap()
        }
    }

    impl Drop for TestContext {
        fn drop(&mut self) {
            let url = test_database_url();
            let chain_id = self.chain_id;
            std::thread::spawn(move || {
                if let Ok(rt) = tokio::runtime::Runtime::new() {
                    rt.block_on(async move {
                        if let Ok(pg) = db::connect(&url, 1).await {
                            let _ = sqlx::query(
                                r#"DELETE FROM event_valuations
                                    WHERE event_id IN (
                                      SELECT id FROM raw_protocol_events WHERE chain_id = $1
                                    );
                                   DELETE FROM raw_protocol_events WHERE chain_id = $1;"#,
                            )
                            .bind(chain_id)
                            .execute(&pg)
                            .await;
                        }
                    });
                }
            });
        }
    }

    async fn seed_event(
        pg: &PgPool,
        chain_id: i64,
        tx_hash: &str,
        contract_name: &str,
        event_name: &str,
        asset: Option<&str>,
    ) -> i64 {
        let row = sqlx::query(
            r#"INSERT INTO raw_protocol_events (
                   chain_id, tx_hash, log_index, block_number, block_hash, block_timestamp,
                   contract_address, contract_name, event_name, event_signature,
                   asset, is_valuable, finality, is_canonical, raw_event, abi_hash_used
               ) VALUES (
                   $1, $2, 0, 100, '0xblock', now(),
                   '0x0000000000000000000000000000000000000001', $3, $4, '0xsig',
                   $5, TRUE, 'finalized', TRUE, '{}'::jsonb, 'abi-test'
               ) RETURNING id"#,
        )
        .bind(chain_id)
        .bind(tx_hash)
        .bind(contract_name)
        .bind(event_name)
        .bind(asset)
        .fetch_one(pg)
        .await
        .unwrap();
        sqlx::Row::get::<i64, _>(&row, 0)
    }

    async fn seed_valuation(
        pg: &PgPool,
        event_id: i64,
        asset: &str,
        valuation_version: &str,
        native_usd_price: &str,
    ) {
        sqlx::query(
            r#"INSERT INTO event_valuations (
                   chain_id, event_id, valuation_version, asset, pricing_method,
                   source, block_number, amount_native, native_usd_price,
                   amount_usd, pricing_chain, status
               ) VALUES (
                   42161, $1, $2, $3, 'test',
                   'test', 100, 0, $4::numeric,
                   0, '{}'::jsonb, 'priced'
               )"#,
        )
        .bind(event_id)
        .bind(valuation_version)
        .bind(asset)
        .bind(native_usd_price)
        .execute(pg)
        .await
        .unwrap();
    }

    fn unique_chain_id() -> i64 {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos() as i64;
        970_000 + (nanos % 100_000)
    }

    fn test_database_url() -> String {
        if let Ok(url) = std::env::var("DATABASE_URL") {
            return url.replace("@postgres:", "@127.0.0.1:");
        }
        let env_path = format!("{}/../../.env", env!("CARGO_MANIFEST_DIR"));
        let env_file = std::fs::read_to_string(&env_path)
            .unwrap_or_else(|_| panic!("{env_path} must exist for API route tests"));
        let mut user = None;
        let mut password = None;
        let mut db_name = None;
        let mut port = None;
        for line in env_file.lines() {
            let trimmed = line.trim();
            if trimmed.starts_with('#') || trimmed.is_empty() {
                continue;
            }
            let Some((key, value)) = trimmed.split_once('=') else {
                continue;
            };
            let value = value.trim().trim_matches('"');
            match key.trim() {
                "POSTGRES_USER" => user = Some(value.to_string()),
                "POSTGRES_PASSWORD" => password = Some(value.to_string()),
                "POSTGRES_DB" => db_name = Some(value.to_string()),
                "POSTGRES_PORT" => port = Some(value.to_string()),
                _ => {}
            }
        }
        format!(
            "postgres://{}:{}@127.0.0.1:{}/{}",
            user.expect("POSTGRES_USER"),
            password.expect("POSTGRES_PASSWORD"),
            port.unwrap_or_else(|| "5432".to_string()),
            db_name.expect("POSTGRES_DB"),
        )
    }
}

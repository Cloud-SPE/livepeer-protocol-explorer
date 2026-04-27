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

const DEFAULT_LIMIT: u32 = 100;
const MAX_LIMIT: u32 = 1_000;

#[derive(Debug, Default, Deserialize)]
pub struct EventsQuery {
    pub from_block: Option<i64>,
    pub to_block: Option<i64>,
    pub contract: Option<String>,
    pub event_name: Option<String>,
    /// Legacy alias for event_name (SPEC §14.3.1).
    pub event_type: Option<String>,
    pub from_address: Option<String>,
    pub to_address: Option<String>,
    /// Any-role address match (matches either from_address or to_address).
    pub address: Option<String>,
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

#[derive(Debug, Serialize)]
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

#[derive(Debug, Serialize)]
pub struct ValuationInline {
    pub asset: String,
    pub valuation_version: String,
    pub amount_native: String,
    pub native_usd_price: String,
    pub amount_usd: String,
    pub source: String,
    pub pricing_method: String,
    pub status: String,
}

#[derive(Debug, Serialize)]
pub struct EventListResponse {
    pub data: Vec<EventRow>,
    pub next_cursor: Option<String>,
    pub last_finalized_block: Option<String>,
}

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
    let Some(row) = row else { return Err(ApiError::not_found(format!("event id {id}"))) };
    let mut event = row_to_event(&row);
    if q.with_valuations {
        event.valuations = Some(load_valuations(&state, id).await?);
    }
    Ok(Json(event))
}

pub async fn list(
    State(state): State<AppState>,
    Query(q): Query<EventsQuery>,
) -> Result<Json<EventListResponse>, ApiError> {
    let limit = q.limit.unwrap_or(DEFAULT_LIMIT).min(MAX_LIMIT) as i64;
    let sort = q.sort.as_deref().unwrap_or("block_asc");
    let (sort_clause, sort_dir_asc) = match sort {
        "block_asc" => ("ORDER BY block_number ASC, log_index ASC", true),
        "block_desc" => ("ORDER BY block_number DESC, log_index DESC", false),
        // amount_usd_desc requires a join + is implemented in a follow-up slice.
        other => return Err(ApiError::bad_request(format!("unsupported sort: {other}; use block_asc or block_desc"))),
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
    add_filter!(q.asset.map(|s| Bind::Str(s.to_uppercase())), "asset = ${idx}");
    add_filter!(q.from_address.map(|s| Bind::Str(s.to_lowercase())), "from_address = ${idx}");
    add_filter!(q.to_address.map(|s| Bind::Str(s.to_lowercase())), "to_address = ${idx}");
    if let Some(addr) = q.address.as_ref() {
        let lower = addr.to_lowercase();
        where_clauses.push(format!("(from_address = ${idx} OR to_address = ${idx})", idx = next_idx));
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
                native_usd_price: r.get::<BigDecimal, _>(4).to_string(),
                amount_usd: r.get::<BigDecimal, _>(5).to_string(),
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

async fn load_valuations(state: &AppState, event_id: i64) -> Result<Vec<ValuationInline>, ApiError> {
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
            native_usd_price: r.get::<BigDecimal, _>(3).to_string(),
            amount_usd: r.get::<BigDecimal, _>(4).to_string(),
            source: r.get(5),
            pricing_method: r.get(6),
            status: r.get(7),
        })
        .collect())
}

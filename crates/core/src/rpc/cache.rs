//! `rpc_call_cache` writes / reads. Determinism backbone — SPEC §11.12, §13.5.

use crate::error::Result;
use serde_json::Value;
use sha2::{Digest, Sha256};
use sqlx::PgPool;
use std::sync::atomic::{AtomicBool, Ordering};

static CACHE_ONLY_MODE: AtomicBool = AtomicBool::new(false);

pub fn set_cache_only_mode(enabled: bool) {
    CACHE_ONLY_MODE.store(enabled, Ordering::SeqCst);
}

pub fn cache_only_mode() -> bool {
    CACHE_ONLY_MODE.load(Ordering::SeqCst)
}

/// `call_hash = sha256(method || canonical_params || block)`. SPEC §11.12.
pub fn compute_call_hash(method: &str, params: &Value, block_number: Option<i64>) -> String {
    let mut h = Sha256::new();
    h.update(method.as_bytes());
    h.update(b"\0");
    // serde_json::to_string preserves array order and uses lexicographic-ish object key
    // ordering when the map is BTreeMap-backed, but for our use case all params come from
    // serde_json::json! macros that produce stable layouts. Explicit canonicalization
    // (sort keys) can land if a future call emits objects with non-deterministic ordering.
    let canonical = serde_json::to_string(params).unwrap_or_default();
    h.update(canonical.as_bytes());
    h.update(b"\0");
    if let Some(n) = block_number {
        h.update(n.to_be_bytes());
    }
    hex::encode(h.finalize())
}

pub fn hash_response_bytes(bytes: &[u8]) -> String {
    let mut h = Sha256::new();
    h.update(bytes);
    hex::encode(h.finalize())
}

/// Write a row. Idempotent. `cross_check_*` may be NULL — set them when a second
/// provider has confirmed the same bytes.
#[allow(clippy::too_many_arguments)]
pub async fn store(
    pool: &PgPool,
    call_hash: &str,
    method: &str,
    params: &Value,
    block_number: Option<i64>,
    response_bytes: &[u8],
    response_hash: &str,
    provider: &str,
    cross_check_provider: Option<&str>,
    cross_check_response_hash: Option<&str>,
) -> Result<()> {
    sqlx::query(
        r#"INSERT INTO rpc_call_cache
              (call_hash, method, params, block_number, response_bytes, response_hash,
               provider, cross_check_provider, cross_check_response_hash)
           VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9)
           ON CONFLICT (call_hash) DO NOTHING"#,
    )
    .bind(call_hash)
    .bind(method)
    .bind(params)
    .bind(block_number)
    .bind(response_bytes)
    .bind(response_hash)
    .bind(provider)
    .bind(cross_check_provider)
    .bind(cross_check_response_hash)
    .execute(pool)
    .await?;
    Ok(())
}

pub async fn get(
    pool: &PgPool,
    call_hash: &str,
) -> Result<Option<(Vec<u8>, String, String)>> {
    let row: Option<(Vec<u8>, String, String)> = sqlx::query_as(
        "SELECT response_bytes, response_hash, provider FROM rpc_call_cache WHERE call_hash = $1",
    )
    .bind(call_hash)
    .fetch_optional(pool)
    .await?;
    Ok(row)
}

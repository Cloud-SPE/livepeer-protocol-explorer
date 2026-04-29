//! Two-provider cross-check. SPEC §7.6.
//!
//! Two flavours of cross-check are needed because providers disagree on JSON shape
//! even when they agree on chain data:
//!
//! - `cross_check_call` — raw-bytes compare. Correct for `eth_call` (result is a hex
//!   blob with no provider-side rendering choices) and similar.
//! - `cross_check_block_hash` — extracts `.hash` from `eth_getBlockByNumber` responses
//!   and compares those. Different providers emit different optional null fields
//!   (`requestsHash`, `withdrawals`) so raw-bytes compare is too strict; the chain
//!   invariant is "block N has hash H" anyway (SPEC §9.2).
//!
//! On match: write to `rpc_call_cache` with both provider names + both hashes.
//! On mismatch: write a `rpc_divergence_failures` row and return `RpcDivergence` —
//! NEVER auto-retry, always surface for human review (§13.3, §10.6).

use crate::error::{CoreError, Result};
use crate::rpc::{cache, Provider};
use serde_json::Value;
use sqlx::PgPool;

/// Result of a cross-check. The bytes are the raw `result` value (JSON-encoded).
pub struct CrossCheckOutcome {
    pub call_hash: String,
    pub response_bytes: Vec<u8>,
    pub response_hash: String,
}

/// Call the same method on two providers, compare raw response bytes, and on match
/// write to cache and return the canonical bytes. On mismatch, write a divergence row
/// and return `RpcDivergence`.
///
/// `block_number` is the cache key — `Some(n)` for block-pinned calls (cacheable),
/// `None` for live calls (still cross-checked but with a different cache strategy that
/// the caller may choose to skip — for now we still write the cache row keyed on
/// the call_hash which encodes the method + params).
pub async fn cross_check_call(
    pg: &PgPool,
    a: &Provider,
    b: &Provider,
    method: &str,
    params: &Value,
    block_number: Option<i64>,
) -> Result<CrossCheckOutcome> {
    let call_hash = cache::compute_call_hash(method, params, block_number);

    let result_a = a.call(method, params).await?;
    let result_b = b.call(method, params).await?;

    let bytes_a = serde_json::to_vec(&result_a).unwrap_or_default();
    let bytes_b = serde_json::to_vec(&result_b).unwrap_or_default();
    let hash_a = cache::hash_response_bytes(&bytes_a);
    let hash_b = cache::hash_response_bytes(&bytes_b);

    if hash_a != hash_b {
        sqlx::query(
            r#"INSERT INTO rpc_divergence_failures
                  (method, params, block_number, provider_a, response_a_bytes,
                   response_a_hash, provider_b, response_b_bytes, response_b_hash)
               VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9)"#,
        )
        .bind(method)
        .bind(params)
        .bind(block_number)
        .bind(a.name())
        .bind(&bytes_a)
        .bind(&hash_a)
        .bind(b.name())
        .bind(&bytes_b)
        .bind(&hash_b)
        .execute(pg)
        .await?;
        return Err(CoreError::RpcDivergence {
            method: method.to_string(),
            block: block_number,
            provider_a: a.name().to_string(),
            hash_a,
            provider_b: b.name().to_string(),
            hash_b,
        });
    }

    cache::store(
        pg,
        &call_hash,
        method,
        params,
        block_number,
        &bytes_a,
        &hash_a,
        a.name(),
        Some(b.name()),
        Some(&hash_b),
    )
    .await?;

    Ok(CrossCheckOutcome {
        call_hash,
        response_bytes: bytes_a,
        response_hash: hash_a,
    })
}

/// Cross-check a block by extracting `.hash` from `eth_getBlockByNumber` responses on
/// both providers and comparing. The chain-level invariant per SPEC §9.2 is "block N
/// has hash H" — full-header byte compare is too strict because providers disagree on
/// optional null fields.
///
/// Returns the agreed-upon block hash on success.
pub async fn cross_check_block_hash(
    pg: &PgPool,
    a: &Provider,
    b: &Provider,
    block: u64,
) -> Result<String> {
    let params = serde_json::json!([format!("0x{:x}", block), false]);
    let result_a = a.call("eth_getBlockByNumber", &params).await?;
    let result_b = b.call("eth_getBlockByNumber", &params).await?;

    let hash_a = result_a
        .get("hash")
        .and_then(|v| v.as_str())
        .unwrap_or_default()
        .to_string();
    let hash_b = result_b
        .get("hash")
        .and_then(|v| v.as_str())
        .unwrap_or_default()
        .to_string();

    if hash_a.is_empty() || hash_b.is_empty() {
        return Err(CoreError::JsonRpc {
            provider: format!("{} or {}", a.name(), b.name()),
            method: "eth_getBlockByNumber".to_string(),
            code: -32000,
            message: format!(
                "missing .hash on block {block}: a='{hash_a}' b='{hash_b}'"
            ),
        });
    }

    if hash_a != hash_b {
        let bytes_a = serde_json::to_vec(&result_a).unwrap_or_default();
        let bytes_b = serde_json::to_vec(&result_b).unwrap_or_default();
        sqlx::query(
            r#"INSERT INTO rpc_divergence_failures
                  (method, params, block_number, provider_a, response_a_bytes,
                   response_a_hash, provider_b, response_b_bytes, response_b_hash)
               VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9)"#,
        )
        .bind("eth_getBlockByNumber.hash")
        .bind(&params)
        .bind(block as i64)
        .bind(a.name())
        .bind(&bytes_a)
        .bind(&hash_a)
        .bind(b.name())
        .bind(&bytes_b)
        .bind(&hash_b)
        .execute(pg)
        .await?;
        return Err(CoreError::RpcDivergence {
            method: "eth_getBlockByNumber.hash".to_string(),
            block: Some(block as i64),
            provider_a: a.name().to_string(),
            hash_a,
            provider_b: b.name().to_string(),
            hash_b,
        });
    }

    Ok(hash_a)
}

/// Single-provider call with cache write. Used for archive-only paths (`eth_call` at
/// historical blocks) where cross-check is impossible because the secondary lacks
/// archive depth.
pub async fn single_call_cached(
    pg: &PgPool,
    p: &Provider,
    method: &str,
    params: &Value,
    block_number: Option<i64>,
) -> Result<CrossCheckOutcome> {
    let call_hash = cache::compute_call_hash(method, params, block_number);
    if let Some((bytes, hash, _provider)) = cache::get(pg, &call_hash).await? {
        return Ok(CrossCheckOutcome {
            call_hash,
            response_bytes: bytes,
            response_hash: hash,
        });
    }
    let result = p.call(method, params).await?;
    let bytes = serde_json::to_vec(&result).unwrap_or_default();
    let hash = cache::hash_response_bytes(&bytes);
    cache::store(
        pg,
        &call_hash,
        method,
        params,
        block_number,
        &bytes,
        &hash,
        p.name(),
        None,
        None,
    )
    .await?;
    Ok(CrossCheckOutcome {
        call_hash,
        response_bytes: bytes,
        response_hash: hash,
    })
}

/// Single-provider BATCH call with cache write. Walks the request list,
/// satisfies whichever entries are already cached from a single bulk SELECT,
/// then sends only the misses as a JSON-RPC batch in one HTTP POST.
///
/// Returns one `CrossCheckOutcome` per input position (preserves order).
/// Failed individual entries within the batch propagate as `Result::Err` per
/// entry — caller decides whether to fail the whole batch or skip just the
/// failed ones.
///
/// Determinism: each call's `call_hash` and `response_bytes` are
/// byte-identical to what `single_call_cached` would produce for the same
/// `(method, params, block_number)`. Cache keys and stored bytes do not
/// depend on whether the call was sent solo or as part of a batch.
pub async fn batch_call_cached(
    pg: &PgPool,
    p: &Provider,
    requests: &[(String, Value, Option<i64>)],
) -> Result<Vec<Result<CrossCheckOutcome>>> {
    let n = requests.len();
    if n == 0 {
        return Ok(Vec::new());
    }

    // Compute call_hashes upfront so we can both check the cache and key
    // the cache writes deterministically.
    let call_hashes: Vec<String> = requests
        .iter()
        .map(|(method, params, block)| cache::compute_call_hash(method, params, *block))
        .collect();

    // Cache lookup, position-by-position. (A bulk SELECT with WHERE
    // call_hash = ANY would be slightly cheaper but the per-key SELECT is
    // already a PK lookup at ~1ms each — not the bottleneck.)
    let mut outcomes: Vec<Option<Result<CrossCheckOutcome>>> = (0..n).map(|_| None).collect();
    let mut miss_indices: Vec<usize> = Vec::new();
    for (i, hash) in call_hashes.iter().enumerate() {
        if let Some((bytes, response_hash, _provider)) = cache::get(pg, hash).await? {
            outcomes[i] = Some(Ok(CrossCheckOutcome {
                call_hash: hash.clone(),
                response_bytes: bytes,
                response_hash,
            }));
        } else {
            miss_indices.push(i);
        }
    }

    // If everything was cached, we're done — no HTTP call needed.
    if !miss_indices.is_empty() {
        // Build the batch in MISS order; we'll map results back via miss_indices.
        let miss_batch: Vec<(String, Value)> = miss_indices
            .iter()
            .map(|&i| (requests[i].0.clone(), requests[i].1.clone()))
            .collect();
        let batch_results = p.call_batch(&miss_batch).await?;

        // Each batch entry's result corresponds to position `miss_indices[k]`
        // in the original request list. Store successful entries to cache;
        // surface errors through the outcome vector.
        for (k, result) in batch_results.into_iter().enumerate() {
            let orig_idx = miss_indices[k];
            let hash = &call_hashes[orig_idx];
            let (method, params, block_number) = &requests[orig_idx];
            match result {
                Ok(value) => {
                    let bytes = serde_json::to_vec(&value).unwrap_or_default();
                    let response_hash = cache::hash_response_bytes(&bytes);
                    cache::store(
                        pg,
                        hash,
                        method,
                        params,
                        *block_number,
                        &bytes,
                        &response_hash,
                        p.name(),
                        None,
                        None,
                    )
                    .await?;
                    outcomes[orig_idx] = Some(Ok(CrossCheckOutcome {
                        call_hash: hash.clone(),
                        response_bytes: bytes,
                        response_hash,
                    }));
                }
                Err(e) => {
                    outcomes[orig_idx] = Some(Err(e));
                }
            }
        }
    }

    // Unwrap — every position has been filled either from cache or batch.
    Ok(outcomes.into_iter().map(|o| o.expect("all positions filled")).collect())
}

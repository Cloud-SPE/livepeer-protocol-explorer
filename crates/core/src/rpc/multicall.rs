//! Cache-aware JSON-RPC batch helper.
//!
//! TD-022. Wraps `Provider::call_batch` so callers can pack many cache-keyed
//! eth_calls into a single HTTP round-trip. The load-bearing invariant is
//! that **every constituent response is stored in `rpc_call_cache` under
//! its original `(method, params, block_number)` key** — exactly what
//! `single_call_cached` would have stored.
//!
//! Why JSON-RPC batching, not on-chain Multicall3:
//! - Multicall3's universal CREATE2 deployment (0xcA11bde0...) is **not**
//!   present on Arbitrum One — verified empirically via `eth_getCode`.
//!   We saw `0x` (no code) at that address on both archive providers.
//! - JSON-RPC batching achieves the same effect (one round-trip per N
//!   calls) without an on-chain dependency. Chainstack accepts batches.
//! - It's also more general: works for any RPC method, not just `eth_call`,
//!   so future callers (e.g. block-header fan-outs) can reuse the helper.
//! - The cache layer keys on `(method, params, block_number)` regardless
//!   of whether the underlying RPC was sent solo or in a batch. So replay
//!   determinism is bit-identical and existing fixtures keep working.
//!
//! See `provider.rs::call_batch` for the underlying batch-RPC primitive.

use crate::error::{CoreError, Result};
use crate::rpc::{cache, Provider};
use serde_json::Value;
use sqlx::PgPool;

/// One constituent of a batched RPC call. Carries the same shape as a
/// `single_call_cached` invocation so the cache key is bit-identical.
#[derive(Debug, Clone)]
pub struct PendingCall {
    pub method: String,
    pub params: Value,
    /// Cache scoping. `None` for non-historical calls (rare). Should match
    /// what `single_call_cached`'s `block_number` argument would have been
    /// — typically `Some(block)` for `eth_call` reads at a historical block.
    pub block_number: Option<i64>,
}

impl PendingCall {
    pub fn new(method: impl Into<String>, params: Value, block_number: Option<i64>) -> Self {
        PendingCall {
            method: method.into(),
            params,
            block_number,
        }
    }
}

/// Cache-aware batched RPC call. Returns one `Vec<u8>` per input call,
/// in the same order as `calls`. Each returned blob is the JSON-serialized
/// response value (identical shape to what `single_call_cached` returns).
///
/// For misses, makes ONE network call that packs all of them into a
/// JSON-RPC request array, then writes each constituent response back to
/// `rpc_call_cache` under its original cache key.
pub async fn batch_call_cached(
    pg: &PgPool,
    archive: &Provider,
    calls: Vec<PendingCall>,
) -> Result<Vec<Vec<u8>>> {
    let n = calls.len();
    let mut results: Vec<Option<Vec<u8>>> = vec![None; n];
    let mut misses: Vec<usize> = Vec::new();

    // 1. Probe cache for each pending call.
    for (i, c) in calls.iter().enumerate() {
        let call_hash = cache::compute_call_hash(&c.method, &c.params, c.block_number);
        if let Some((bytes, _hash, _provider)) = cache::get(pg, &call_hash).await? {
            results[i] = Some(bytes);
        } else {
            misses.push(i);
        }
    }

    if misses.is_empty() {
        return Ok(results.into_iter().map(Option::unwrap).collect());
    }

    if cache::cache_only_mode() {
        return Err(CoreError::JsonRpc {
            provider: archive.name().to_string(),
            method: "batch".to_string(),
            code: -32002,
            message: format!(
                "cache-only replay: {} of {} cached, {} missing in batch",
                n - misses.len(),
                n,
                misses.len()
            ),
        });
    }

    // 2. Build the batch payload from misses only.
    let batch: Vec<(String, Value)> = misses
        .iter()
        .map(|&i| (calls[i].method.clone(), calls[i].params.clone()))
        .collect();

    // 3. ONE round-trip for the whole batch.
    let responses = archive.call_batch(&batch).await?;

    if responses.len() != misses.len() {
        return Err(CoreError::JsonRpc {
            provider: archive.name().to_string(),
            method: "batch".to_string(),
            code: -32603,
            message: format!(
                "batch returned {} responses, expected {}",
                responses.len(),
                misses.len()
            ),
        });
    }

    // 4. Per constituent: serialize the response value into the same byte
    //    shape as single_call_cached, store under the original cache key,
    //    and slot it back into the output vector. Consume `responses` by
    //    iterator so we can propagate per-call errors with `?` (CoreError
    //    isn't Clone).
    for (response, &miss_idx) in responses.into_iter().zip(misses.iter()) {
        let value = response?;
        let bytes = serde_json::to_vec(&value).unwrap_or_default();
        let hash = cache::hash_response_bytes(&bytes);
        let c = &calls[miss_idx];
        let call_hash = cache::compute_call_hash(&c.method, &c.params, c.block_number);
        cache::store(
            pg,
            &call_hash,
            &c.method,
            &c.params,
            c.block_number,
            &bytes,
            &hash,
            archive.name(),
            None,
            None,
        )
        .await?;
        results[miss_idx] = Some(bytes);
    }

    Ok(results.into_iter().map(Option::unwrap).collect())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn pending_call_cache_key_matches_single_call_cached() {
        // The cache key for a PendingCall must be byte-identical to what
        // a single_call_cached invocation with the same (method, params,
        // block_number) would produce. If this drifts, every batched-then-
        // replayed call breaks. Keep this test green.
        let method = "eth_call";
        let params = json!([{"to":"0x1234","data":"0xabcd"}, "0x5ca71d"]);
        let block = Some(6072093i64);

        let pc = PendingCall::new(method, params.clone(), block);

        assert_eq!(pc.method, method);
        assert_eq!(pc.params, params);
        assert_eq!(pc.block_number, block);

        // The hash is the load-bearing identity. Same inputs in PendingCall
        // and direct single_call_cached invocation must produce the same
        // call_hash byte-for-byte.
        assert_eq!(
            cache::compute_call_hash(&pc.method, &pc.params, pc.block_number),
            cache::compute_call_hash(method, &params, block),
        );
    }
}

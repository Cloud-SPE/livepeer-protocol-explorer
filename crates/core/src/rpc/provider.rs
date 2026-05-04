//! Thin JSON-RPC client. Single struct per (name, url). No routing or retries here —
//! the indexer composes those on top.

use crate::error::{CoreError, Result};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::{OwnedSemaphorePermit, RwLock, Semaphore};
use tracing::info;
use std::sync::OnceLock;

/// How often the background task replaces the reqwest client with a fresh
/// instance. Closes the existing connection pool's TCP sockets and forces
/// new TLS handshakes for the next requests. See TD-011 — Cloudflare appears
/// to demote long-lived flows from this client over time, and direct curl
/// from the same host stays fast through the demotion. Replacing the pool
/// every 30 min is a cheap experiment to test that hypothesis.
const POOL_REFRESH_INTERVAL: Duration = Duration::from_secs(30 * 60);
static GLOBAL_RPC_SEMAPHORE: OnceLock<Arc<Semaphore>> = OnceLock::new();

#[derive(Clone, Debug)]
pub struct Provider {
    name: String,
    url: String,
    /// Wrapped in Arc<RwLock<…>> so a background task can swap in a fresh
    /// reqwest client periodically. `call_once` takes a brief read lock,
    /// clones the client (cheap — reqwest::Client is internally Arc'd),
    /// and releases the lock before the actual HTTP work.
    client: Arc<RwLock<reqwest::Client>>,
}

#[derive(Clone, Copy, Debug)]
pub enum BlockTag {
    Latest,
    Number(u64),
}

impl BlockTag {
    pub fn to_param(self) -> String {
        match self {
            BlockTag::Latest => "latest".to_string(),
            BlockTag::Number(n) => format!("0x{:x}", n),
        }
    }

    pub fn cache_key(self) -> Option<i64> {
        match self {
            BlockTag::Latest => None,
            BlockTag::Number(n) => Some(n as i64),
        }
    }
}

#[derive(Serialize)]
struct JsonRpcRequest<'a> {
    jsonrpc: &'static str,
    method: &'a str,
    params: &'a Value,
    id: u64,
}

#[derive(Deserialize)]
struct JsonRpcResponse {
    #[serde(default)]
    id: Option<u64>,
    #[serde(default)]
    result: Option<Value>,
    #[serde(default)]
    error: Option<JsonRpcErrorBody>,
}

#[derive(Deserialize)]
struct JsonRpcErrorBody {
    code: i64,
    message: String,
}

/// Build a fresh `reqwest::Client` with the configured timeout. Used both at
/// startup and by the periodic-refresh background task.
fn build_client(timeout: Duration, provider_name: &str) -> Result<reqwest::Client> {
    reqwest::Client::builder()
        .timeout(timeout)
        .build()
        .map_err(|e| CoreError::Http {
            provider: provider_name.to_string(),
            source: e,
        })
}

impl Provider {
    pub fn set_global_concurrency_limit(limit: usize) {
        let _ = GLOBAL_RPC_SEMAPHORE.set(Arc::new(Semaphore::new(limit)));
    }

    pub fn new(name: impl Into<String>, url: impl Into<String>) -> Result<Self> {
        // 60s timeout — Chainstack's dashboard shows ~6% of our requests are
        // 499s (client-closed) under sustained concurrent load: their archive
        // backend queues some calls and the slow tail exceeds short timeouts
        // while they're still processing. Bumping to 60s lets us wait through
        // the slow tail instead of dropping responses Chainstack is about to
        // return; should drop the 499 rate to near-zero. The fast common path
        // is unaffected (most calls return in <500ms regardless of timeout).
        // See TD-011 — this is the response to the 499-rate finding.
        Self::with_timeout(name, url, Duration::from_secs(60))
    }

    pub fn with_timeout(name: impl Into<String>, url: impl Into<String>, timeout: Duration) -> Result<Self> {
        let name = name.into();
        let client = build_client(timeout, &name)?;
        let client = Arc::new(RwLock::new(client));

        // Spawn the background pool-refresh task. Holds a Weak reference so
        // it auto-exits when the last Provider clone drops; no explicit
        // shutdown signaling needed. Logs each successful rotation for
        // visibility.
        let weak = Arc::downgrade(&client);
        let provider_name = name.clone();
        tokio::spawn(async move {
            loop {
                tokio::time::sleep(POOL_REFRESH_INTERVAL).await;
                let Some(client_arc) = weak.upgrade() else {
                    return; // Provider has been fully dropped
                };
                match build_client(timeout, &provider_name) {
                    Ok(fresh) => {
                        *client_arc.write().await = fresh;
                        info!(provider = %provider_name, "rotated reqwest connection pool (TD-011 experiment)");
                    }
                    Err(e) => {
                        // Log and keep using the existing client — don't crash
                        // the RPC layer just because a single rebuild failed.
                        tracing::warn!(provider = %provider_name, error = %e, "reqwest client rebuild failed; retaining previous pool");
                    }
                }
            }
        });

        Ok(Self {
            name,
            url: url.into(),
            client,
        })
    }

    pub fn name(&self) -> &str {
        &self.name
    }

    /// Generic call. `params` is a JSON value (typically an array). Returns the raw
    /// `result` value as `serde_json::Value`. The cache layer wants the raw bytes —
    /// see `cache::store_with_call`.
    ///
    /// We wrap the whole `send → text` flow in a `tokio::time::timeout` so even if
    /// reqwest's per-request timeout misfires (observed on half-closed pooled
    /// connections that hang silently), we always get a clean Err in bounded time.
    pub async fn call(&self, method: &str, params: &Value) -> Result<Value> {
        // No retry. The 1-shot retry that lived here amplified queue pressure
        // when Chainstack's archive backend was already slow under sustained
        // concurrent load — every timed-out request became two requests, both
        // potentially queuing. After bumping the per-request timeout to 60s
        // and the hard-timeout to 90s, the underlying slow-tail finishes
        // instead of being abandoned, so retries aren't needed.
        //
        // If a request still fails (HTTP error, malformed body, hard timeout,
        // JSON-RPC error), the caller's outer loop handles it via
        // `summary.other_skipped` and the next run can re-attempt — same
        // behavior as before, just one fewer round-trip per real failure.
        self.call_once(method, params).await
    }

    async fn call_once(&self, method: &str, params: &Value) -> Result<Value> {
        let _permit = global_rpc_permit().await;
        let req = JsonRpcRequest {
            jsonrpc: "2.0",
            method,
            params,
            id: 1,
        };
        // Snapshot the current client. reqwest::Client is internally Arc'd,
        // so cloning is cheap (refcount bump). Releasing the read lock before
        // the HTTP work means a pool rotation mid-flight just leaves the
        // already-snapshot client in use for in-flight requests; subsequent
        // calls pick up the fresh client.
        let client = self.client.read().await.clone();
        // A bit longer than the reqwest client timeout (60s) so the inner
        // one usually fires first; this is the hard floor for half-closed
        // pooled connections that hang silently past reqwest's own timeout.
        let hard_timeout = Duration::from_secs(90);
        let send_text = async {
            client
                .post(&self.url)
                .json(&req)
                .send()
                .await
                .map_err(|e| CoreError::Http {
                    provider: self.name.clone(),
                    source: e,
                })?
                .error_for_status()
                .map_err(|e| CoreError::Http {
                    provider: self.name.clone(),
                    source: e,
                })?
                .text()
                .await
                .map_err(|e| CoreError::Http {
                    provider: self.name.clone(),
                    source: e,
                })
        };
        let resp_text = match tokio::time::timeout(hard_timeout, send_text).await {
            Ok(r) => r?,
            Err(_) => {
                return Err(CoreError::JsonRpc {
                    provider: self.name.clone(),
                    method: method.to_string(),
                    code: -32001,
                    message: format!("hard timeout {}s exceeded", hard_timeout.as_secs()),
                });
            }
        };
        let parsed: JsonRpcResponse = serde_json::from_str(&resp_text).map_err(|_| {
            // Treat malformed JSON-RPC as a determinism-fatal error per §13.3
            CoreError::JsonRpc {
                provider: self.name.clone(),
                method: method.to_string(),
                code: -32700,
                message: format!("malformed response: {resp_text}"),
            }
        })?;
        if let Some(err) = parsed.error {
            return Err(CoreError::JsonRpc {
                provider: self.name.clone(),
                method: method.to_string(),
                code: err.code,
                message: err.message,
            });
        }
        Ok(parsed.result.unwrap_or(Value::Null))
    }

    /// JSON-RPC batch call. Sends an array of `{method, params}` requests in a
    /// single HTTP POST and returns one `Result<Value>` per input position.
    /// Reduces HTTP/TCP/queue overhead by a factor equal to the batch size,
    /// which empirically matters most against archive RPCs that serialize
    /// concurrent calls per-IP (TD-011 finding 2026-04-29: 25× per-call
    /// latency under our concurrent load vs single-curl baseline).
    ///
    /// Each batch entry's response is mapped back by id (JSON-RPC servers
    /// MAY return responses in any order). Position-N response = result for
    /// position-N input.
    ///
    /// Determinism: each individual response is byte-identical to what the
    /// non-batched `call` would have produced — we just amortize the
    /// envelope. Cache layer keys on (method, params, block) per call.
    pub async fn call_batch(&self, batch: &[(String, Value)]) -> Result<Vec<Result<Value>>> {
        if batch.is_empty() {
            return Ok(Vec::new());
        }
        let _permit = global_rpc_permit().await;

        // Build the request array. id is the 1-based index — must be unique
        // within the batch and stable so we can match responses back.
        let reqs: Vec<JsonRpcRequest> = batch
            .iter()
            .enumerate()
            .map(|(i, (method, params))| JsonRpcRequest {
                jsonrpc: "2.0",
                method,
                params,
                id: (i + 1) as u64,
            })
            .collect();

        let client = self.client.read().await.clone();
        let hard_timeout = Duration::from_secs(90);
        let send_text = async {
            client
                .post(&self.url)
                .json(&reqs)
                .send()
                .await
                .map_err(|e| CoreError::Http {
                    provider: self.name.clone(),
                    source: e,
                })?
                .error_for_status()
                .map_err(|e| CoreError::Http {
                    provider: self.name.clone(),
                    source: e,
                })?
                .text()
                .await
                .map_err(|e| CoreError::Http {
                    provider: self.name.clone(),
                    source: e,
                })
        };
        let resp_text = match tokio::time::timeout(hard_timeout, send_text).await {
            Ok(r) => r?,
            Err(_) => {
                return Err(CoreError::JsonRpc {
                    provider: self.name.clone(),
                    method: "call_batch".to_string(),
                    code: -32001,
                    message: format!("hard timeout {}s exceeded for batch of {}", hard_timeout.as_secs(), batch.len()),
                });
            }
        };

        // Parse the response array. Some servers (incorrectly) return a
        // single object for a single-element batch — handle both shapes.
        let parsed: Vec<JsonRpcResponse> = match serde_json::from_str::<Vec<JsonRpcResponse>>(&resp_text) {
            Ok(v) => v,
            Err(_) => match serde_json::from_str::<JsonRpcResponse>(&resp_text) {
                Ok(single) => vec![single],
                Err(_) => {
                    return Err(CoreError::JsonRpc {
                        provider: self.name.clone(),
                        method: "call_batch".to_string(),
                        code: -32700,
                        message: format!("malformed batch response: {}", resp_text.chars().take(500).collect::<String>()),
                    });
                }
            },
        };

        // Map responses back to input positions by id. Default each slot to
        // a "missing response" error; overwrite as responses come in.
        let mut results: Vec<Result<Value>> = (0..batch.len())
            .map(|i| {
                Err(CoreError::JsonRpc {
                    provider: self.name.clone(),
                    method: batch[i].0.clone(),
                    code: -32603,
                    message: "no response for batch entry".to_string(),
                })
            })
            .collect();
        for resp in parsed {
            let Some(id) = resp.id else { continue };
            let idx = (id as usize).checked_sub(1);
            let Some(idx) = idx.filter(|&i| i < batch.len()) else { continue };
            results[idx] = if let Some(err) = resp.error {
                Err(CoreError::JsonRpc {
                    provider: self.name.clone(),
                    method: batch[idx].0.clone(),
                    code: err.code,
                    message: err.message,
                })
            } else {
                Ok(resp.result.unwrap_or(Value::Null))
            };
        }
        Ok(results)
    }

    pub async fn eth_chain_id(&self) -> Result<u64> {
        let v = self.call("eth_chainId", &serde_json::json!([])).await?;
        let s = v.as_str().unwrap_or_default().trim_start_matches("0x");
        u64::from_str_radix(s, 16).map_err(|_| CoreError::JsonRpc {
            provider: self.name.clone(),
            method: "eth_chainId".to_string(),
            code: -32000,
            message: format!("not a hex u64: {v}"),
        })
    }

    pub async fn eth_block_number(&self) -> Result<u64> {
        let v = self.call("eth_blockNumber", &serde_json::json!([])).await?;
        let s = v.as_str().unwrap_or_default().trim_start_matches("0x");
        u64::from_str_radix(s, 16).map_err(|_| CoreError::JsonRpc {
            provider: self.name.clone(),
            method: "eth_blockNumber".to_string(),
            code: -32000,
            message: format!("not a hex u64: {v}"),
        })
    }

    /// `eth_getBlockByNumber(block, false)` — header only. Used for block-hash cross-check.
    pub async fn eth_get_block_header(&self, block: BlockTag) -> Result<Value> {
        let params = serde_json::json!([block.to_param(), false]);
        self.call("eth_getBlockByNumber", &params).await
    }

    /// `eth_call({to, data}, block)` — read-only contract call.
    pub async fn eth_call(&self, to: &str, data: &str, block: BlockTag) -> Result<Value> {
        let params = serde_json::json!([
            { "to": to, "data": data },
            block.to_param(),
        ]);
        self.call("eth_call", &params).await
    }

    /// `eth_getLogs` filtered by single contract address + topic0. Returns the raw
    /// JSON array of logs — decode upstream.
    pub async fn eth_get_logs(
        &self,
        contract: &str,
        topic0: &str,
        from_block: u64,
        to_block: u64,
    ) -> Result<Value> {
        let params = serde_json::json!([{
            "address": contract,
            "topics": [topic0],
            "fromBlock": format!("0x{:x}", from_block),
            "toBlock":   format!("0x{:x}", to_block),
        }]);
        self.call("eth_getLogs", &params).await
    }
}

async fn global_rpc_permit() -> Option<OwnedSemaphorePermit> {
    let semaphore = GLOBAL_RPC_SEMAPHORE.get()?.clone();
    semaphore.acquire_owned().await.ok()
}

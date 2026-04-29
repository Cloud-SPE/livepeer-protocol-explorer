//! Thin JSON-RPC client. Single struct per (name, url). No routing or retries here —
//! the indexer composes those on top.

use crate::error::{CoreError, Result};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::RwLock;
use tracing::info;

/// How often the background task replaces the reqwest client with a fresh
/// instance. Closes the existing connection pool's TCP sockets and forces
/// new TLS handshakes for the next requests. See TD-011 — Cloudflare appears
/// to demote long-lived flows from this client over time, and direct curl
/// from the same host stays fast through the demotion. Replacing the pool
/// every 30 min is a cheap experiment to test that hypothesis.
const POOL_REFRESH_INTERVAL: Duration = Duration::from_secs(30 * 60);

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
    pub fn new(name: impl Into<String>, url: impl Into<String>) -> Result<Self> {
        // 25s timeout — empirically Chainstack archive cold reads usually complete
        // in <15s. 25s covers the slow tail without burning a whole minute per retry.
        // The dynamic-batch halving + bounded retry count in the indexer is the
        // secondary defense.
        Self::with_timeout(name, url, Duration::from_secs(25))
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
        // 1-shot retry on HTTP-layer (connection) failures only. Cloudflare
        // (which fronts Chainstack) silently drops idle HTTP/2 sessions and
        // reqwest's pool then hands us a half-closed socket that fails on
        // first use. A single retry with a fresh request usually picks a
        // different pooled connection or opens a new one, masking the issue
        // without needing protocol-level keep-alive (which can deadlock the
        // pool — observed 2026-04-29).
        //
        // We DO NOT retry JSON-RPC errors (`code` field set, malformed body,
        // hard timeout) — those are determinism-relevant and must surface.
        match self.call_once(method, params).await {
            Ok(v) => Ok(v),
            Err(CoreError::Http { .. }) => self.call_once(method, params).await,
            Err(e) => Err(e),
        }
    }

    async fn call_once(&self, method: &str, params: &Value) -> Result<Value> {
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
        // A bit longer than the reqwest client timeout so the inner one usually
        // fires first; this is the hard floor.
        let hard_timeout = Duration::from_secs(30);
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

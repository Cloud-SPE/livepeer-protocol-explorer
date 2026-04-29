//! Thin JSON-RPC client. Single struct per (name, url). No routing or retries here —
//! the indexer composes those on top.

use crate::error::{CoreError, Result};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::time::Duration;

#[derive(Clone, Debug)]
pub struct Provider {
    name: String,
    url: String,
    client: reqwest::Client,
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
        // HTTP/2 keep-alive + TCP keepalive. Cloudflare (which fronts most
        // archive RPC providers including Chainstack) silently drops idle
        // HTTP/2 streams after ~30-60s. Without these settings the valuator's
        // brief between-batch idles caused ~17-20% of subsequent requests to
        // hit "error sending request" against half-closed sessions.
        //
        //  - http2_keep_alive_interval(15s): send a PING frame every 15s,
        //    well inside Cloudflare's drop window
        //  - http2_keep_alive_timeout(5s):   if a PING isn't acked in 5s the
        //    connection is genuinely dead — close it and open a fresh one
        //    instead of waiting for the 25s timeout to fire
        //  - http2_keep_alive_while_idle(true): send PINGs even with no
        //    requests in flight (the critical bit — without it PINGs only
        //    ride alongside actual traffic, defeating the purpose)
        //  - tcp_keepalive(30s): TCP-layer fallback for the rare case the L4
        //    LB drops the connection before HTTP/2 PINGs negotiate
        let client = reqwest::Client::builder()
            .timeout(timeout)
            .http2_keep_alive_interval(Duration::from_secs(15))
            .http2_keep_alive_timeout(Duration::from_secs(5))
            .http2_keep_alive_while_idle(true)
            .tcp_keepalive(Some(Duration::from_secs(30)))
            .build()
            .map_err(|e| CoreError::Http {
                provider: name.clone(),
                source: e,
            })?;
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
        let req = JsonRpcRequest {
            jsonrpc: "2.0",
            method,
            params,
            id: 1,
        };
        // A bit longer than the reqwest client timeout so the inner one usually
        // fires first; this is the hard floor.
        let hard_timeout = Duration::from_secs(30);
        let send_text = async {
            self.client
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

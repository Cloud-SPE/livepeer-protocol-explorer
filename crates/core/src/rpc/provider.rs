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
        let client = reqwest::Client::builder()
            .timeout(timeout)
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
    pub async fn call(&self, method: &str, params: &Value) -> Result<Value> {
        let req = JsonRpcRequest {
            jsonrpc: "2.0",
            method,
            params,
            id: 1,
        };
        let resp_text = self
            .client
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
            })?;
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

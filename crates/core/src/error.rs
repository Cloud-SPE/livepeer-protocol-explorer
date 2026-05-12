use thiserror::Error;

#[derive(Debug, Error)]
pub enum CoreError {
    #[error("io error reading {path}: {source}")]
    Io {
        path: String,
        #[source]
        source: std::io::Error,
    },

    #[error("yaml parse error in {path}: {source}")]
    Yaml {
        path: String,
        #[source]
        source: serde_yaml::Error,
    },

    #[error("required env var {0} is not set")]
    MissingEnv(String),

    #[error("database error: {0}")]
    Db(#[from] sqlx::Error),

    #[error("ABI hash mismatch for {path}: expected {expected}, got {actual}")]
    AbiHashMismatch {
        path: String,
        expected: String,
        actual: String,
    },

    #[error("HTTP error talking to {provider}: {source}")]
    Http {
        provider: String,
        #[source]
        source: reqwest::Error,
    },

    #[error("JSON-RPC error from {provider} on {method}: code={code} message={message}")]
    JsonRpc {
        provider: String,
        method: String,
        code: i64,
        message: String,
    },

    // Boxed so the `RpcDivergence` payload (6 Strings, ~136B) doesn't bloat
    // every `Result<T, CoreError>` across the workspace — fixes
    // clippy::result_large_err without forcing a crate-level allow.
    #[error("RPC divergence on {0}")]
    RpcDivergence(Box<RpcDivergenceInfo>),
}

#[derive(Debug, thiserror::Error)]
#[error("{method} block={block:?}: provider {provider_a} hash={hash_a} vs provider {provider_b} hash={hash_b}")]
pub struct RpcDivergenceInfo {
    pub method: String,
    pub block: Option<i64>,
    pub provider_a: String,
    pub hash_a: String,
    pub provider_b: String,
    pub hash_b: String,
}

pub type Result<T> = std::result::Result<T, CoreError>;

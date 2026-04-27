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

    #[error("RPC divergence on {method} block={block:?}: provider {provider_a} hash={hash_a} vs provider {provider_b} hash={hash_b}")]
    RpcDivergence {
        method: String,
        block: Option<i64>,
        provider_a: String,
        hash_a: String,
        provider_b: String,
        hash_b: String,
    },
}

pub type Result<T> = std::result::Result<T, CoreError>;

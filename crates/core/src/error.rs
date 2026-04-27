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
}

pub type Result<T> = std::result::Result<T, CoreError>;

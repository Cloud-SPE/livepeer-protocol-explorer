//! ABI registry — load JSON files, compute sha256, sync into `contract_abi_registry`.
//!
//! Per SPEC §5.5: every loaded ABI's hash is recomputed at boot and compared to the
//! registry; mismatch = refuse to start.

use crate::error::{CoreError, Result};
use sha2::{Digest, Sha256};
use sqlx::PgPool;
use std::path::{Path, PathBuf};

/// One row of `contract_abi_registry` — what the indexer/valuator/staker
/// resolves at boot for each (proxy, block_range).
#[derive(Debug, Clone)]
pub struct AbiRegistration {
    pub contract_name: String,
    pub proxy_address: String,
    pub target_address: String,
    pub from_block: i64,
    pub to_block: Option<i64>,
    pub abi_path: String,
    pub abi_hash: String,
    pub strict_decode: bool,
}

/// Compute sha256(file contents) and return as lowercase hex.
pub fn hash_file(path: &Path) -> Result<String> {
    let bytes = std::fs::read(path).map_err(|e| CoreError::Io {
        path: path.display().to_string(),
        source: e,
    })?;
    let mut hasher = Sha256::new();
    hasher.update(&bytes);
    Ok(hex::encode(hasher.finalize()))
}

/// Recompute hashes for a fixed set of ABI files and verify each matches the registry.
/// Returns the verified rows on success. Any mismatch is a hard error per §5.5.
/// `(contract_name, proxy_address, target_address, from_block, to_block, abi_path, abi_hash, strict_decode)`.
type AbiRegistryRow = (
    String,
    String,
    String,
    i64,
    Option<i64>,
    String,
    String,
    bool,
);

pub async fn verify_against_registry(
    pool: &PgPool,
    abi_dir: &Path,
) -> Result<Vec<AbiRegistration>> {
    let rows: Vec<AbiRegistryRow> = sqlx::query_as(
        r#"SELECT contract_name, proxy_address, target_address, from_block, to_block,
                      abi_path, abi_hash, strict_decode
               FROM contract_abi_registry
               ORDER BY contract_name, from_block"#,
    )
    .fetch_all(pool)
    .await?;

    let mut verified = Vec::with_capacity(rows.len());
    for (
        contract_name,
        proxy_address,
        target_address,
        from_block,
        to_block,
        abi_path,
        abi_hash,
        strict_decode,
    ) in rows
    {
        let resolved: PathBuf = abi_dir.join(&abi_path);
        let actual = hash_file(&resolved)?;
        if actual != abi_hash {
            return Err(CoreError::AbiHashMismatch {
                path: abi_path,
                expected: abi_hash,
                actual,
            });
        }
        verified.push(AbiRegistration {
            contract_name,
            proxy_address,
            target_address,
            from_block,
            to_block,
            abi_path,
            abi_hash,
            strict_decode,
        });
    }
    Ok(verified)
}

/// Insert (or skip if present) a registry row. Idempotent.
pub async fn upsert(pool: &PgPool, reg: &AbiRegistration) -> Result<()> {
    sqlx::query(
        r#"INSERT INTO contract_abi_registry
             (contract_name, proxy_address, target_address, from_block, to_block,
              abi_path, abi_hash, strict_decode)
           VALUES ($1, $2, $3, $4, $5, $6, $7, $8)
           ON CONFLICT (contract_name, from_block) DO NOTHING"#,
    )
    .bind(&reg.contract_name)
    .bind(&reg.proxy_address)
    .bind(&reg.target_address)
    .bind(reg.from_block)
    .bind(reg.to_block)
    .bind(&reg.abi_path)
    .bind(&reg.abi_hash)
    .bind(reg.strict_decode)
    .execute(pool)
    .await?;
    Ok(())
}

//! Read-only Postgres adapter.
//!
//! The load-bearing read-only guarantee is the `diag_ro` DB role (SELECT-only
//! grants + `default_transaction_read_only`). This adapter adds a second,
//! independent layer at the session level so that even a mis-provisioned role
//! (e.g. local dev connecting as the app user) cannot write: every pooled
//! connection is pinned to `default_transaction_read_only = on` with a hard
//! statement timeout. See the plan's "Read-only enforcement" section.

use crate::config::DiagConfig;
use anyhow::{Context, Result};
use sqlx::postgres::PgPoolOptions;
use sqlx::{Executor, PgPool};
use std::time::Duration;

#[derive(Clone)]
pub struct RoDb {
    pool: PgPool,
}

impl RoDb {
    pub async fn connect(cfg: &DiagConfig) -> Result<Self> {
        let pool = PgPoolOptions::new()
            .max_connections(cfg.db_max_connections)
            .acquire_timeout(Duration::from_secs(10))
            .after_connect(|conn, _meta| {
                Box::pin(async move {
                    // Session-level read-only + resource guards. These run on
                    // every physical connection the pool opens.
                    conn.execute(
                        "SET SESSION default_transaction_read_only = on; \
                         SET SESSION statement_timeout = '15000'; \
                         SET SESSION idle_in_transaction_session_timeout = '30000';",
                    )
                    .await?;
                    Ok(())
                })
            })
            .connect(&cfg.database_url)
            .await
            .context("connecting to Postgres (read-only diag pool)")?;
        Ok(Self { pool })
    }

    pub fn pool(&self) -> &PgPool {
        &self.pool
    }

    /// Execute a caller-supplied single `SELECT`/`WITH` statement (already
    /// validated by `sql_guard`) inside an explicit read-only transaction,
    /// wrapping it so an enforced `LIMIT` is always applied and every row is
    /// returned as a JSON object regardless of column types.
    ///
    /// Returns each result row as a `serde_json::Value` (object). Per-cell
    /// truncation for token budget is applied by the caller (`output`).
    pub async fn raw_select(&self, sql: &str, limit: i64) -> Result<Vec<serde_json::Value>> {
        // to_jsonb(_diag) turns each row into a JSON object using Postgres'
        // own type→JSON mapping, so we don't have to decode arbitrary column
        // types by hand. The LIMIT is applied on the wrapper, not the inner
        // query, so it caps output even if the inner query has its own LIMIT.
        let wrapped = format!("SELECT to_jsonb(_diag) AS row FROM ( {sql} ) _diag LIMIT {limit}");

        let mut tx = self.pool.begin().await.context("begin ro txn")?;
        // Belt-and-suspenders: force this transaction read-only even if the
        // session default were somehow cleared.
        tx.execute("SET TRANSACTION READ ONLY")
            .await
            .context("set transaction read only")?;
        let rows: Vec<serde_json::Value> = sqlx::query_scalar(&wrapped)
            .fetch_all(&mut *tx)
            .await
            .context("executing raw_sql query")?;
        tx.rollback().await.ok();
        Ok(rows)
    }
}

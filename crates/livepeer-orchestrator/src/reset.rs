use anyhow::Result;
use sqlx::PgPool;

pub async fn truncate_for_bootstrap(pg: &PgPool) -> Result<()> {
    sqlx::query(
        r#"TRUNCATE TABLE
              raw_protocol_events,
              decode_failures,
              event_valuations,
              valuation_attempts,
              stake_balances_by_block,
              delegator_registry,
              token_prices_by_block,
              reorg_events,
              reorg_mutations,
              rpc_divergence_failures,
              indexer_checkpoints
           RESTART IDENTITY CASCADE"#,
    )
    .execute(pg)
    .await?;
    Ok(())
}

pub async fn truncate_for_replay(pg: &PgPool, keep_raw_events: bool) -> Result<()> {
    let sql = if keep_raw_events {
        r#"TRUNCATE TABLE
              event_valuations,
              valuation_attempts,
              stake_balances_by_block,
              delegator_registry,
              token_prices_by_block,
              reorg_events,
              reorg_mutations,
              rpc_divergence_failures
           RESTART IDENTITY CASCADE"#
    } else {
        r#"TRUNCATE TABLE
              raw_protocol_events,
              decode_failures,
              event_valuations,
              valuation_attempts,
              stake_balances_by_block,
              delegator_registry,
              token_prices_by_block,
              reorg_events,
              reorg_mutations,
              rpc_divergence_failures,
              indexer_checkpoints
           RESTART IDENTITY CASCADE"#
    };
    sqlx::query(sql).execute(pg).await?;
    Ok(())
}

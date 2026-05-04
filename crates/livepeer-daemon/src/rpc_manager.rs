use anyhow::{Context, Result};
use livepeer_core::{config::Config, rpc::Provider};
use std::sync::Arc;

const GLOBAL_RPC_LIMIT: usize = 24;
const INDEXER_RPC_LIMIT: usize = 8;
const FINALITY_RPC_LIMIT: usize = 2;
const REORG_RPC_LIMIT: usize = 2;
const VALUATOR_RPC_LIMIT: usize = 16;
const STAKER_RPC_LIMIT: usize = 6;

#[derive(Clone)]
pub struct RpcManager {
    pub archive: Arc<Provider>,
    pub secondary: Arc<Provider>,
    pub l1: Arc<Provider>,
}

impl RpcManager {
    pub async fn new(cfg: &Config) -> Result<Self> {
        Provider::set_global_concurrency_limit(GLOBAL_RPC_LIMIT);
        Provider::set_task_concurrency_limit("indexer", INDEXER_RPC_LIMIT);
        Provider::set_task_concurrency_limit("finality", FINALITY_RPC_LIMIT);
        Provider::set_task_concurrency_limit("reorg", REORG_RPC_LIMIT);
        Provider::set_task_concurrency_limit("valuator", VALUATOR_RPC_LIMIT);
        Provider::set_task_concurrency_limit("staker", STAKER_RPC_LIMIT);
        let archive = Arc::new(Provider::new(
            "chainstack",
            cfg.archive_rpc_url().context("CHAINSTACK_RPC_URL")?,
        )?);
        let secondary = Arc::new(Provider::new(
            "secondary",
            cfg.secondary_rpc_url().context("SECONDARY_RPC_URL")?,
        )?);
        let l1_url = std::env::var(
            cfg.env
                .l1
                .as_ref()
                .context("L1 config missing for daemon follow mode")?
                .url_env
                .as_str(),
        )
        .context("L1 RPC env missing for daemon follow mode")?;
        let l1 = Arc::new(Provider::new("l1-eth", l1_url)?);
        Ok(Self {
            archive,
            secondary,
            l1,
        })
    }
}

use anyhow::{Context, Result};
use livepeer_core::{config::Config, rpc::Provider};
use std::sync::Arc;

#[derive(Clone)]
pub struct RpcManager {
    pub archive: Arc<Provider>,
    pub l1: Arc<Provider>,
}

impl RpcManager {
    pub async fn new(cfg: &Config) -> Result<Self> {
        let archive = Arc::new(Provider::new(
            "chainstack",
            cfg.archive_rpc_url().context("CHAINSTACK_RPC_URL")?,
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
        Ok(Self { archive, l1 })
    }
}

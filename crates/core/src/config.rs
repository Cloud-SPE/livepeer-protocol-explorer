use crate::error::{CoreError, Result};
use serde::Deserialize;
use std::path::Path;

#[derive(Debug, Deserialize, Clone)]
pub struct ChainConfig {
    pub name: String,
    pub chain_id: u64,
    pub livepeer_arbitrum_genesis_block: u64,
}

#[derive(Debug, Deserialize, Clone)]
pub struct Contracts {
    pub controller: String,
    pub governor: String,
    pub livepeer_token: String,
    pub minter: String,
    pub bonding_manager: String,
    pub ticket_broker: String,
    pub rounds_manager: String,
    pub bonding_votes: String,
    pub l2_lpt_gateway: String,
}

#[derive(Debug, Deserialize, Clone)]
pub struct PricingConfig {
    pub default_valuation_version: String,
    pub twap_window_seconds: u64,
    pub required_observation_cardinality: u32,
    pub uniswap_v3_lpt_weth_pool: String,
    pub chainlink_eth_usd_aggregator: String,
    pub l2_sequencer_uptime_feed: String,
    pub chainlink_staleness_seconds: u64,
    pub chainlink_warn_staleness_seconds: u64,
}

#[derive(Debug, Deserialize, Clone)]
pub struct StaticConfig {
    pub chain: ChainConfig,
    pub contracts: Contracts,
    pub pricing: PricingConfig,
}

#[derive(Debug, Deserialize, Clone)]
pub struct PostgresConfig {
    pub connection_string_env: String,
    pub pool_max_connections: u32,
}

#[derive(Debug, Deserialize, Clone)]
pub struct RpcEndpoint {
    pub url_env: String,
    pub rate_limit_rps: u32,
    pub burst: u32,
    pub max_concurrent: u32,
}

#[derive(Debug, Deserialize, Clone)]
pub struct RpcEndpoints {
    pub archive: RpcEndpoint,
    pub secondary: RpcEndpoint,
}

#[derive(Debug, Deserialize, Clone)]
pub struct EnvConfig {
    pub log_level: String,
    pub postgres: PostgresConfig,
    pub rpc: RpcEndpoints,
    #[serde(default)]
    pub alerting: Option<AlertingConfig>,
    /// Optional. Required when the finality-watcher runs.
    #[serde(default)]
    pub l1: Option<L1Endpoint>,
}

#[derive(Debug, Deserialize, Clone)]
pub struct AlertingConfig {
    pub telegram: TelegramAlertingConfig,
}

#[derive(Debug, Deserialize, Clone)]
pub struct TelegramAlertingConfig {
    pub bot_token_env: String,
    pub chat_id_env: String,
    pub enabled: bool,
}

#[derive(Debug, Deserialize, Clone)]
pub struct L1Endpoint {
    pub url_env: String,
}

#[derive(Debug, Clone)]
pub struct Config {
    pub static_: StaticConfig,
    pub env: EnvConfig,
}

impl Config {
    pub fn load(static_path: impl AsRef<Path>, env_path: impl AsRef<Path>) -> Result<Self> {
        let static_ = read_yaml::<StaticConfig>(static_path.as_ref())?;
        let env = read_yaml::<EnvConfig>(env_path.as_ref())?;
        Ok(Config { static_, env })
    }

    pub fn database_url(&self) -> Result<String> {
        env_var(&self.env.postgres.connection_string_env)
    }

    pub fn archive_rpc_url(&self) -> Result<String> {
        env_var(&self.env.rpc.archive.url_env)
    }

    pub fn secondary_rpc_url(&self) -> Result<String> {
        env_var(&self.env.rpc.secondary.url_env)
    }

    pub fn telegram_alerting_enabled(&self) -> bool {
        self.env
            .alerting
            .as_ref()
            .map(|a| a.telegram.enabled)
            .unwrap_or(false)
    }

    pub fn telegram_bot_token(&self) -> Result<String> {
        env_var(
            &self
                .env
                .alerting
                .as_ref()
                .ok_or_else(|| CoreError::MissingEnv("alerting.telegram".to_string()))?
                .telegram
                .bot_token_env,
        )
    }

    pub fn telegram_chat_id(&self) -> Result<String> {
        env_var(
            &self
                .env
                .alerting
                .as_ref()
                .ok_or_else(|| CoreError::MissingEnv("alerting.telegram".to_string()))?
                .telegram
                .chat_id_env,
        )
    }
}

fn read_yaml<T: for<'de> Deserialize<'de>>(path: &Path) -> Result<T> {
    let text = std::fs::read_to_string(path).map_err(|e| CoreError::Io {
        path: path.display().to_string(),
        source: e,
    })?;
    serde_yaml::from_str(&text).map_err(|e| CoreError::Yaml {
        path: path.display().to_string(),
        source: e,
    })
}

fn env_var(key: &str) -> Result<String> {
    std::env::var(key).map_err(|_| CoreError::MissingEnv(key.to_string()))
}

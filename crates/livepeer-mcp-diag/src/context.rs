//! Shared, cloneable handle to every adapter + resolved config. Tools take
//! `&DiagContext` and stay free of transport/rmcp concerns.

use crate::adapters::db::RoDb;
use crate::adapters::docker::DockerClient;
use crate::adapters::metrics::MetricsClient;
use crate::config::DiagConfig;

pub struct DiagContext {
    pub db: RoDb,
    pub metrics: MetricsClient,
    pub docker: DockerClient,
    pub cfg: DiagConfig,
}

impl DiagContext {
    pub fn valuation_version(&self) -> &str {
        &self.cfg.valuation_version
    }
}

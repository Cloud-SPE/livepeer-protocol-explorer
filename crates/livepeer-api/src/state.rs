use crate::metrics::Metrics;
use livepeer_core::rpc::Provider;
use sqlx::PgPool;
use std::sync::Arc;

#[derive(Clone)]
pub struct AppState {
    pub pg: PgPool,
    pub default_version: String,
    pub chain_id: i64,
    pub ticket_broker_address: String,
    pub archive: Provider,
    pub metrics: Arc<Metrics>,
}

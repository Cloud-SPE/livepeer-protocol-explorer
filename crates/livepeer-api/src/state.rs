use crate::metrics::Metrics;
use sqlx::PgPool;
use std::sync::Arc;

#[derive(Clone)]
pub struct AppState {
    pub pg: PgPool,
    pub default_version: String,
    pub chain_id: i64,
    pub metrics: Arc<Metrics>,
}

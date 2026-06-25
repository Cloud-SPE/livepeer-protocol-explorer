use crate::metrics::Metrics;
use livepeer_core::rpc::Provider;
use sqlx::PgPool;
use std::path::PathBuf;
use std::sync::Arc;

#[derive(Clone)]
pub struct AppState {
    pub pg: PgPool,
    pub default_version: String,
    pub chain_id: i64,
    pub ticket_broker_address: String,
    pub archive: Provider,
    pub metrics: Arc<Metrics>,
    /// TD-033: directory of locally-cached avatar files written by the
    /// enricher (`<address>.<ext>`). Shared volume; `None` disables local
    /// avatar serving. Mirrors the enricher's `AVATAR_STORE_DIR`.
    pub avatar_dir: Option<PathBuf>,
}

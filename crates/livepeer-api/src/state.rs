use sqlx::PgPool;

#[derive(Clone)]
pub struct AppState {
    pub pg: PgPool,
    pub default_version: String,
    pub chain_id: i64,
}

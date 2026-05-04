use crate::metrics::Metrics;
use anyhow::{Context, Result};
use axum::{extract::State, http::header, response::IntoResponse, routing::get, Router};
use prometheus::{Encoder, TextEncoder};
use std::{net::SocketAddr, sync::Arc};

pub async fn serve(bind: &str, metrics: Arc<Metrics>) -> Result<()> {
    let addr: SocketAddr = bind.parse().context("parsing daemon metrics bind address")?;
    let app = Router::new()
        .route("/health", get(health))
        .route("/metrics", get(metrics_handler))
        .with_state(metrics);
    let listener = tokio::net::TcpListener::bind(addr)
        .await
        .with_context(|| format!("binding daemon metrics server on {addr}"))?;
    axum::serve(listener, app)
        .await
        .context("serving daemon metrics HTTP")
}

async fn health() -> &'static str {
    "ok"
}

async fn metrics_handler(State(metrics): State<Arc<Metrics>>) -> impl IntoResponse {
    let encoder = TextEncoder::new();
    let mut buf = Vec::new();
    let mut families = metrics.registry.gather();
    families.extend(livepeer_core::rpc::metrics::gather());
    if encoder.encode(&families, &mut buf).is_err() {
        return (axum::http::StatusCode::INTERNAL_SERVER_ERROR, "encode failed").into_response();
    }
    (
        [(header::CONTENT_TYPE, encoder.format_type().to_string())],
        String::from_utf8(buf).unwrap_or_default(),
    )
        .into_response()
}

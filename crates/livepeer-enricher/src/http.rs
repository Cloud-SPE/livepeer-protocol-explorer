use crate::metrics::Metrics;
use anyhow::{Context, Result};
use axum::{extract::State, response::IntoResponse, routing::get, Router};
use prometheus::{Encoder, TextEncoder};
use std::{net::SocketAddr, sync::Arc};

pub async fn serve(bind: &str, metrics: Arc<Metrics>) -> Result<()> {
    let addr: SocketAddr = bind
        .parse()
        .context("parsing enricher metrics bind address")?;
    let app = Router::new()
        .route("/health", get(health))
        .route("/metrics", get(metrics_handler))
        .with_state(metrics);
    let listener = tokio::net::TcpListener::bind(addr)
        .await
        .with_context(|| format!("binding enricher metrics server on {addr}"))?;
    axum::serve(listener, app)
        .await
        .context("serving enricher metrics HTTP")
}

async fn health() -> &'static str {
    "ok"
}

async fn metrics_handler(State(metrics): State<Arc<Metrics>>) -> impl IntoResponse {
    let encoder = TextEncoder::new();
    let families = metrics.registry.gather();
    let mut buf = Vec::new();
    if encoder.encode(&families, &mut buf).is_err() {
        return (
            axum::http::StatusCode::INTERNAL_SERVER_ERROR,
            "metrics encode error".to_string(),
        );
    }
    (
        axum::http::StatusCode::OK,
        String::from_utf8_lossy(&buf).to_string(),
    )
}

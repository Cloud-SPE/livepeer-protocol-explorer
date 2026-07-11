use crate::metrics::{now_unix, Metrics};
use anyhow::{Context, Result};
use axum::{
    extract::State, http::header, http::StatusCode, response::IntoResponse, routing::get, Router,
};
use prometheus::{Encoder, TextEncoder};
use std::{net::SocketAddr, sync::Arc};

/// Per-task max heartbeat age (seconds) before `/health` reports the daemon
/// unhealthy. Roughly `k × cadence`: fast loops surface a stall in minutes
/// while the 300s staker keeps margin. The indexer threshold must exceed a
/// legitimate multi-minute chunk stall (backfill's 50-retry capped backoff).
/// `matview` is intentionally excluded — its staleness is cosmetic (stale
/// profiles) and must not restart the whole daemon.
const HEALTH_THRESHOLDS: &[(&str, i64)] = &[
    ("indexer", 300),
    ("finality", 300),
    ("reorg", 300),
    ("valuator", 300),
    ("staker", 900),
];

/// Grace period after startup during which `/health` always reports OK, so a
/// slow first iteration (or the Docker `start_period`) doesn't cause a restart
/// loop before the loops have had a chance to run.
const HEALTH_START_GRACE_SECS: i64 = 120;

#[derive(Clone)]
struct HttpState {
    metrics: Arc<Metrics>,
    started_unix: i64,
}

pub async fn serve(bind: &str, metrics: Arc<Metrics>) -> Result<()> {
    let addr: SocketAddr = bind
        .parse()
        .context("parsing daemon metrics bind address")?;
    let state = HttpState {
        metrics,
        started_unix: now_unix(),
    };
    let app = Router::new()
        .route("/health", get(health))
        .route("/metrics", get(metrics_handler))
        .with_state(state);
    let listener = tokio::net::TcpListener::bind(addr)
        .await
        .with_context(|| format!("binding daemon metrics server on {addr}"))?;
    axum::serve(listener, app)
        .await
        .context("serving daemon metrics HTTP")
}

/// Liveness probe used by the Docker healthcheck. Returns 503 when any gated
/// task's heartbeat is stale or the task has been escalated (`task_up==0`),
/// so a wedged or permanently-broken loop becomes a whole-container restart
/// instead of a silent partial stall. Reads the in-process heartbeat gauges —
/// no DB query — which uniformly covers loops that own no DB checkpoint
/// (valuator, reorg).
async fn health(State(state): State<HttpState>) -> impl IntoResponse {
    match health_status(&state.metrics, now_unix(), state.started_unix) {
        Ok(msg) => (StatusCode::OK, msg),
        Err(msg) => (StatusCode::SERVICE_UNAVAILABLE, msg),
    }
}

/// Pure health decision (testable). Ok = healthy, Err(reason) = unhealthy.
fn health_status(metrics: &Metrics, now: i64, started_unix: i64) -> Result<String, String> {
    // Startup grace: don't fail health until the loops have had time to run.
    if now - started_unix <= HEALTH_START_GRACE_SECS {
        return Ok("ok (startup grace)".to_string());
    }

    let mut unhealthy = Vec::new();
    for (task, threshold) in HEALTH_THRESHOLDS {
        let hb = metrics.heartbeat(task);
        let age = now - hb;
        let stale = age > *threshold;
        let down = metrics.task_up_value(task) == 0;
        if stale || down {
            unhealthy.push(format!(
                "{task}(age={age}s,threshold={threshold}s,up={})",
                if down { 0 } else { 1 }
            ));
        }
    }

    if unhealthy.is_empty() {
        Ok("ok".to_string())
    } else {
        Err(format!("unhealthy: {}", unhealthy.join(", ")))
    }
}

async fn metrics_handler(State(state): State<HttpState>) -> impl IntoResponse {
    let encoder = TextEncoder::new();
    let mut buf = Vec::new();
    let mut families = state.metrics.registry.gather();
    families.extend(livepeer_core::rpc::metrics::gather());
    families.extend(livepeer_staker::metrics::gather());
    if encoder.encode(&families, &mut buf).is_err() {
        return (
            axum::http::StatusCode::INTERNAL_SERVER_ERROR,
            "encode failed",
        )
            .into_response();
    }
    (
        [(header::CONTENT_TYPE, encoder.format_type().to_string())],
        String::from_utf8(buf).unwrap_or_default(),
    )
        .into_response()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::metrics::Metrics;

    #[test]
    fn healthy_when_heartbeats_fresh() {
        let m = Metrics::new();
        let now = 1_000_000i64;
        for (task, _) in HEALTH_THRESHOLDS {
            m.task_last_success_timestamp
                .with_label_values(&[task])
                .set(now);
            m.set_task_up(task, true);
        }
        // started long ago so we're past the grace window.
        assert!(health_status(&m, now, now - 10_000).is_ok());
    }

    #[test]
    fn unhealthy_when_a_task_is_stale() {
        let m = Metrics::new();
        let now = 1_000_000i64;
        for (task, _) in HEALTH_THRESHOLDS {
            m.task_last_success_timestamp
                .with_label_values(&[task])
                .set(now);
            m.set_task_up(task, true);
        }
        // Make the indexer heartbeat 10 minutes old (> 300s threshold).
        m.task_last_success_timestamp
            .with_label_values(&["indexer"])
            .set(now - 600);
        let err = health_status(&m, now, now - 10_000).unwrap_err();
        assert!(err.contains("indexer"), "got: {err}");
    }

    #[test]
    fn unhealthy_when_a_task_is_down() {
        let m = Metrics::new();
        let now = 1_000_000i64;
        for (task, _) in HEALTH_THRESHOLDS {
            m.task_last_success_timestamp
                .with_label_values(&[task])
                .set(now);
            m.set_task_up(task, true);
        }
        m.set_task_up("valuator", false);
        let err = health_status(&m, now, now - 10_000).unwrap_err();
        assert!(err.contains("valuator"), "got: {err}");
    }

    #[test]
    fn healthy_during_startup_grace() {
        let m = Metrics::new();
        let now = 1_000_000i64;
        // All stale, but within the grace window → still OK.
        assert!(health_status(&m, now, now - 10).is_ok());
    }
}

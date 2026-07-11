//! Transport wiring. Two modes behind one binary:
//! - `stdio`: the MCP client spawns this process (local dev). The JSON-RPC
//!   stream owns stdout, so logs MUST go to stderr (handled in `main`).
//! - `http`: Streamable HTTP for the co-deployed prod container, mounted on an
//!   axum router with a `/health` route and a bearer-auth layer on `/mcp`.

use crate::context::DiagContext;
use crate::server::DiagServer;
use anyhow::{Context, Result};
use axum::{
    extract::{Request, State},
    http::{header::AUTHORIZATION, StatusCode},
    middleware::{self, Next},
    response::Response,
    routing::get,
    Router,
};
use rmcp::transport::streamable_http_server::{
    session::local::LocalSessionManager, StreamableHttpServerConfig, StreamableHttpService,
};
use rmcp::ServiceExt;
use std::sync::Arc;
use tracing::{info, warn};

/// stdio transport — client-spawned subprocess. Blocks until the peer closes.
pub async fn serve_stdio(ctx: Arc<DiagContext>) -> Result<()> {
    let server = DiagServer::new(ctx);
    let running = server
        .serve((tokio::io::stdin(), tokio::io::stdout()))
        .await
        .context("starting stdio MCP service")?;
    running.waiting().await.context("stdio MCP service")?;
    Ok(())
}

/// Streamable HTTP transport for prod. `bind` is intra-network only — never
/// publish it; reach it via SSH tunnel / VPN. `bearer` is the second factor.
///
/// `allowed_hosts` controls rmcp's DNS-rebinding `Host` allowlist (default
/// localhost/127.0.0.1/::1). Behind a reverse proxy / Cloudflare tunnel the
/// inbound Host is the public domain, which would otherwise 403 — set this to
/// that hostname (comma-separated for several), or `*` to disable the check
/// entirely (safe here: the endpoint is already gated by the bearer token and,
/// typically, Cloudflare Access).
pub async fn serve_http(
    ctx: Arc<DiagContext>,
    bind: &str,
    bearer: String,
    allowed_hosts: Option<String>,
) -> Result<()> {
    let mut http_cfg = StreamableHttpServerConfig::default();
    match allowed_hosts.as_deref().map(str::trim) {
        None | Some("") => {}
        Some("*") => {
            warn!("DIAG_ALLOWED_HOSTS=* — Host allowlist disabled; relying on bearer/Access");
            http_cfg = http_cfg.disable_allowed_hosts();
        }
        Some(list) => {
            let hosts: Vec<String> = list
                .split(',')
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty())
                .collect();
            info!(?hosts, "restricting MCP Host header to allowlist");
            http_cfg = http_cfg.with_allowed_hosts(hosts);
        }
    }

    let session_ctx = ctx.clone();
    let service = StreamableHttpService::new(
        move || Ok(DiagServer::new(session_ctx.clone())),
        LocalSessionManager::default().into(),
        http_cfg,
    );

    let token = Arc::new(bearer);
    let mcp = Router::new()
        .nest_service("/mcp", service)
        .layer(middleware::from_fn_with_state(token, require_bearer));

    let app = Router::new().route("/health", get(health)).merge(mcp);

    let listener = tokio::net::TcpListener::bind(bind)
        .await
        .with_context(|| format!("binding diag MCP HTTP server on {bind}"))?;
    info!(bind, "diag MCP streamable-http server starting (bearer-protected /mcp)");
    axum::serve(listener, app).await.context("serving diag MCP HTTP")?;
    Ok(())
}

async fn health() -> &'static str {
    "ok"
}

/// Require `Authorization: Bearer <token>` on `/mcp`. Constant-time compare.
async fn require_bearer(
    State(token): State<Arc<String>>,
    req: Request,
    next: Next,
) -> Result<Response, StatusCode> {
    let expected = format!("Bearer {}", token.as_str());
    let presented = req
        .headers()
        .get(AUTHORIZATION)
        .and_then(|h| h.to_str().ok())
        .unwrap_or("");
    if constant_time_eq(presented.as_bytes(), expected.as_bytes()) {
        Ok(next.run(req).await)
    } else {
        warn!("rejected MCP request with missing/invalid bearer token");
        Err(StatusCode::UNAUTHORIZED)
    }
}

/// Length-independent constant-time byte comparison.
fn constant_time_eq(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    let mut diff = 0u8;
    for (x, y) in a.iter().zip(b.iter()) {
        diff |= x ^ y;
    }
    diff == 0
}

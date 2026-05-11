pub mod abi;
pub mod cursor;
pub mod error;
pub mod metrics;
pub mod openapi;
pub mod routes;
pub mod state;

use axum::body::{Body, Bytes};
use axum::extract::Request;
use axum::http::{header, HeaderValue, Method, Response, StatusCode};
use axum::middleware::{self, Next};
use axum::{routing::get, Router};
use sha2::{Digest, Sha256};
use state::AppState;
use std::convert::Infallible;
use tower_http::{
    cors::{AllowOrigin, Any, CorsLayer},
    services::ServeDir,
    trace::TraceLayer,
};
use utoipa::OpenApi;
use utoipa_swagger_ui::SwaggerUi;

pub fn build_router(state: AppState) -> Router {
    Router::new()
        // Operational
        .route("/health", get(routes::operational::health))
        .route("/metrics", get(routes::operational::metrics))
        .route(
            "/backfills/status",
            get(routes::operational::backfill_status),
        )
        // Frontend runtime config — env-driven, served from the same origin
        // as the FE bundle. Registered before the static fallback so this
        // route wins over /opt/livepeer/frontend-ui/dist/config.json.
        .route("/config.json", get(routes::operational::frontend_config))
        // Events
        .route("/events", get(routes::events::list))
        .route("/events/{id}", get(routes::events::get_one))
        .route("/events/{id}/valuation", get(routes::valuations::for_event))
        // Valuations
        .route("/valuations", get(routes::valuations::list))
        // Aggregations
        .route("/aggregations/events", get(routes::aggregations::events))
        .route("/payouts/leaderboard", get(routes::payouts::leaderboard))
        .route(
            "/payouts/summary/daily/{date}",
            get(routes::payouts::summary_daily),
        )
        .route(
            "/payouts/summary/weekly/{date}",
            get(routes::payouts::summary_weekly),
        )
        .route(
            "/payouts/summary/monthly/{date}",
            get(routes::payouts::summary_monthly),
        )
        .route("/rewards/leaderboard", get(routes::rewards::leaderboard))
        .route(
            "/rewards/summary/daily/{date}",
            get(routes::rewards::summary_daily),
        )
        .route(
            "/rewards/summary/weekly/{date}",
            get(routes::rewards::summary_weekly),
        )
        .route(
            "/rewards/summary/monthly/{date}",
            get(routes::rewards::summary_monthly),
        )
        .route(
            "/tickets/timeseries/daily",
            get(routes::tickets::timeseries_daily),
        )
        .route("/reports/payouts.csv", get(routes::reports::payouts_csv))
        .route("/reports/rewards.csv", get(routes::reports::rewards_csv))
        .route(
            "/reports/gateway-payouts.csv",
            get(routes::reports::gateway_payouts_csv),
        )
        .route(
            "/orchestrators/{address}/tickets/latest",
            get(routes::reports::orchestrator_tickets_latest),
        )
        .route(
            "/gateways/{address}/tickets",
            get(routes::reports::gateway_tickets),
        )
        // Governance
        .route("/governance/proposals", get(routes::governance::list))
        .route(
            "/governance/proposals/{proposal_id}",
            get(routes::governance::get_one),
        )
        .route("/governance/votes", get(routes::governance::votes))
        // Prices
        .route(
            "/prices/{asset}/{quote}/block/{block}",
            get(routes::prices::at_block),
        )
        .route(
            "/prices/{asset}/{quote}/latest",
            get(routes::prices::latest),
        )
        .route("/prices/{asset}/{quote}/range", get(routes::prices::range))
        // Gateways
        .route(
            "/gateways/{gateway}/balance/latest",
            get(routes::gateways::balance_latest),
        )
        .route(
            "/gateways/{gateway}/balance/block/{block}",
            get(routes::gateways::balance_at_block),
        )
        .route(
            "/gateways/{gateway}/balance/history",
            get(routes::gateways::balance_history),
        )
        .route(
            "/gateways/{gateway}/claimants/block/{block}",
            get(routes::gateways::claimants_at_block),
        )
        .route(
            "/gateways/{gateway}/claimants/history",
            get(routes::gateways::claimants_history),
        )
        .route("/gateways/{gateway}/flows", get(routes::gateways::flows))
        .route(
            "/gateways/{gateway}/payouts",
            get(routes::gateways::payouts),
        )
        .route(
            "/gateways/{gateway}/recipients",
            get(routes::gateways::recipients),
        )
        .route(
            "/gateways/{gateway}/summary",
            get(routes::gateways::summary),
        )
        .route(
            "/gateways/{gateway}/analytics/summary",
            get(routes::gateways::analytics_summary),
        )
        .route("/gateways", get(routes::profiles::gateways_list))
        .route(
            "/gateways/{address}/profile",
            get(routes::profiles::gateways_get),
        )
        .route("/orchestrators", get(routes::profiles::orchestrators_list))
        .route(
            "/orchestrators/{address}",
            get(routes::profiles::orchestrators_get),
        )
        // Orchestrator history (TD-027)
        .route(
            "/orchestrators/{address}/stake-history",
            get(routes::profiles::orchestrators_stake_history),
        )
        .route(
            "/orchestrators/{address}/cuts-history",
            get(routes::profiles::orchestrators_cuts_history),
        )
        .route(
            "/orchestrators/{address}/net-economics",
            get(routes::profiles::orchestrators_net_economics),
        )
        // Delegators (TD-027)
        .route("/rounds", get(routes::network::rounds_index))
        .route(
            "/rounds/{round_id}/events",
            get(routes::network::round_events),
        )
        .route(
            "/rounds/{round_id}/event-counts",
            get(routes::network::round_event_counts),
        )
        .route("/delegators", get(routes::delegators::list))
        .route("/delegators/{address}", get(routes::delegators::get))
        .route(
            "/delegators/{address}/events",
            get(routes::delegators::events_for),
        )
        .route(
            "/orchestrators/{address}/delegators",
            get(routes::delegators::for_orchestrator),
        )
        // Network-level (TD-027)
        .route("/network/stats", get(routes::network::stats))
        .route("/rounds/{round_id}", get(routes::network::round_get))
        // Stake
        .route(
            "/stake/{delegator}/block/{block}",
            get(routes::stake::at_block),
        )
        .route("/stake/{delegator}/range", get(routes::stake::range))
        .route(
            "/transcoders/{transcoder}/delegators/block/{block}",
            get(routes::stake::delegators_at_block),
        )
        // Transcoders
        .route(
            "/transcoders/{transcoder}/params/latest",
            get(routes::transcoders::latest),
        )
        .route(
            "/transcoders/{transcoder}/params/block/{block}",
            get(routes::transcoders::at_block),
        )
        .route(
            "/transcoders/{transcoder}/params/history",
            get(routes::transcoders::history),
        )
        .route(
            "/transcoders/{transcoder}/lifecycle/latest",
            get(routes::transcoders::lifecycle_latest),
        )
        .route(
            "/transcoders/{transcoder}/lifecycle/block/{block}",
            get(routes::transcoders::lifecycle_at_block),
        )
        .route(
            "/transcoders/{transcoder}/lifecycle/history",
            get(routes::transcoders::lifecycle_history),
        )
        .route(
            "/transcoders/{transcoder}/profile/block/{block}",
            get(routes::transcoders::profile_at_block),
        )
        .with_state(state)
        // CORS: env-driven so other UIs / third-party callers can hit the
        // API cross-origin. The bundled FE is same-origin (served by this
        // process) and doesn't need CORS for itself.
        // See `build_cors_layer` for the env contract.
        .layer(build_cors_layer())
        .layer(TraceLayer::new_for_http())
        .merge(SwaggerUi::new("/docs").url("/openapi.json", openapi::ApiDoc::openapi()))
        // Static frontend bundle. FE_STATIC_DIR overrides; defaults to the
        // Dockerfile install path. ServeDir handles known assets (proper
        // content-type, range requests, last-modified). For unknown paths
        // we fall back to a small in-memory index.html service that returns
        // 200 OK — required for SPA deep-linking, since
        // `ServeDir::not_found_service` would otherwise propagate the 404
        // even though the body is correct. API routes registered above
        // always win over this fallback (axum Router precedence).
        .fallback_service(static_frontend_service())
        // Cache-Control headers per response (decides via path + content
        // type so API responses are left untouched).
        .layer(middleware::from_fn(static_cache_headers))
}

/// Add `Cache-Control` headers to responses produced by the static-frontend
/// surface. Decision matrix (in order):
///
/// | Path / Content-Type                 | `Cache-Control`                          |
/// |-------------------------------------|------------------------------------------|
/// | `/assets/*`                         | `public, max-age=31536000, immutable`    |
/// | `/config.json`                      | `no-store`                               |
/// | response is `text/html*`            | `no-cache, must-revalidate`              |
/// | response is `image/*` or `font/*`   | `public, max-age=86400`                  |
/// | anything else (API JSON, plain)     | unchanged — caller controls              |
///
/// Hashed `/assets/*` filenames are content-addressed by Vite, so they're
/// safe to cache forever. The SPA entrypoint (`index.html`, served either
/// directly at `/` or as the fallback for SPA deep-links) revalidates on
/// every load so a fresh deploy lands within one page reload. `/config.json`
/// is intentionally never cached so env-driven changes take effect on the
/// next page load. API responses are untouched — leave their cache policy
/// to the originating handler.
async fn static_cache_headers(req: Request, next: Next) -> Response<Body> {
    let path = req.uri().path().to_string();
    let mut resp = next.run(req).await;

    let cache_value: Option<&'static str> = if path.starts_with("/assets/") {
        Some("public, max-age=31536000, immutable")
    } else if path == "/config.json" {
        Some("no-store")
    } else {
        let content_type = resp
            .headers()
            .get(header::CONTENT_TYPE)
            .and_then(|v| v.to_str().ok())
            .unwrap_or("");
        if content_type.starts_with("text/html") {
            Some("no-cache, must-revalidate")
        } else if content_type.starts_with("image/") || content_type.starts_with("font/") {
            Some("public, max-age=86400")
        } else {
            None
        }
    };

    if let Some(val) = cache_value {
        resp.headers_mut()
            .insert(header::CACHE_CONTROL, HeaderValue::from_static(val));
    }
    resp
}

/// Build a CORS layer from the `CORS_ALLOWED_ORIGINS` env var.
///
/// Values:
/// - unset, empty, or `"*"` → permissive (any origin). Browsers treat this
///   as anonymous-only — credentials/cookies are not allowed.
/// - comma-separated list of origins (e.g.
///   `"https://stats.example.com,https://portal.example.com"`) → only
///   those exact origins are allowed.
///
/// Methods (`GET`, `POST`) and headers (`Content-Type`, `Authorization`)
/// are always permitted; the env var only controls origin allow-listing.
fn build_cors_layer() -> CorsLayer {
    let raw = std::env::var("CORS_ALLOWED_ORIGINS").unwrap_or_else(|_| "*".to_string());
    let trimmed = raw.trim();
    let layer = CorsLayer::new()
        .allow_methods([Method::GET, Method::POST])
        .allow_headers([header::CONTENT_TYPE, header::AUTHORIZATION]);
    if trimmed.is_empty() || trimmed == "*" {
        layer.allow_origin(Any)
    } else {
        let origins: Vec<HeaderValue> = trimmed
            .split(',')
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .filter_map(|s| HeaderValue::from_str(s).ok())
            .collect();
        layer.allow_origin(AllowOrigin::list(origins))
    }
}

/// Build the static-frontend fallback. Reads `index.html` once at boot so
/// we can serve SPA deep-links as `200 OK` rather than the `404` that
/// `ServeDir::not_found_service` would otherwise force via `SetStatus`.
/// Also computes a content-based ETag at boot so revalidation is cheap.
fn static_frontend_service() -> ServeDir<SpaIndex> {
    let dir = std::env::var("FE_STATIC_DIR")
        .unwrap_or_else(|_| "/opt/livepeer/frontend-ui/dist".to_string());
    let index_path = std::path::PathBuf::from(&dir).join("index.html");
    let index_bytes = std::fs::read(&index_path)
        .map(Bytes::from)
        .unwrap_or_else(|_| Bytes::from_static(b""));
    let etag = etag_from_bytes(&index_bytes);
    // `.fallback()` (not `.not_found_service()`) preserves the inner
    // service's status, so SpaIndex's 200 actually reaches the client.
    ServeDir::new(dir).fallback(SpaIndex { index_bytes, etag })
}

/// Compute an `ETag` value from the bytes of a response body. Uses the
/// first 16 hex chars of SHA-256 (~64 bits of collision resistance —
/// plenty for cache-validation correctness on a single-deploy bundle).
/// The `"…"` quotes around the value are required by RFC 7232 §2.3.
fn etag_from_bytes(bytes: &[u8]) -> HeaderValue {
    let mut h = Sha256::new();
    h.update(bytes);
    let digest = h.finalize();
    let hex: String = digest.iter().take(8).map(|b| format!("{b:02x}")).collect();
    HeaderValue::from_str(&format!("\"{hex}\""))
        .expect("hex-formatted ETag is always a valid HeaderValue")
}

/// Always returns the cached `index.html` bytes with `200 OK` (or `304 Not
/// Modified` when the client's `If-None-Match` matches our ETag). Used as
/// the fallback for the static dir so SPA deep-links don't 404.
#[derive(Clone)]
pub struct SpaIndex {
    index_bytes: Bytes,
    etag: HeaderValue,
}

impl<R> tower::Service<axum::http::Request<R>> for SpaIndex
where
    R: Send + 'static,
{
    type Response = Response<Body>;
    type Error = Infallible;
    type Future = std::future::Ready<Result<Self::Response, Infallible>>;

    fn poll_ready(
        &mut self,
        _cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<Result<(), Infallible>> {
        std::task::Poll::Ready(Ok(()))
    }

    fn call(&mut self, req: axum::http::Request<R>) -> Self::Future {
        // Bundle missing → diagnostic 404 (no caching makes sense here).
        if self.index_bytes.is_empty() {
            let resp = Response::builder()
                .status(StatusCode::NOT_FOUND)
                .header(
                    header::CONTENT_TYPE,
                    HeaderValue::from_static("text/plain; charset=utf-8"),
                )
                .body(Body::from(
                    "frontend bundle not found; set FE_STATIC_DIR or place index.html",
                ))
                .expect("static index response");
            return std::future::ready(Ok(resp));
        }

        // Cache-Control set here too (not just in `static_cache_headers`)
        // so 304 responses — which have no Content-Type for the middleware
        // to switch on — still tell clients how to treat their cached entry.
        let cache_control = HeaderValue::from_static("no-cache, must-revalidate");

        // Conditional request: if the client's ETag matches ours, return
        // 304 Not Modified with no body. Saves the round-trip body for
        // SPA deep-links that revalidate on every load (no-cache directive).
        let if_none_match = req.headers().get(header::IF_NONE_MATCH);
        if if_none_match.is_some_and(|v| v == self.etag) {
            let resp = Response::builder()
                .status(StatusCode::NOT_MODIFIED)
                .header(header::ETAG, self.etag.clone())
                .header(header::CACHE_CONTROL, cache_control)
                .body(Body::empty())
                .expect("304 response");
            return std::future::ready(Ok(resp));
        }

        let resp = Response::builder()
            .status(StatusCode::OK)
            .header(
                header::CONTENT_TYPE,
                HeaderValue::from_static("text/html; charset=utf-8"),
            )
            .header(header::ETAG, self.etag.clone())
            .header(header::CACHE_CONTROL, cache_control)
            .body(Body::from(self.index_bytes.clone()))
            .expect("static index response");
        std::future::ready(Ok(resp))
    }
}

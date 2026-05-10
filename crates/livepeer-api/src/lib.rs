pub mod abi;
pub mod cursor;
pub mod error;
pub mod metrics;
pub mod openapi;
pub mod routes;
pub mod state;

use axum::{
    http::{header, Method},
    routing::get,
    Router,
};
use state::AppState;
use tower_http::{
    cors::{Any, CorsLayer},
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
        // CORS: permissive for now (read-only data API). Tighten by replacing
        // `Any` with a specific origin once the FE host is fixed in prod.
        .layer(
            CorsLayer::new()
                .allow_origin(Any)
                .allow_methods([Method::GET, Method::POST])
                .allow_headers([header::CONTENT_TYPE, header::AUTHORIZATION]),
        )
        .layer(TraceLayer::new_for_http())
        .merge(SwaggerUi::new("/docs").url("/openapi.json", openapi::ApiDoc::openapi()))
}

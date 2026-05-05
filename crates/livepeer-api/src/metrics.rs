//! Prometheus metrics — SPEC §17.2 catalog (subset; expand as endpoints grow).
//!
//! The registry is kept on `AppState` and each handler increments the relevant
//! counters. `/metrics` exposes the standard text-format exposition.

use prometheus::{opts, IntCounterVec, Registry};

#[derive(Clone)]
pub struct Metrics {
    pub registry: Registry,
    /// `api_requests_total{route, status}` — counts every API request by route and status group.
    pub api_requests_total: IntCounterVec,
}

impl Metrics {
    pub fn new() -> Self {
        let registry = Registry::new();
        let api_requests_total = IntCounterVec::new(
            opts!(
                "api_requests_total",
                "Total HTTP requests served, labeled by route and status group"
            ),
            &["route", "status"],
        )
        .expect("metric construction");
        registry
            .register(Box::new(api_requests_total.clone()))
            .expect("metric registration");
        Self {
            registry,
            api_requests_total,
        }
    }
}

impl Default for Metrics {
    fn default() -> Self {
        Self::new()
    }
}

use prometheus::{opts, IntCounterVec, IntGauge, Registry};

#[derive(Clone, Debug)]
pub struct Metrics {
    pub registry: Registry,
    pub sweeps_total: IntCounterVec,
    pub rows_updated_total: IntCounterVec,
    pub rows_named_total: IntCounterVec,
    pub resolve_failures_total: IntCounterVec,
    pub breaker_open_total: IntCounterVec,
    pub breaker_open: IntGauge,
}

impl Metrics {
    pub fn new() -> Self {
        let registry = Registry::new();
        let sweeps_total = IntCounterVec::new(
            opts!(
                "livepeer_enricher_sweeps_total",
                "Completed ENS sweep iterations by result"
            ),
            &["result"],
        )
        .expect("metric construction");
        let rows_updated_total = IntCounterVec::new(
            opts!(
                "livepeer_enricher_rows_updated_total",
                "ENS projection rows updated by entity type"
            ),
            &["entity"],
        )
        .expect("metric construction");
        let rows_named_total = IntCounterVec::new(
            opts!(
                "livepeer_enricher_named_rows_total",
                "Resolved ENS name/avatar hits by entity type and field"
            ),
            &["entity", "field"],
        )
        .expect("metric construction");
        let resolve_failures_total = IntCounterVec::new(
            opts!(
                "livepeer_enricher_resolve_failures_total",
                "Failed ENS resolutions by entity type"
            ),
            &["entity"],
        )
        .expect("metric construction");
        let breaker_open_total = IntCounterVec::new(
            opts!(
                "livepeer_enricher_breaker_open_total",
                "Number of times the ENS L1 failure breaker opened"
            ),
            &["reason"],
        )
        .expect("metric construction");
        let breaker_open = IntGauge::new(
            "livepeer_enricher_breaker_open",
            "Whether the ENS L1 failure breaker is currently open (1) or closed (0)",
        )
        .expect("metric construction");

        for collector in [
            Box::new(sweeps_total.clone()) as Box<dyn prometheus::core::Collector>,
            Box::new(rows_updated_total.clone()),
            Box::new(rows_named_total.clone()),
            Box::new(resolve_failures_total.clone()),
            Box::new(breaker_open_total.clone()),
            Box::new(breaker_open.clone()),
        ] {
            registry.register(collector).expect("metric registration");
        }

        Self {
            registry,
            sweeps_total,
            rows_updated_total,
            rows_named_total,
            resolve_failures_total,
            breaker_open_total,
            breaker_open,
        }
    }
}

impl Default for Metrics {
    fn default() -> Self {
        Self::new()
    }
}

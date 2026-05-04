use prometheus::{
    opts, proto::MetricFamily, HistogramOpts, HistogramVec, IntCounterVec, Registry,
};
use std::sync::OnceLock;

pub struct RpcMetrics {
    pub registry: Registry,
    pub rpc_calls_total: IntCounterVec,
    pub rpc_call_duration_seconds: HistogramVec,
    pub rpc_divergence_total: IntCounterVec,
}

fn global() -> &'static RpcMetrics {
    static METRICS: OnceLock<RpcMetrics> = OnceLock::new();
    METRICS.get_or_init(|| {
        let registry = Registry::new();
        let rpc_calls_total = IntCounterVec::new(
            opts!(
                "livepeer_rpc_calls_total",
                "RPC calls by provider, method, and result"
            ),
            &["provider", "method", "result"],
        )
        .expect("metric construction");
        let rpc_call_duration_seconds = HistogramVec::new(
            HistogramOpts::new(
                "livepeer_rpc_call_duration_seconds",
                "RPC call duration in seconds by provider and method",
            ),
            &["provider", "method"],
        )
        .expect("metric construction");
        let rpc_divergence_total = IntCounterVec::new(
            opts!(
                "livepeer_rpc_divergence_total",
                "RPC divergence failures by method"
            ),
            &["method"],
        )
        .expect("metric construction");

        for collector in [
            Box::new(rpc_calls_total.clone()) as Box<dyn prometheus::core::Collector>,
            Box::new(rpc_call_duration_seconds.clone()),
            Box::new(rpc_divergence_total.clone()),
        ] {
            registry.register(collector).expect("metric registration");
        }

        RpcMetrics {
            registry,
            rpc_calls_total,
            rpc_call_duration_seconds,
            rpc_divergence_total,
        }
    })
}

pub fn record_call(provider: &str, method: &str, result: &str, duration_seconds: f64) {
    let metrics = global();
    metrics
        .rpc_calls_total
        .with_label_values(&[provider, method, result])
        .inc();
    metrics
        .rpc_call_duration_seconds
        .with_label_values(&[provider, method])
        .observe(duration_seconds);
}

pub fn record_divergence(method: &str) {
    global()
        .rpc_divergence_total
        .with_label_values(&[method])
        .inc();
}

pub fn gather() -> Vec<MetricFamily> {
    global().registry.gather()
}

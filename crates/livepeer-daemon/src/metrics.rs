use anyhow::Error;
use prometheus::{opts, HistogramOpts, HistogramVec, IntCounterVec, IntGauge, IntGaugeVec, Registry};

#[derive(Clone)]
pub struct Metrics {
    pub registry: Registry,
    pub iterations_total: IntCounterVec,
    pub iteration_failures_total: IntCounterVec,
    pub iteration_duration_seconds: HistogramVec,
    pub chain_head_block: IntGauge,
    pub task_checkpoint_block: IntGaugeVec,
    pub task_lag_blocks: IntGaugeVec,
}

impl Metrics {
    pub fn new() -> Self {
        let registry = Registry::new();

        let iterations_total = IntCounterVec::new(
            opts!("livepeer_iterations_total", "Successful daemon iterations by task"),
            &["task"],
        )
        .expect("metric construction");
        let iteration_failures_total = IntCounterVec::new(
            opts!(
                "livepeer_iteration_failures_total",
                "Failed daemon iterations by task and error kind"
            ),
            &["task", "error_kind"],
        )
        .expect("metric construction");
        let iteration_duration_seconds = HistogramVec::new(
            HistogramOpts::new(
                "livepeer_iteration_duration_seconds",
                "Daemon iteration duration in seconds by task",
            ),
            &["task"],
        )
        .expect("metric construction");
        let chain_head_block = IntGauge::new(
            "livepeer_chain_head_block",
            "Latest chain head block observed by the daemon",
        )
        .expect("metric construction");
        let task_checkpoint_block = IntGaugeVec::new(
            opts!(
                "livepeer_task_checkpoint_block",
                "Latest checkpoint-like block observed by daemon task"
            ),
            &["task"],
        )
        .expect("metric construction");
        let task_lag_blocks = IntGaugeVec::new(
            opts!("livepeer_task_lag_blocks", "Per-task lag in blocks"),
            &["task"],
        )
        .expect("metric construction");

        for collector in [
            Box::new(iterations_total.clone()) as Box<dyn prometheus::core::Collector>,
            Box::new(iteration_failures_total.clone()),
            Box::new(iteration_duration_seconds.clone()),
            Box::new(chain_head_block.clone()),
            Box::new(task_checkpoint_block.clone()),
            Box::new(task_lag_blocks.clone()),
        ] {
            registry.register(collector).expect("metric registration");
        }

        Self {
            registry,
            iterations_total,
            iteration_failures_total,
            iteration_duration_seconds,
            chain_head_block,
            task_checkpoint_block,
            task_lag_blocks,
        }
    }

    pub fn record_success(&self, task: &'static str, duration_seconds: f64) {
        self.iterations_total.with_label_values(&[task]).inc();
        self.iteration_duration_seconds
            .with_label_values(&[task])
            .observe(duration_seconds);
    }

    pub fn record_failure(&self, task: &'static str, error: &Error, duration_seconds: f64) {
        self.iteration_failures_total
            .with_label_values(&[task, classify_error(error)])
            .inc();
        self.iteration_duration_seconds
            .with_label_values(&[task])
            .observe(duration_seconds);
    }
}

impl Default for Metrics {
    fn default() -> Self {
        Self::new()
    }
}

fn classify_error(error: &Error) -> &'static str {
    let text = error.to_string().to_ascii_lowercase();
    if text.contains("rpc") || text.contains("http") || text.contains("timeout") {
        "rpc"
    } else if text.contains("sql") || text.contains("postgres") || text.contains("database") {
        "db"
    } else {
        "internal"
    }
}

use anyhow::Error;
use prometheus::{
    opts, HistogramOpts, HistogramVec, IntCounterVec, IntGauge, IntGaugeVec, Registry,
};

#[derive(Clone)]
pub struct Metrics {
    pub registry: Registry,
    pub iterations_total: IntCounterVec,
    pub iteration_failures_total: IntCounterVec,
    pub iteration_duration_seconds: HistogramVec,
    pub events_indexed_total: IntCounterVec,
    pub decode_failures_total: IntCounterVec,
    pub events_valued_total: IntCounterVec,
    pub reorgs_detected_total: IntCounterVec,
    pub chain_head_block: IntGauge,
    pub task_checkpoint_block: IntGaugeVec,
    pub task_lag_blocks: IntGaugeVec,
    pub task_rpc_limit: IntGaugeVec,
    pub task_rpc_in_flight: IntGaugeVec,
    /// `livepeer_matview_refresh_total{view,result}` — count of matview
    /// refresh attempts by view name and outcome (TD-025).
    pub matview_refresh_total: IntCounterVec,
    /// `livepeer_matview_refresh_seconds{view}` — last observed refresh
    /// duration. Exposed as a gauge (last sample) since each refresh is
    /// one observation per cadence tick.
    pub matview_refresh_seconds: prometheus::GaugeVec,
    /// `livepeer_task_last_success_timestamp{task}` — unix seconds of the last
    /// successful iteration. The per-task liveness heartbeat: it advances every
    /// cadence when a loop is healthy (even when idle) and stops when the loop
    /// is wedged, erroring, or escalated. Read by `supervise` (progress-based
    /// backoff reset) and by `/health`.
    pub task_last_success_timestamp: IntGaugeVec,
    /// `livepeer_task_restarts_total{task,reason}` — times `supervise` restarted
    /// a loop; reason ∈ {error, panic}.
    pub task_restarts_total: IntCounterVec,
    /// `livepeer_task_up{task}` — 1 healthy, 0 when a loop has exceeded its
    /// restart budget (escalated). Fed into the `/health` decision.
    pub task_up: IntGaugeVec,
}

impl Metrics {
    pub fn new() -> Self {
        let registry = Registry::new();

        let iterations_total = IntCounterVec::new(
            opts!(
                "livepeer_iterations_total",
                "Successful daemon iterations by task"
            ),
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
        let events_indexed_total = IntCounterVec::new(
            opts!(
                "livepeer_events_indexed_total",
                "Raw protocol events indexed by contract"
            ),
            &["contract"],
        )
        .expect("metric construction");
        let decode_failures_total = IntCounterVec::new(
            opts!(
                "livepeer_decode_failures_total",
                "Decode failures written by contract"
            ),
            &["contract"],
        )
        .expect("metric construction");
        let events_valued_total = IntCounterVec::new(
            opts!(
                "livepeer_events_valued_total",
                "Valuation outcomes written by status"
            ),
            &["status"],
        )
        .expect("metric construction");
        let reorgs_detected_total = IntCounterVec::new(
            opts!(
                "livepeer_reorgs_detected_total",
                "Reorg divergences detected by the daemon"
            ),
            &["severity"],
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
        let task_rpc_limit = IntGaugeVec::new(
            opts!(
                "livepeer_task_rpc_limit",
                "Configured soft RPC concurrency cap by daemon task"
            ),
            &["task"],
        )
        .expect("metric construction");
        let task_rpc_in_flight = IntGaugeVec::new(
            opts!(
                "livepeer_task_rpc_in_flight",
                "Current in-flight RPC permits consumed by daemon task"
            ),
            &["task"],
        )
        .expect("metric construction");
        let matview_refresh_total = IntCounterVec::new(
            opts!(
                "livepeer_matview_refresh_total",
                "Materialized view refresh attempts by view and outcome"
            ),
            &["view", "result"],
        )
        .expect("metric construction");
        let matview_refresh_seconds = prometheus::GaugeVec::new(
            opts!(
                "livepeer_matview_refresh_seconds",
                "Wall-clock seconds for the most recent matview refresh"
            ),
            &["view"],
        )
        .expect("metric construction");
        let task_last_success_timestamp = IntGaugeVec::new(
            opts!(
                "livepeer_task_last_success_timestamp",
                "Unix seconds of the last successful iteration, by task"
            ),
            &["task"],
        )
        .expect("metric construction");
        let task_restarts_total = IntCounterVec::new(
            opts!(
                "livepeer_task_restarts_total",
                "Times a supervised loop was restarted, by task and reason"
            ),
            &["task", "reason"],
        )
        .expect("metric construction");
        let task_up = IntGaugeVec::new(
            opts!(
                "livepeer_task_up",
                "1 when a task is healthy, 0 when it has exceeded its restart budget"
            ),
            &["task"],
        )
        .expect("metric construction");

        for collector in [
            Box::new(iterations_total.clone()) as Box<dyn prometheus::core::Collector>,
            Box::new(iteration_failures_total.clone()),
            Box::new(iteration_duration_seconds.clone()),
            Box::new(events_indexed_total.clone()),
            Box::new(decode_failures_total.clone()),
            Box::new(events_valued_total.clone()),
            Box::new(reorgs_detected_total.clone()),
            Box::new(chain_head_block.clone()),
            Box::new(task_checkpoint_block.clone()),
            Box::new(task_lag_blocks.clone()),
            Box::new(task_rpc_limit.clone()),
            Box::new(task_rpc_in_flight.clone()),
            Box::new(matview_refresh_total.clone()),
            Box::new(matview_refresh_seconds.clone()),
            Box::new(task_last_success_timestamp.clone()),
            Box::new(task_restarts_total.clone()),
            Box::new(task_up.clone()),
        ] {
            registry.register(collector).expect("metric registration");
        }

        Self {
            registry,
            iterations_total,
            iteration_failures_total,
            iteration_duration_seconds,
            events_indexed_total,
            decode_failures_total,
            events_valued_total,
            reorgs_detected_total,
            chain_head_block,
            task_checkpoint_block,
            task_lag_blocks,
            task_rpc_limit,
            task_rpc_in_flight,
            matview_refresh_total,
            matview_refresh_seconds,
            task_last_success_timestamp,
            task_restarts_total,
            task_up,
        }
    }

    pub fn record_matview_refresh(&self, view: &str, duration_seconds: f64, succeeded: bool) {
        let result = if succeeded { "success" } else { "error" };
        self.matview_refresh_total
            .with_label_values(&[view, result])
            .inc();
        self.matview_refresh_seconds
            .with_label_values(&[view])
            .set(duration_seconds);
        if succeeded {
            self.beat("matview");
        }
    }

    pub fn record_success(&self, task: &'static str, duration_seconds: f64) {
        self.iterations_total.with_label_values(&[task]).inc();
        self.iteration_duration_seconds
            .with_label_values(&[task])
            .observe(duration_seconds);
        self.beat(task);
    }

    /// Stamp a task's liveness heartbeat with the current unix time.
    pub fn beat(&self, task: &str) {
        self.task_last_success_timestamp
            .with_label_values(&[task])
            .set(now_unix());
    }

    /// Current heartbeat value (unix seconds) for a task, 0 if never set.
    pub fn heartbeat(&self, task: &str) -> i64 {
        self.task_last_success_timestamp
            .with_label_values(&[task])
            .get()
    }

    pub fn record_restart(&self, task: &str, reason: &str) {
        self.task_restarts_total
            .with_label_values(&[task, reason])
            .inc();
    }

    pub fn set_task_up(&self, task: &str, up: bool) {
        self.task_up
            .with_label_values(&[task])
            .set(if up { 1 } else { 0 });
    }

    /// Current up/down state for a task. NOTE: an unset gauge reads 0, so
    /// `run_follow` initializes every task to `set_task_up(task, true)` at
    /// startup; a task is otherwise only 0 after an explicit escalation.
    pub fn task_up_value(&self, task: &str) -> i64 {
        self.task_up.with_label_values(&[task]).get()
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

/// Current unix time in seconds. Monotonic-enough for staleness math; a clock
/// jump only affects one heartbeat comparison, never correctness.
pub fn now_unix() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
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

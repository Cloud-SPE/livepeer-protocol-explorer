//! Prometheus metrics for `livepeer-staker` workers.
//!
//! TD-016 Phase E. Exposes per-phase counters / gauges for the gateway
//! backfill so operators can see whether each phase is advancing or
//! stalled. Mirrors the `livepeer_core::rpc::metrics` pattern: a global
//! OnceLock-backed `Registry` that the daemon's `/metrics` endpoint pulls
//! from via `gather()`. Standalone staker invocations also register their
//! work even though there's no HTTP surface to scrape it from there —
//! the metrics still aggregate correctly when the daemon is running.

use prometheus::{opts, proto::MetricFamily, IntCounterVec, IntGauge, IntGaugeVec, Registry};
use std::sync::OnceLock;

pub struct StakerMetrics {
    pub registry: Registry,

    /// `gateway_backfill_candidates_remaining{phase}` — depth of work the
    /// next iteration is going to look at, per phase.
    pub gateway_candidates_remaining: IntGaugeVec,

    /// `gateway_backfill_rows_written_total{phase}` — cumulative rows
    /// upserted into `gateway_balances_by_block` / `gateway_flows` /
    /// `gateway_claimants_by_block`. One counter per phase.
    pub gateway_rows_written_total: IntCounterVec,

    /// `gateway_backfill_last_processed_block{phase}` — the cursor each
    /// phase last advanced to. Lets dashboards/alerts catch a stalled
    /// phase even when the worker is still iterating.
    pub gateway_last_processed_block: IntGaugeVec,

    /// `gateway_backfill_iterations_total{phase,result}` — iteration
    /// outcomes by phase. result=`success` for normal completion,
    /// `error` for a phase failure.
    pub gateway_iterations_total: IntCounterVec,

    /// `gateway_backfill_iteration_seconds{phase}` — most recent iteration
    /// duration per phase. Surfaced as a gauge (last sample) since each
    /// phase only emits one duration per supervisor tick.
    pub gateway_iteration_seconds: IntGauge,

    // TD-020 tx-receipts backfill — same 5-family surface as gateway.
    pub tx_receipts_candidates_remaining: IntGauge,
    pub tx_receipts_rows_written_total: IntCounterVec,
    pub tx_receipts_last_processed_block: IntGauge,
    pub tx_receipts_iterations_total: IntCounterVec,
    pub tx_receipts_iteration_seconds: IntGauge,
}

fn global() -> &'static StakerMetrics {
    static METRICS: OnceLock<StakerMetrics> = OnceLock::new();
    METRICS.get_or_init(|| {
        let registry = Registry::new();

        let gateway_candidates_remaining = IntGaugeVec::new(
            opts!(
                "gateway_backfill_candidates_remaining",
                "Number of candidate rows the next gateway-backfill iteration will scan, by phase"
            ),
            &["phase"],
        )
        .expect("metric construction");

        let gateway_rows_written_total = IntCounterVec::new(
            opts!(
                "gateway_backfill_rows_written_total",
                "Cumulative rows written by the gateway-backfill worker, by phase"
            ),
            &["phase"],
        )
        .expect("metric construction");

        let gateway_last_processed_block = IntGaugeVec::new(
            opts!(
                "gateway_backfill_last_processed_block",
                "Most recently checkpointed block number, by gateway-backfill phase"
            ),
            &["phase"],
        )
        .expect("metric construction");

        let gateway_iterations_total = IntCounterVec::new(
            opts!(
                "gateway_backfill_iterations_total",
                "Gateway-backfill iterations completed, by phase and outcome"
            ),
            &["phase", "result"],
        )
        .expect("metric construction");

        let gateway_iteration_seconds = IntGauge::new(
            "gateway_backfill_iteration_seconds",
            "Wall-clock seconds for the most recent gateway-backfill iteration",
        )
        .expect("metric construction");

        let tx_receipts_candidates_remaining = IntGauge::new(
            "tx_receipts_backfill_candidates_remaining",
            "Distinct tx_hashes still missing from tx_receipts above the current checkpoint",
        )
        .expect("metric construction");

        let tx_receipts_rows_written_total = IntCounterVec::new(
            opts!(
                "tx_receipts_backfill_rows_written_total",
                "Cumulative rows written by the tx-receipts backfill worker"
            ),
            &[],
        )
        .expect("metric construction");

        let tx_receipts_last_processed_block = IntGauge::new(
            "tx_receipts_backfill_last_processed_block",
            "Most recently checkpointed block_number for the tx-receipts backfill",
        )
        .expect("metric construction");

        let tx_receipts_iterations_total = IntCounterVec::new(
            opts!(
                "tx_receipts_backfill_iterations_total",
                "Tx-receipts backfill iterations completed, by outcome"
            ),
            &["result"],
        )
        .expect("metric construction");

        let tx_receipts_iteration_seconds = IntGauge::new(
            "tx_receipts_backfill_iteration_seconds",
            "Wall-clock seconds for the most recent tx-receipts backfill iteration",
        )
        .expect("metric construction");

        for collector in [
            Box::new(gateway_candidates_remaining.clone()) as Box<dyn prometheus::core::Collector>,
            Box::new(gateway_rows_written_total.clone()),
            Box::new(gateway_last_processed_block.clone()),
            Box::new(gateway_iterations_total.clone()),
            Box::new(gateway_iteration_seconds.clone()),
            Box::new(tx_receipts_candidates_remaining.clone()),
            Box::new(tx_receipts_rows_written_total.clone()),
            Box::new(tx_receipts_last_processed_block.clone()),
            Box::new(tx_receipts_iterations_total.clone()),
            Box::new(tx_receipts_iteration_seconds.clone()),
        ] {
            registry.register(collector).expect("metric registration");
        }

        StakerMetrics {
            registry,
            gateway_candidates_remaining,
            gateway_rows_written_total,
            gateway_last_processed_block,
            gateway_iterations_total,
            gateway_iteration_seconds,
            tx_receipts_candidates_remaining,
            tx_receipts_rows_written_total,
            tx_receipts_last_processed_block,
            tx_receipts_iterations_total,
            tx_receipts_iteration_seconds,
        }
    })
}

/// One axis of the three (balance / flow / claimant) gateway sub-iterations.
pub struct GatewayAxisRecord {
    pub candidates: i64,
    pub rows_written: u64,
    pub checkpoint_block: Option<i64>,
}

pub struct GatewayIterationRecord {
    pub balance: GatewayAxisRecord,
    pub flow: GatewayAxisRecord,
    pub claimant: GatewayAxisRecord,
    pub elapsed_seconds: i64,
    pub succeeded: bool,
}

pub fn record_gateway_iteration(r: GatewayIterationRecord) {
    let GatewayIterationRecord {
        balance,
        flow,
        claimant,
        elapsed_seconds,
        succeeded,
    } = r;
    let balance_candidates = balance.candidates;
    let balance_rows_written = balance.rows_written;
    let balance_checkpoint_block = balance.checkpoint_block;
    let flow_candidates = flow.candidates;
    let flow_rows_written = flow.rows_written;
    let flow_checkpoint_block = flow.checkpoint_block;
    let claimant_candidates = claimant.candidates;
    let claimant_rows_written = claimant.rows_written;
    let claimant_checkpoint_block = claimant.checkpoint_block;
    let m = global();
    let result = if succeeded { "success" } else { "error" };

    m.gateway_candidates_remaining
        .with_label_values(&["balance"])
        .set(balance_candidates);
    m.gateway_candidates_remaining
        .with_label_values(&["flow"])
        .set(flow_candidates);
    m.gateway_candidates_remaining
        .with_label_values(&["claimant"])
        .set(claimant_candidates);

    m.gateway_rows_written_total
        .with_label_values(&["balance"])
        .inc_by(balance_rows_written);
    m.gateway_rows_written_total
        .with_label_values(&["flow"])
        .inc_by(flow_rows_written);
    m.gateway_rows_written_total
        .with_label_values(&["claimant"])
        .inc_by(claimant_rows_written);

    if let Some(b) = balance_checkpoint_block {
        m.gateway_last_processed_block
            .with_label_values(&["balance"])
            .set(b);
    }
    if let Some(b) = flow_checkpoint_block {
        m.gateway_last_processed_block
            .with_label_values(&["flow"])
            .set(b);
    }
    if let Some(b) = claimant_checkpoint_block {
        m.gateway_last_processed_block
            .with_label_values(&["claimant"])
            .set(b);
    }

    m.gateway_iterations_total
        .with_label_values(&["balance", result])
        .inc();
    m.gateway_iterations_total
        .with_label_values(&["flow", result])
        .inc();
    m.gateway_iterations_total
        .with_label_values(&["claimant", result])
        .inc();

    m.gateway_iteration_seconds.set(elapsed_seconds);
}

pub fn record_tx_receipts_iteration(
    candidates_remaining: i64,
    rows_written: u64,
    last_processed_block: Option<i64>,
    elapsed_seconds: i64,
    succeeded: bool,
) {
    let m = global();
    let result = if succeeded { "success" } else { "error" };

    m.tx_receipts_candidates_remaining.set(candidates_remaining);
    m.tx_receipts_rows_written_total
        .with_label_values(&[])
        .inc_by(rows_written);
    if let Some(b) = last_processed_block {
        m.tx_receipts_last_processed_block.set(b);
    }
    m.tx_receipts_iterations_total
        .with_label_values(&[result])
        .inc();
    m.tx_receipts_iteration_seconds.set(elapsed_seconds);
}

pub fn gather() -> Vec<MetricFamily> {
    global().registry.gather()
}

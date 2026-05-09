-- 038_create_event_metrics_daily
-- TD-018 Phase 1. Daily-bucketed event metrics rollup that backs
-- /aggregations/events for broad time-window queries. Lets the API skip
-- raw_protocol_events scans for day/week/month bucket reads.
--
-- Determinism: row contents are derived solely from raw_protocol_events
-- (canonical, finalized, is_valuable) joined to event_valuations under a
-- specific valuation_version. Replay-covered like the other rollups.
--
-- Schema notes
-- - One row per (chain_id, day_utc, contract_name, event_name, asset,
--   valuation_version). Each is_valuable event contributes a row per
--   valuation version that priced it.
-- - asset is always present because the rollup only ingests is_valuable
--   events (which by spec always carry an asset).
-- - last_event_id powers monotonic-upsert guard (TD-017 acceptance #3).

CREATE TABLE event_metrics_daily (
    chain_id              BIGINT NOT NULL,
    day_utc               DATE NOT NULL,
    contract_name         TEXT NOT NULL,
    event_name            TEXT NOT NULL,
    asset                 TEXT NOT NULL,
    valuation_version     TEXT NOT NULL,
    event_count           BIGINT NOT NULL,
    sum_amount_native     NUMERIC(38, 18),
    sum_amount_usd        NUMERIC(38, 18),
    usd_rows_priced       BIGINT NOT NULL DEFAULT 0,
    last_event_id         BIGINT NOT NULL,
    updated_at            TIMESTAMPTZ NOT NULL DEFAULT now(),
    PRIMARY KEY (chain_id, day_utc, contract_name, event_name, asset, valuation_version)
);

-- Lookup hot paths.
-- (event_name, day_utc DESC) — typical "show me X over time" queries
CREATE INDEX idx_event_metrics_daily_event_day
    ON event_metrics_daily (event_name, day_utc DESC);

-- (day_utc DESC, event_name) — broad day-bucket scans across event types
CREATE INDEX idx_event_metrics_daily_day_event
    ON event_metrics_daily (day_utc DESC, event_name);

-- (valuation_version, day_utc DESC) — version-pinned aggregation queries
CREATE INDEX idx_event_metrics_daily_version_day
    ON event_metrics_daily (valuation_version, day_utc DESC);

-- 034_create_orch_payouts_daily
-- TD-017 Phase 2 payout rollup materialization plus the supporting
-- TranscoderUpdate covering index needed for point-in-time fee share lookups.

CREATE TABLE orch_payouts_daily (
    chain_id                        BIGINT NOT NULL,
    day_utc                         DATE NOT NULL,
    orchestrator_address            TEXT NOT NULL,
    valuation_version               TEXT NOT NULL,
    broadcaster_kind                TEXT NOT NULL CHECK (broadcaster_kind IN ('ai', 'transcoding')),
    ticket_count                    BIGINT NOT NULL,
    sum_face_value_native           NUMERIC(38, 18) NOT NULL,
    sum_face_value_usd              NUMERIC(38, 18) NOT NULL,
    sum_commission_native           NUMERIC(38, 18) NOT NULL,
    sum_commission_usd              NUMERIC(38, 18) NOT NULL,
    sum_delegators_share_native     NUMERIC(38, 18) NOT NULL,
    sum_delegators_share_usd        NUMERIC(38, 18) NOT NULL,
    distinct_gateways               INT NOT NULL,
    usd_rows_priced                 BIGINT NOT NULL,
    source_max_event_id             BIGINT NOT NULL,
    updated_at                      TIMESTAMPTZ NOT NULL DEFAULT now(),

    PRIMARY KEY (
        chain_id,
        day_utc,
        orchestrator_address,
        valuation_version,
        broadcaster_kind
    )
);

CREATE INDEX idx_orch_payouts_daily_orch_day
    ON orch_payouts_daily (orchestrator_address, day_utc DESC);

CREATE INDEX idx_orch_payouts_daily_leaderboard
    ON orch_payouts_daily (day_utc DESC, sum_commission_usd DESC NULLS LAST);

CREATE INDEX idx_events_transcoder_cover_rollup
    ON raw_protocol_events (
        chain_id,
        contract_name,
        event_name,
        to_address,
        block_number DESC,
        log_index DESC
    )
    WHERE is_canonical = TRUE
      AND contract_name = 'BondingManager'
      AND event_name = 'TranscoderUpdate'
      AND to_address IS NOT NULL;

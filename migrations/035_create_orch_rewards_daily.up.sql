-- 035_create_orch_rewards_daily
-- TD-017 Phase 3 reward rollup materialization.

CREATE TABLE orch_rewards_daily (
    chain_id                     BIGINT NOT NULL,
    day_utc                      DATE NOT NULL,
    orchestrator_address         TEXT NOT NULL,
    valuation_version            TEXT NOT NULL,
    reward_event_count           BIGINT NOT NULL,
    sum_total_tokens             NUMERIC(38, 18) NOT NULL,
    sum_total_tokens_usd         NUMERIC(38, 18) NOT NULL,
    sum_orch_tokens              NUMERIC(38, 18) NOT NULL,
    sum_orch_tokens_usd          NUMERIC(38, 18) NOT NULL,
    sum_delegators_tokens        NUMERIC(38, 18) NOT NULL,
    sum_delegators_tokens_usd    NUMERIC(38, 18) NOT NULL,
    usd_rows_priced              BIGINT NOT NULL,
    source_max_event_id          BIGINT NOT NULL,
    updated_at                   TIMESTAMPTZ NOT NULL DEFAULT now(),

    PRIMARY KEY (
        chain_id,
        day_utc,
        orchestrator_address,
        valuation_version
    )
);

CREATE INDEX idx_orch_rewards_daily_orch_day
    ON orch_rewards_daily (orchestrator_address, day_utc DESC);

CREATE INDEX idx_orch_rewards_daily_leaderboard
    ON orch_rewards_daily (day_utc DESC, sum_orch_tokens_usd DESC NULLS LAST);

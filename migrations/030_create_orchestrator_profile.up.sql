-- 030_create_orchestrator_profile
-- Phase 1 TD-017 deterministic orchestrator profile materialization.

CREATE TABLE orchestrator_profile (
    chain_id                    BIGINT NOT NULL,
    address                     TEXT NOT NULL,
    total_stake                 NUMERIC(38, 18) NOT NULL,
    latest_fee_cut_percent      NUMERIC(10, 4) NOT NULL,
    latest_reward_cut_percent   NUMERIC(10, 4) NOT NULL,
    latest_fee_share_percent    NUMERIC(10, 4) NOT NULL,
    is_active                   BOOLEAN NOT NULL,
    last_lifecycle_event_at     TIMESTAMPTZ,
    as_of_block                 BIGINT NOT NULL,
    as_of_round                 BIGINT,
    last_event_id               BIGINT NOT NULL,
    service_uri                 TEXT,
    updated_at                  TIMESTAMPTZ NOT NULL DEFAULT now(),

    PRIMARY KEY (chain_id, address)
);

CREATE INDEX idx_orchestrator_profile_stake
    ON orchestrator_profile (total_stake DESC, address ASC);

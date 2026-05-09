-- 031_create_broadcaster_profile
-- Phase 1 TD-017 deterministic broadcaster/gateway profile materialization.

CREATE TABLE broadcaster_profile (
    chain_id                BIGINT NOT NULL,
    address                 TEXT NOT NULL,
    latest_deposit          NUMERIC(38, 18) NOT NULL,
    latest_reserve          NUMERIC(38, 18) NOT NULL,
    unlock_in_progress      BOOLEAN NOT NULL,
    as_of_block             BIGINT NOT NULL,
    last_event_id           BIGINT NOT NULL,
    updated_at              TIMESTAMPTZ NOT NULL DEFAULT now(),

    PRIMARY KEY (chain_id, address)
);

CREATE INDEX idx_broadcaster_profile_deposit
    ON broadcaster_profile (latest_deposit DESC, address ASC);

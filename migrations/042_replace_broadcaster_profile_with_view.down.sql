-- Reverts 042. Restores the original broadcaster_profile table shape.
-- Note: rolling back the schema does NOT restore the gateway-half of
-- profile-follow code. To revive the worker path the corresponding code
-- changes must also be reverted; otherwise the table stays empty until
-- gateway-balance-backfill re-populates it indirectly.
-- The data lives in gateway_balances_by_block and can be re-derived at
-- any time with the same DISTINCT-ON SQL the matview uses.

DROP MATERIALIZED VIEW IF EXISTS broadcaster_profile;

CREATE TABLE broadcaster_profile (
    chain_id           BIGINT          NOT NULL,
    address            TEXT            NOT NULL,
    latest_deposit     NUMERIC(38, 18) NOT NULL,
    latest_reserve     NUMERIC(38, 18) NOT NULL,
    unlock_in_progress BOOLEAN         NOT NULL,
    as_of_block        BIGINT          NOT NULL,
    last_event_id      BIGINT          NOT NULL,
    updated_at         TIMESTAMPTZ     NOT NULL DEFAULT NOW(),
    PRIMARY KEY (chain_id, address)
);

CREATE INDEX idx_broadcaster_profile_deposit
   ON broadcaster_profile (latest_deposit DESC, address);

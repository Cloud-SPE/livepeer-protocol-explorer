-- Reverts 044. Recreates the original orchestrator_profile table shape.
-- Bootstraps from the matview's current state (which lives in
-- orch_stake_by_round) so rollback doesn't lose data.

DROP MATERIALIZED VIEW IF EXISTS orchestrator_profile;

CREATE TABLE orchestrator_profile (
    chain_id                  BIGINT          NOT NULL,
    address                   TEXT            NOT NULL,
    total_stake               NUMERIC(38, 18) NOT NULL,
    latest_fee_cut_percent    NUMERIC(10, 4)  NOT NULL,
    latest_reward_cut_percent NUMERIC(10, 4)  NOT NULL,
    latest_fee_share_percent  NUMERIC(10, 4)  NOT NULL,
    is_active                 BOOLEAN         NOT NULL,
    last_lifecycle_event_at   TIMESTAMPTZ,
    as_of_block               BIGINT          NOT NULL,
    as_of_round               BIGINT,
    last_event_id             BIGINT          NOT NULL,
    service_uri               TEXT,
    updated_at                TIMESTAMPTZ     NOT NULL DEFAULT NOW(),
    PRIMARY KEY (chain_id, address)
);

CREATE INDEX idx_orchestrator_profile_stake
   ON orchestrator_profile (total_stake DESC, address);

-- Bootstrap from orch_stake_by_round latest-per-orch.
INSERT INTO orchestrator_profile (
    chain_id, address, total_stake, latest_fee_cut_percent,
    latest_reward_cut_percent, latest_fee_share_percent,
    is_active, last_lifecycle_event_at, as_of_block, as_of_round,
    last_event_id, service_uri, updated_at
)
SELECT DISTINCT ON (chain_id, address)
  chain_id,
  address,
  total_stake,
  latest_fee_cut_percent,
  latest_reward_cut_percent,
  latest_fee_share_percent,
  is_active,
  last_lifecycle_event_at,
  block_number AS as_of_block,
  round        AS as_of_round,
  triggering_event_id AS last_event_id,
  service_uri,
  NOW()
FROM orch_stake_by_round
ORDER BY chain_id, address, round DESC;

-- Reverts 045. Re-imposes NOT NULL + FK on triggering_event_id.
-- (Will fail if any rows have NULL triggering_event_id — by design.)

DROP MATERIALIZED VIEW IF EXISTS orchestrator_profile;

ALTER TABLE orch_stake_by_round
    ALTER COLUMN triggering_event_id SET NOT NULL;

ALTER TABLE orch_stake_by_round
    ADD CONSTRAINT orch_stake_by_round_triggering_event_id_fkey
        FOREIGN KEY (triggering_event_id) REFERENCES raw_protocol_events(id);

CREATE MATERIALIZED VIEW orchestrator_profile AS
SELECT DISTINCT ON (chain_id, address)
  chain_id,
  address,
  total_stake,
  latest_fee_cut_percent,
  latest_reward_cut_percent,
  latest_fee_share_percent,
  is_active,
  last_lifecycle_event_at,
  block_number       AS as_of_block,
  round              AS as_of_round,
  triggering_event_id AS last_event_id,
  service_uri,
  NOW()              AS updated_at
FROM orch_stake_by_round
ORDER BY chain_id, address, round DESC;

CREATE UNIQUE INDEX orchestrator_profile_pkey
   ON orchestrator_profile (chain_id, address);

CREATE INDEX idx_orchestrator_profile_stake
   ON orchestrator_profile (total_stake DESC, address);

-- 045_relax_orch_stake_triggering_event_fk
-- TD-026 follow-up. Mirrors `gateway_balances_by_block.triggering_event_id`'s
-- shape: nullable, no FK. The column stays as provenance metadata
-- (production workers always populate it with a real event id) but is no
-- longer enforced as NOT NULL or as a foreign key. This matches how the
-- sibling table is shaped and lets test fixtures insert synthetic rows
-- without seeding `raw_protocol_events`.

ALTER TABLE orch_stake_by_round
    DROP CONSTRAINT orch_stake_by_round_triggering_event_id_fkey;

ALTER TABLE orch_stake_by_round
    ALTER COLUMN triggering_event_id DROP NOT NULL;

-- Re-create the matview so the type of `last_event_id` (projected from
-- `triggering_event_id`) reflects the relaxed nullability. Postgres
-- doesn't let us alter the underlying column type via the matview, so
-- a drop+create is the simplest path.
DROP MATERIALIZED VIEW orchestrator_profile;

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

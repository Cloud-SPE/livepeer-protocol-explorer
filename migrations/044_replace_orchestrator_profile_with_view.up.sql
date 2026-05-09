-- 044_replace_orchestrator_profile_with_view
-- TD-026 Phase C. Drops the `orchestrator_profile` table and replaces it
-- with a materialized view over `orch_stake_by_round` (introduced in
-- migration 043).
--
-- Sequencing matters: the matview must have rows immediately after creation
-- so API consumers don't see an empty `orchestrator_profile`. We bootstrap
-- by porting the existing 1,936 `orchestrator_profile` rows into
-- `orch_stake_by_round` BEFORE the swap. block_timestamp + block_hash for
-- those rows come from joining raw_protocol_events on orchestrator_profile
-- .last_event_id (every existing profile row points at a real event).
--
-- After this migration:
--   - orch_stake_by_round contains the previous "current" snapshot per orch,
--     plus any per-round snapshots the worker has written since 043.
--   - orchestrator_profile is a matview projecting DISTINCT ON (address)
--     ... ORDER BY round DESC. Refreshed by the daemon's matview-refresh
--     hook (TD-025) every 30 s.

-- 1. Bootstrap: port existing rows. ON CONFLICT DO NOTHING because the
--    worker may have already written newer rounds for the same orch.
INSERT INTO orch_stake_by_round (
    chain_id, address, round, block_number, block_timestamp, block_hash,
    total_stake, service_uri, latest_fee_cut_percent, latest_reward_cut_percent,
    latest_fee_share_percent, is_active, last_lifecycle_event_at,
    triggering_event_id
)
SELECT op.chain_id,
       op.address,
       COALESCE(op.as_of_round, 0) AS round,
       op.as_of_block,
       e.block_timestamp,
       e.block_hash,
       op.total_stake,
       op.service_uri,
       op.latest_fee_cut_percent,
       op.latest_reward_cut_percent,
       op.latest_fee_share_percent,
       op.is_active,
       op.last_lifecycle_event_at,
       op.last_event_id
  FROM orchestrator_profile op
  JOIN raw_protocol_events e ON e.id = op.last_event_id
ON CONFLICT (chain_id, address, round) DO NOTHING;

-- 2. Drop the table; recreate as a matview.
DROP TABLE orchestrator_profile;

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

-- Required for REFRESH MATERIALIZED VIEW CONCURRENTLY.
CREATE UNIQUE INDEX orchestrator_profile_pkey
   ON orchestrator_profile (chain_id, address);

-- Mirrors the prior table's secondary index for leaderboard sort by stake.
CREATE INDEX idx_orchestrator_profile_stake
   ON orchestrator_profile (total_stake DESC, address);

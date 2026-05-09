-- 042_replace_broadcaster_profile_with_view
-- TD-025. Replaces the `broadcaster_profile` table (populated by the gateway
-- half of `livepeer-staker profile-follow` via per-event RPC walk) with a
-- materialized view over `gateway_balances_by_block` (already populated by
-- `livepeer-staker gateway-backfill` to chain head, with the same source
-- RPCs but a strictly larger column set).
--
-- See docs/exec-plans/active/td-025-broadcaster-profile-derived-view.md for
-- the full investigation. The walk it eliminates was reading 600K+ events
-- and ~1.12M cached eth_calls to materialize 13 rows that already existed.
--
-- The matview adds two fields previously discarded:
--   - reserve_claimed_in_current_round
--   - withdraw_round
--
-- Refresh strategy: a daemon-hosted task runs
--   REFRESH MATERIALIZED VIEW CONCURRENTLY broadcaster_profile
-- every ~30s. The unique index below is required for CONCURRENTLY.

DROP TABLE IF EXISTS broadcaster_profile;

CREATE MATERIALIZED VIEW broadcaster_profile AS
SELECT DISTINCT ON (chain_id, gateway_address)
  chain_id,
  gateway_address                      AS address,
  deposit                              AS latest_deposit,
  reserve_funds_remaining              AS latest_reserve,
  reserve_claimed_in_current_round,
  withdraw_round,
  unlock_in_progress,
  block_number                         AS as_of_block,
  block_timestamp                      AS as_of_timestamp,
  triggering_event_id                  AS last_event_id,
  NOW()                                AS updated_at
FROM gateway_balances_by_block
ORDER BY chain_id, gateway_address, block_number DESC;

-- Required for REFRESH MATERIALIZED VIEW CONCURRENTLY.
CREATE UNIQUE INDEX broadcaster_profile_pkey
   ON broadcaster_profile (chain_id, address);

-- Mirrors the prior table's secondary index for leaderboard sort by deposit.
CREATE INDEX idx_broadcaster_profile_deposit
   ON broadcaster_profile (latest_deposit DESC, address);

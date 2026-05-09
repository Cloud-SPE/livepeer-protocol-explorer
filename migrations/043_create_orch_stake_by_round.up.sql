-- 043_create_orch_stake_by_round
-- TD-026. Per-round historical orchestrator snapshots.
--
-- Today the orch fanout in `livepeer-staker profile-follow` walks every
-- `NewRound` × 1,936 orchs, reads `transcoderTotalStake` + `getServiceURI`
-- at each, and upserts into `orchestrator_profile` with `last_event_id`
-- monotonic guard — only the latest snapshot survives. Every prior
-- snapshot is read and discarded. Unlike gateway snapshots (which
-- persist into `gateway_balances_by_block`), orch total-stake history
-- has no dedicated home.
--
-- This table preserves every (orch, round) snapshot the worker produces.
-- TD-026 Phase C converts `orchestrator_profile` into a materialized
-- view over this table. Future stake-history features (per-orch stake
-- charts, leaderboard time series, decline detection) read from here.

CREATE TABLE orch_stake_by_round (
    chain_id                  BIGINT       NOT NULL,
    address                   TEXT         NOT NULL,
    round                     BIGINT       NOT NULL,
    block_number              BIGINT       NOT NULL,
    block_timestamp           TIMESTAMPTZ  NOT NULL,
    block_hash                TEXT         NOT NULL,

    -- From the on-chain reads at the NewRound block
    total_stake               NUMERIC(38,18) NOT NULL,
    service_uri               TEXT,

    -- Joined from event tables at this block (no extra RPC reads)
    latest_fee_cut_percent    NUMERIC(10,4) NOT NULL,
    latest_reward_cut_percent NUMERIC(10,4) NOT NULL,
    latest_fee_share_percent  NUMERIC(10,4) NOT NULL,
    is_active                 BOOLEAN       NOT NULL,
    last_lifecycle_event_at   TIMESTAMPTZ,

    -- Provenance
    triggering_event_id       BIGINT NOT NULL REFERENCES raw_protocol_events(id),
    raw_call                  JSONB,
    created_at                TIMESTAMPTZ NOT NULL DEFAULT NOW(),

    PRIMARY KEY (chain_id, address, round)
);

-- Lookup by orch (latest round per orch via DISTINCT ON)
CREATE INDEX idx_orch_stake_address_round
   ON orch_stake_by_round (address, round DESC);

-- Per-round leaderboard / time-series queries
CREATE INDEX idx_orch_stake_round
   ON orch_stake_by_round (chain_id, round);

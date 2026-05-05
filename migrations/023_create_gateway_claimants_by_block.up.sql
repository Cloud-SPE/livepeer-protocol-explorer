-- 023_create_gateway_claimants_by_block
-- Phase 3 TicketBroker claimant-level reserve state snapshots.

CREATE TABLE gateway_claimants_by_block (
    chain_id            BIGINT NOT NULL,
    gateway_address     TEXT NOT NULL,
    claimant_address    TEXT NOT NULL,
    block_number        BIGINT NOT NULL,
    block_timestamp     TIMESTAMPTZ NOT NULL,
    block_hash          TEXT NOT NULL,

    claimable_reserve   NUMERIC(38, 18) NOT NULL,
    claimed_reserve     NUMERIC(38, 18) NOT NULL,

    source              TEXT NOT NULL,   -- 'rpc_reconciled' | 'event_derived' | 'both'
    raw_call            JSONB,
    triggering_event_id BIGINT REFERENCES raw_protocol_events(id),
    created_at          TIMESTAMPTZ NOT NULL DEFAULT now(),

    PRIMARY KEY (chain_id, gateway_address, claimant_address, block_number)
);

CREATE INDEX idx_gateway_claimants_recent
    ON gateway_claimants_by_block (gateway_address, claimant_address, block_number DESC);

CREATE INDEX idx_gateway_claimants_gateway
    ON gateway_claimants_by_block (gateway_address, block_number DESC);

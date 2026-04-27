-- 009_create_stake_balances_by_block
-- Per-event-block stake snapshots. Scope 2: event-triggered, not full fan-out. SPEC §11.10.

CREATE TABLE stake_balances_by_block (
    chain_id              BIGINT NOT NULL,
    delegator_address     TEXT NOT NULL,
    delegate_address      TEXT NOT NULL,
    block_number          BIGINT NOT NULL,
    block_timestamp       TIMESTAMPTZ NOT NULL,
    block_hash            TEXT NOT NULL,

    bonded_principal      NUMERIC(38, 18) NOT NULL,
    pending_stake         NUMERIC(38, 18),
    pending_fees          NUMERIC(38, 18),
    pending_round         BIGINT,

    source                TEXT NOT NULL,    -- 'flow_derived' | 'pending_call' | 'both'
    raw_call              JSONB,

    triggering_event_id   BIGINT REFERENCES raw_protocol_events(id),
    created_at            TIMESTAMPTZ NOT NULL DEFAULT now(),

    PRIMARY KEY (chain_id, delegator_address, block_number)
);

CREATE INDEX idx_stake_delegator_recent ON stake_balances_by_block (delegator_address, block_number DESC);
CREATE INDEX idx_stake_delegate         ON stake_balances_by_block (delegate_address, block_number DESC);

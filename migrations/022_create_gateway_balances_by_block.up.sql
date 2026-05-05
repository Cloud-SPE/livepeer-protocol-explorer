-- 022_create_gateway_balances_by_block
-- Phase 2 TicketBroker gateway state materialization. Historical sender balance
-- snapshots keyed by gateway address and block. See gateway-ticketbroker-data-model.md.

CREATE TABLE gateway_balances_by_block (
    chain_id                           BIGINT NOT NULL,
    gateway_address                    TEXT NOT NULL,
    block_number                       BIGINT NOT NULL,
    block_timestamp                    TIMESTAMPTZ NOT NULL,
    block_hash                         TEXT NOT NULL,

    deposit                            NUMERIC(38, 18) NOT NULL,
    reserve_funds_remaining            NUMERIC(38, 18) NOT NULL,
    reserve_claimed_in_current_round   NUMERIC(38, 18) NOT NULL,
    withdraw_round                     BIGINT NOT NULL,
    unlock_in_progress                 BOOLEAN NOT NULL,

    source                             TEXT NOT NULL,   -- 'rpc_reconciled' | 'event_derived' | 'both'
    raw_call                           JSONB,
    triggering_event_id                BIGINT REFERENCES raw_protocol_events(id),
    created_at                         TIMESTAMPTZ NOT NULL DEFAULT now(),

    PRIMARY KEY (chain_id, gateway_address, block_number)
);

CREATE INDEX idx_gateway_balances_recent
    ON gateway_balances_by_block (gateway_address, block_number DESC);

CREATE INDEX idx_gateway_balances_event
    ON gateway_balances_by_block (triggering_event_id)
    WHERE triggering_event_id IS NOT NULL;

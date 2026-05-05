-- 024_create_gateway_flows
-- Phase 3 materialized gateway funding / payout ledger for query-heavy analytics.

CREATE TABLE gateway_flows (
    id                  BIGSERIAL PRIMARY KEY,
    chain_id            BIGINT NOT NULL,
    event_id            BIGINT NOT NULL REFERENCES raw_protocol_events(id),
    gateway_address     TEXT NOT NULL,
    claimant_address    TEXT,
    counterparty_address TEXT,
    block_number        BIGINT NOT NULL,
    block_timestamp     TIMESTAMPTZ NOT NULL,
    tx_hash             TEXT NOT NULL,
    log_index           INTEGER NOT NULL,
    event_name          TEXT NOT NULL,
    flow_kind           TEXT NOT NULL,
    asset               TEXT,
    amount_native       NUMERIC(38, 18),
    amount_usd          NUMERIC(38, 18),
    valuation_version   TEXT,
    created_at          TIMESTAMPTZ NOT NULL DEFAULT now(),

    UNIQUE (event_id, flow_kind)
);

CREATE INDEX idx_gateway_flows_gateway_recent
    ON gateway_flows (gateway_address, block_number DESC, log_index DESC);

CREATE INDEX idx_gateway_flows_claimant_recent
    ON gateway_flows (claimant_address, block_number DESC, log_index DESC)
    WHERE claimant_address IS NOT NULL;

CREATE INDEX idx_gateway_flows_kind_recent
    ON gateway_flows (flow_kind, block_number DESC, log_index DESC);

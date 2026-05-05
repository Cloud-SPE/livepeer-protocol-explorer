-- 025_gateway_flow_analytics_indexes
-- Support recipient leaderboards and rolling gateway analytics over the
-- materialized gateway_flows ledger.

CREATE INDEX idx_gateway_flows_gateway_kind_time
    ON gateway_flows (gateway_address, flow_kind, block_timestamp DESC);

CREATE INDEX idx_gateway_flows_gateway_counterparty_time
    ON gateway_flows (gateway_address, counterparty_address, block_timestamp DESC)
    WHERE counterparty_address IS NOT NULL;

CREATE INDEX idx_gateway_flows_gateway_claimant_time
    ON gateway_flows (gateway_address, claimant_address, block_timestamp DESC)
    WHERE claimant_address IS NOT NULL;

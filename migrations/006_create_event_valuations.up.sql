-- 006_create_event_valuations
-- Immutable per (event_id, valuation_version, asset). SPEC §11.7, §6.8.
-- Multi-asset events (e.g. EarningsClaimed) produce multiple rows per version.
--
-- ROWS ARE NEVER UPDATED. New pricing logic = new valuation_version. Conflicts with
-- differing values fire a CRITICAL determinism alert (SPEC §10.5).

CREATE TABLE event_valuations (
    event_id              BIGINT NOT NULL REFERENCES raw_protocol_events(id),
    valuation_version     TEXT NOT NULL,
    asset                 TEXT NOT NULL,
    pricing_method        TEXT NOT NULL,

    chain_id              BIGINT NOT NULL,
    block_number          BIGINT NOT NULL,

    amount_native         NUMERIC(38, 18) NOT NULL,
    native_usd_price      NUMERIC(38, 18) NOT NULL,
    amount_usd            NUMERIC(38, 18) NOT NULL,

    pricing_chain         JSONB NOT NULL,

    status                TEXT NOT NULL,    -- 'priced' | 'priced_with_warning'
    source                TEXT NOT NULL,    -- 'trusted_historical_seed_v1' | 'uniswap_v3_dual_rpc' | 'chainlink_dual_rpc'

    created_at            TIMESTAMPTZ NOT NULL DEFAULT now(),

    PRIMARY KEY (event_id, valuation_version, asset),
    CHECK (status IN ('priced', 'priced_with_warning'))
);

CREATE INDEX idx_valuations_version_block ON event_valuations (valuation_version, block_number);
CREATE INDEX idx_valuations_asset_block   ON event_valuations (asset, block_number);

-- 005_create_token_prices_by_block
-- On-chain price reads (cache for valuator). SPEC §11.6.

CREATE TABLE token_prices_by_block (
    chain_id          BIGINT NOT NULL,
    asset             TEXT NOT NULL,
    quote             TEXT NOT NULL,

    block_number      BIGINT NOT NULL,
    block_hash        TEXT NOT NULL,
    block_timestamp   TIMESTAMPTZ NOT NULL,

    price             NUMERIC(38, 18) NOT NULL,

    source            TEXT NOT NULL,           -- 'uniswap_v3_twap_30min' | 'uniswap_v3_spot' | 'chainlink' | 'trusted_historical_seed_v1'
    pool_address      TEXT,
    oracle_address    TEXT,

    raw               JSONB,
    created_at        TIMESTAMPTZ NOT NULL DEFAULT now(),

    PRIMARY KEY (chain_id, asset, quote, block_number, source)
);

CREATE INDEX idx_token_prices_lookup ON token_prices_by_block (chain_id, asset, quote, block_number DESC);

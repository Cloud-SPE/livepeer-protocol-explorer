-- 004_create_seeded_event_prices
-- Trusted historical valuations imported from SQLite. SPEC §11.5, §8.
--
-- Per Q-OD-2 (verified 2026-04-27): payout (297K) and reward (158K) transaction_id are
-- both unique within their respective tables — no log_index needed for v1. The PK uses
-- COALESCE(log_index, -1) to permit NULL for seeded rows while keeping the constraint sane.

CREATE TABLE seeded_event_prices (
    chain_id             BIGINT NOT NULL,
    tx_hash              TEXT NOT NULL,
    -- log_index is -1 for seeded rows (no per-log resolution from payout/reward; Q-OD-2
    -- confirmed transaction_id is unique within those tables). For optional events.payload
    -- cross-check imports it is the concrete log_index.
    log_index            INT NOT NULL DEFAULT -1,
    event_type_hint      TEXT NOT NULL,        -- 'reward' | 'payout'
    asset                TEXT NOT NULL,

    amount_native        NUMERIC(38, 18) NOT NULL,
    amount_usd           NUMERIC(38, 18) NOT NULL,
    asset_usd_price      NUMERIC(38, 18) NOT NULL,

    source               TEXT NOT NULL DEFAULT 'trusted_historical_seed_v1',
    raw                  JSONB,
    imported_at          TIMESTAMPTZ NOT NULL DEFAULT now(),

    PRIMARY KEY (chain_id, tx_hash, log_index, asset)
);

CREATE INDEX idx_seeded_lookup ON seeded_event_prices (chain_id, tx_hash, asset);

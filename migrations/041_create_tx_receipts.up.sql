-- 041_create_tx_receipts
-- TD-020 Phase A. First-class persistence for transaction receipts so the
-- /reports/*.csv routes (and any future fee/gas analytics) can serve from
-- Postgres instead of fan-out RPC against eth_getTransactionReceipt.
--
-- Determinism: every row is a deterministic projection of a cached
-- eth_getTransactionReceipt response (rpc_call_cache). The replay contract
-- already covers the source bytes; this table is a typed view of them.
-- See docs/DETERMINISM.md.
--
-- Schema notes
-- - One row per (chain_id, tx_hash). The pipeline only writes finalized
--   rows, so reorg-time mutation is a non-issue (finalized -> reorged is
--   structurally impossible per SPEC §9.1).
-- - tx_fee_wei = gas_used * effective_gas_price, precomputed so reports
--   skip per-row arithmetic at read time.
-- - tx_fee_eth is the same value scaled to ETH (NUMERIC(38,18)) for direct
--   inclusion in CSV exports without conversion at the API layer.
-- - status is stored for future filtering; reverted txs (status=0) still
--   carry valid gas charges so existing CSV semantics are preserved.

CREATE TABLE tx_receipts (
    chain_id              BIGINT       NOT NULL,
    tx_hash               TEXT         NOT NULL,

    -- Block context (denormalized so reports can answer time-window queries
    -- without joining raw_protocol_events).
    block_number          BIGINT       NOT NULL,
    block_timestamp       TIMESTAMPTZ  NOT NULL,

    -- Receipt fields
    gas_used              NUMERIC(78,0)  NOT NULL,
    effective_gas_price   NUMERIC(78,0)  NOT NULL,
    tx_fee_wei            NUMERIC(78,0)  NOT NULL,
    tx_fee_eth            NUMERIC(38,18) NOT NULL,
    status                SMALLINT       NOT NULL,

    -- Sender / recipient
    from_address          TEXT         NOT NULL,
    to_address            TEXT,                         -- NULL for contract creations

    created_at            TIMESTAMPTZ  NOT NULL DEFAULT NOW(),

    PRIMARY KEY (chain_id, tx_hash)
);

-- Supports any future "give me receipts for this block range" shape; the
-- reports routes themselves use the PK lookup via tx_hash = ANY(...).
CREATE INDEX idx_tx_receipts_chain_block ON tx_receipts (chain_id, block_number);

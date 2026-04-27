-- 001_create_indexer_checkpoints
-- Per-service progress tracking. SPEC §11.2.
--
-- Names used: 'main', 'reorg_watcher', 'finality_watcher', 'valuator_v1', 'staker'.
-- (SPEC v1.2 dropped the per-event-type seed cursors — block_cursors is no longer consumed.)

CREATE TABLE indexer_checkpoints (
    name                       TEXT PRIMARY KEY,
    chain_id                   BIGINT NOT NULL,
    last_processed_block       BIGINT NOT NULL,
    last_processed_block_hash  TEXT,
    updated_at                 TIMESTAMPTZ NOT NULL DEFAULT now()
);

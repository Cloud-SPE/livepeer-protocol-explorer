-- 014_create_reorg_mutations
-- Audit trail for the limited mutation case (block_number/block_hash update on reorg).
-- SPEC §11.15, §9.3.
--
-- This is the ONLY mutation ever applied to raw_protocol_events. Every such mutation
-- is recorded here with old + new block info and the reorg event that caused it.

CREATE TABLE reorg_mutations (
    id                  BIGSERIAL PRIMARY KEY,
    reorg_event_id      BIGINT NOT NULL REFERENCES reorg_events(id),
    raw_event_id        BIGINT NOT NULL REFERENCES raw_protocol_events(id),
    old_block_number    BIGINT NOT NULL,
    old_block_hash      TEXT NOT NULL,
    new_block_number    BIGINT NOT NULL,
    new_block_hash      TEXT NOT NULL,
    mutated_at          TIMESTAMPTZ NOT NULL DEFAULT now()
);

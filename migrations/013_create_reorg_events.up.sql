-- 013_create_reorg_events
-- Audit log of detected chain reorganizations. SPEC §11.14, §9.2.

CREATE TABLE reorg_events (
    id                      BIGSERIAL PRIMARY KEY,
    chain_id                BIGINT NOT NULL,
    detected_at             TIMESTAMPTZ NOT NULL DEFAULT now(),
    divergence_block        BIGINT NOT NULL,
    depth                   INT NOT NULL,
    old_block_hashes        TEXT[] NOT NULL,
    new_block_hashes        TEXT[] NOT NULL,
    affected_event_count    INT NOT NULL,
    notes                   TEXT
);

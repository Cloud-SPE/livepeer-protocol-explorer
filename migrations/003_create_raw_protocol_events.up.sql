-- 003_create_raw_protocol_events
-- Central event table. SPEC §11.4.
-- Event-level data is denormalized for query speed; full payload preserved in raw_event JSONB.
--
-- Single permitted mutation: reorg-induced (block_number, block_hash) update,
-- audited via reorg_mutations (014). All other fields are write-once. (SPEC §2.1, §9.3)

CREATE TABLE raw_protocol_events (
    id                   BIGSERIAL PRIMARY KEY,

    -- Canonical identity
    chain_id             BIGINT NOT NULL,
    tx_hash              TEXT NOT NULL,
    log_index            INT NOT NULL,

    -- Block context
    block_number         BIGINT NOT NULL,
    block_hash           TEXT NOT NULL,
    block_timestamp      TIMESTAMPTZ NOT NULL,

    -- Contract / event identity
    contract_address     TEXT NOT NULL,
    contract_name        TEXT NOT NULL,
    event_name           TEXT NOT NULL,
    event_signature      TEXT NOT NULL,        -- topic0

    -- Semantics
    asset                TEXT,                 -- 'LPT' | 'ETH' | NULL for non-monetary
    amount_raw           NUMERIC(78, 0),
    amount_normalized    NUMERIC(38, 18),
    is_valuable          BOOLEAN NOT NULL,

    -- Common decoded fields
    from_address         TEXT,
    to_address           TEXT,

    -- Lifecycle
    finality             TEXT NOT NULL DEFAULT 'tentative',
    is_canonical         BOOLEAN NOT NULL DEFAULT TRUE,
    finalized_at         TIMESTAMPTZ,
    l1_batch_tx_hash     TEXT,

    -- Full payload
    raw_event            JSONB NOT NULL,

    -- Provenance
    abi_hash_used        TEXT NOT NULL,

    created_at           TIMESTAMPTZ NOT NULL DEFAULT now(),

    UNIQUE (chain_id, tx_hash, log_index),
    CHECK (finality IN ('tentative', 'l1_posted', 'finalized'))
);

CREATE INDEX idx_events_block            ON raw_protocol_events (chain_id, block_number);
CREATE INDEX idx_events_contract_event   ON raw_protocol_events (contract_name, event_name, block_number);
CREATE INDEX idx_events_valuable_finality ON raw_protocol_events (is_valuable, finality, is_canonical)
    WHERE is_valuable = TRUE;
CREATE INDEX idx_events_from_address     ON raw_protocol_events (from_address) WHERE from_address IS NOT NULL;
CREATE INDEX idx_events_to_address       ON raw_protocol_events (to_address)   WHERE to_address   IS NOT NULL;
CREATE INDEX idx_events_block_timestamp  ON raw_protocol_events (block_timestamp);

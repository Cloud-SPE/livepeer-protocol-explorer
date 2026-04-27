-- 008_create_decode_failures
-- Dead-letter table for non-critical decode failures. SPEC §11.9, §10.2.2.
--
-- Critical events (allowlist, §6.2) trigger a strict halt instead of dead-lettering.
-- Operators recover via `livepeer-indexer recover-decode-failures` after updating the
-- ABI registry (SPEC §10.2.2).

CREATE TABLE decode_failures (
    id                    BIGSERIAL PRIMARY KEY,
    chain_id              BIGINT NOT NULL,
    block_number          BIGINT NOT NULL,
    block_hash            TEXT NOT NULL,
    tx_hash               TEXT NOT NULL,
    log_index             INT NOT NULL,
    contract_address      TEXT NOT NULL,
    topics                TEXT[] NOT NULL,
    data                  BYTEA NOT NULL,
    attempted_abi_hash    TEXT NOT NULL,
    error_message         TEXT NOT NULL,
    resolved_at           TIMESTAMPTZ,
    resolved_event_id     BIGINT REFERENCES raw_protocol_events(id),
    created_at            TIMESTAMPTZ NOT NULL DEFAULT now(),

    UNIQUE (chain_id, tx_hash, log_index)
);

CREATE INDEX idx_decode_failures_unresolved ON decode_failures (created_at) WHERE resolved_at IS NULL;

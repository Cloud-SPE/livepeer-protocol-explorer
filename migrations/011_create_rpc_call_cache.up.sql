-- 011_create_rpc_call_cache
-- Determinism backbone. SPEC §11.12, §13.5.
--
-- Every archive RPC call is cached with raw response bytes here. Drop the rest of the
-- DB and replay from this cache + the seed → byte-identical output (CI gate, §12.4).

CREATE TABLE rpc_call_cache (
    call_hash                  TEXT PRIMARY KEY,    -- sha256(method || canonical_params || block)
    method                     TEXT NOT NULL,
    params                     JSONB NOT NULL,
    block_number               BIGINT,
    response_bytes             BYTEA NOT NULL,
    response_hash              TEXT NOT NULL,
    provider                   TEXT NOT NULL,
    cross_check_provider       TEXT,
    cross_check_response_hash  TEXT,
    created_at                 TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE INDEX idx_rpc_cache_method_block ON rpc_call_cache (method, block_number);

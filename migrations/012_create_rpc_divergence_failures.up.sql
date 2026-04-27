-- 012_create_rpc_divergence_failures
-- Captured when two RPC providers returned different responses for the same query. SPEC §11.13, §7.6.
--
-- Cross-check operates at the raw response level — bytes compared, not derived values. Any
-- mismatch is `failed_rpc_divergence`, NEVER auto-retried, always surfaced for human review.

CREATE TABLE rpc_divergence_failures (
    id                  BIGSERIAL PRIMARY KEY,
    method              TEXT NOT NULL,
    params              JSONB NOT NULL,
    block_number        BIGINT,
    provider_a          TEXT NOT NULL,
    response_a_bytes    BYTEA NOT NULL,
    response_a_hash     TEXT NOT NULL,
    provider_b          TEXT NOT NULL,
    response_b_bytes    BYTEA NOT NULL,
    response_b_hash     TEXT NOT NULL,
    detected_at         TIMESTAMPTZ NOT NULL DEFAULT now(),
    resolved_at         TIMESTAMPTZ,
    resolution_notes    TEXT
);

CREATE INDEX idx_divergence_unresolved ON rpc_divergence_failures (detected_at) WHERE resolved_at IS NULL;

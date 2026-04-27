-- 007_create_valuation_attempts
-- Audit trail of every pricing attempt. SPEC §11.8, §10.3.

CREATE TABLE valuation_attempts (
    id                  BIGSERIAL PRIMARY KEY,
    event_id            BIGINT NOT NULL REFERENCES raw_protocol_events(id),
    valuation_version   TEXT NOT NULL,
    asset               TEXT NOT NULL,
    attempt_number      INT NOT NULL,

    attempted_at        TIMESTAMPTZ NOT NULL DEFAULT now(),
    result_status       TEXT NOT NULL,
    error_detail        JSONB,
    next_retry_at       TIMESTAMPTZ,

    UNIQUE (event_id, valuation_version, asset, attempt_number)
);

CREATE INDEX idx_attempts_retry ON valuation_attempts (next_retry_at) WHERE next_retry_at IS NOT NULL;
CREATE INDEX idx_attempts_event ON valuation_attempts (event_id, valuation_version, asset, attempt_number DESC);

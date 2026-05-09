-- 036_create_tickets_daily
-- TD-017 Phase 3 ticket timeseries materialization.

CREATE TABLE tickets_daily (
    chain_id                  BIGINT NOT NULL,
    day_utc                   DATE NOT NULL,
    broadcaster_kind          TEXT NOT NULL CHECK (broadcaster_kind IN ('ai', 'transcoding')),
    ticket_count              BIGINT NOT NULL,
    distinct_orchestrators    INT NOT NULL,
    distinct_gateways         INT NOT NULL,
    source_max_event_id       BIGINT NOT NULL,
    updated_at                TIMESTAMPTZ NOT NULL DEFAULT now(),

    PRIMARY KEY (
        chain_id,
        day_utc,
        broadcaster_kind
    )
);

CREATE INDEX idx_tickets_daily_day
    ON tickets_daily (day_utc DESC, broadcaster_kind);

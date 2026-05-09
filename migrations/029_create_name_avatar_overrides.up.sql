-- 029_create_name_avatar_overrides
-- Phase 0 TD-017 operator-curated display-name / avatar overrides kept outside
-- the deterministic replay boundary.

CREATE TABLE name_avatar_overrides (
    chain_id                    BIGINT NOT NULL,
    address                     TEXT NOT NULL,
    display_name                TEXT,
    avatar_url                  TEXT,
    notes                       TEXT,
    updated_at                  TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_by                  TEXT,
    ens_name_at_override_time   TEXT,

    PRIMARY KEY (chain_id, address)
);

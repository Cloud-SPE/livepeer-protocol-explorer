-- 032_create_orchestrator_ens
-- Phase 1 TD-017 external ENS projection for orchestrator profiles.

CREATE TABLE orchestrator_ens (
    chain_id                BIGINT NOT NULL,
    address                 TEXT NOT NULL,
    ens_name                TEXT,
    ens_avatar_url          TEXT,
    ens_last_resolved_at    TIMESTAMPTZ,

    PRIMARY KEY (chain_id, address)
);

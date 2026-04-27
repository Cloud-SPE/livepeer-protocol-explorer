-- 010_create_delegator_registry
-- Fast lookup of all known delegators, derived from Bond events. SPEC §11.11.

CREATE TABLE delegator_registry (
    chain_id              BIGINT NOT NULL,
    delegator_address     TEXT NOT NULL,
    first_bond_block      BIGINT NOT NULL,
    first_bond_event_id   BIGINT NOT NULL REFERENCES raw_protocol_events(id),
    last_seen_block       BIGINT NOT NULL,
    last_seen_event_id    BIGINT NOT NULL REFERENCES raw_protocol_events(id),
    is_active             BOOLEAN NOT NULL DEFAULT TRUE,

    PRIMARY KEY (chain_id, delegator_address)
);

CREATE INDEX idx_delegator_active ON delegator_registry (is_active) WHERE is_active = TRUE;

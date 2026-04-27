-- 002_create_contract_abi_registry
-- Per-block-range ABI mapping with strict-decode flag. SPEC §11.3, §5.4, §5.5.
--
-- The registry is the source of truth for (proxy, block_range) -> abi.
-- Each ABI JSON in abi/ has its sha256 recorded here and verified at boot.

CREATE TABLE contract_abi_registry (
    contract_name   TEXT NOT NULL,
    proxy_address   TEXT NOT NULL,
    target_address  TEXT NOT NULL,
    from_block      BIGINT NOT NULL,
    to_block        BIGINT,
    abi_path        TEXT NOT NULL,
    abi_hash        TEXT NOT NULL,
    strict_decode   BOOLEAN NOT NULL DEFAULT FALSE,
    created_at      TIMESTAMPTZ NOT NULL DEFAULT now(),
    PRIMARY KEY (contract_name, from_block)
);

CREATE INDEX idx_abi_registry_proxy ON contract_abi_registry (proxy_address, from_block);

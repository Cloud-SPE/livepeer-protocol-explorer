-- 047_valuator_finalized_at_cursor
-- Incremental valuator candidate detection. The valuator previously re-scanned
-- the entire finalized history every cycle to find pricing candidates; this
-- adds a per-pass high-water mark keyed on `finalized_at` so each cycle scans
-- only the recently-finalized tail.
--
-- Why finalized_at (not block_number/id): the indexer can backfill OLD block
-- ranges at any time (tentative, out-of-order BIGSERIAL ids); those rows
-- finalize with SMALL block numbers but a RECENT finalized_at (stamped once by
-- the finality-watcher). A block/id watermark would skip them forever; a
-- finalized_at watermark cannot. The candidate anti-join predicates remain the
-- correctness backstop — this cursor only bounds the scan.

CREATE TABLE valuator_cursors (
    -- 'valuator_<valuation_version>_<PASS>' where PASS ∈ {ETH, LPT, MULTI, SEED}.
    pass_key      TEXT PRIMARY KEY,
    -- Resolved-through finalized_at for this pass (low-water mark of unresolved
    -- work). SEED uses this column to store its finality-checkpoint marker.
    watermark     TIMESTAMPTZ NOT NULL,
    -- SEED change-detector: highest seeded_event_prices.id folded in. NULL for
    -- the on-chain passes.
    seed_max_id   BIGINT,
    updated_at    TIMESTAMPTZ NOT NULL DEFAULT now()
);

-- Tail-scan support for the ETH/LPT candidate queries (asset-scoped), keyed on
-- finalized_at, mirroring the migration-016 partial-index pattern.
CREATE INDEX idx_events_valuable_finalized_at
    ON raw_protocol_events (chain_id, asset, finalized_at, block_number, log_index)
    WHERE is_valuable = TRUE
      AND is_canonical = TRUE
      AND finality = 'finalized'
      AND asset IS NOT NULL;

-- Tail-scan support for the multi-asset (EarningsClaimed, asset IS NULL) pass.
CREATE INDEX idx_events_earnings_claimed_finalized_at
    ON raw_protocol_events (chain_id, finalized_at, block_number, log_index)
    WHERE is_valuable = TRUE
      AND is_canonical = TRUE
      AND finality = 'finalized'
      AND event_name = 'EarningsClaimed'
      AND asset IS NULL;

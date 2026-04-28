-- 015_finality_watcher_partial_index
--
-- Targets the two UPDATEs in livepeer-finality-watcher that step rows through
-- finality = 'tentative' → 'l1_posted' → 'finalized'. The general
-- idx_events_block_timestamp can serve the new sargable predicate
-- (block_timestamp <= to_timestamp($cutoff)) but each UPDATE still has to
-- filter every matching timestamp row by (chain_id, finality, is_canonical).
--
-- This partial index covers exactly the un-finalized canonical rows and shrinks
-- as the finality watcher promotes them, so steady-state runs touch a tiny
-- index. Predicate intentionally excludes 'finalized' rows — they don't need
-- to be re-examined.
CREATE INDEX IF NOT EXISTS idx_events_finality_pending
  ON raw_protocol_events (chain_id, block_timestamp)
  WHERE is_canonical = TRUE AND finality IN ('tentative', 'l1_posted');

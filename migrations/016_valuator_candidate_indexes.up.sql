-- 016_valuator_candidate_indexes
-- Support valuator candidate fetches that were falling back to full-table scans.

CREATE INDEX IF NOT EXISTS idx_events_valuable_asset_finalized_order
  ON raw_protocol_events (chain_id, asset, block_number, log_index)
  WHERE is_valuable = TRUE
    AND is_canonical = TRUE
    AND finality = 'finalized'
    AND asset IS NOT NULL;

CREATE INDEX IF NOT EXISTS idx_events_earnings_claimed_candidates
  ON raw_protocol_events (chain_id, block_number, log_index)
  WHERE is_valuable = TRUE
    AND is_canonical = TRUE
    AND finality = 'finalized'
    AND event_name = 'EarningsClaimed'
    AND asset IS NULL;

CREATE INDEX IF NOT EXISTS idx_valuations_version_asset_event
  ON event_valuations (valuation_version, asset, event_id);

CREATE INDEX IF NOT EXISTS idx_attempts_failed_version_asset_event
  ON valuation_attempts (valuation_version, asset, event_id)
  WHERE result_status LIKE 'failed_%';

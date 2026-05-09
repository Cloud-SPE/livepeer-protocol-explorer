-- 037_rollup_transcoderupdate_lateral_index
-- Speeds up the lateral fee_share / reward_cut lookup performed by the
-- livepeer-rollups workers, which finds the most recent TranscoderUpdate
-- for an orchestrator at-or-before a given block.
--
-- Pre-existing index `idx_events_transcoder_topic_block` (migration 021)
-- keys on the on-chain log topic, but rollups filter by `to_address`,
-- which forced a BitmapAnd against `idx_events_to_address` and a heap
-- sort. This index lets the planner satisfy the lookup with a single
-- partial-index scan.

CREATE INDEX IF NOT EXISTS idx_events_transcoder_update_by_to_address
    ON raw_protocol_events (
        chain_id,
        to_address,
        block_number DESC,
        log_index DESC
    )
    WHERE is_canonical = TRUE
      AND contract_name = 'BondingManager'
      AND event_name = 'TranscoderUpdate';

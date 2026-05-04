-- Speed transcoder context endpoints:
-- 1. point-in-time/latest params + lifecycle by indexed topic1 (transcoder address)
-- 2. delegator list at block via latest-per-delegator scan

CREATE INDEX CONCURRENTLY IF NOT EXISTS idx_events_transcoder_topic_block
    ON raw_protocol_events (
        chain_id,
        event_name,
        ((raw_event -> 'topics' ->> 1)),
        block_number DESC,
        log_index DESC
    )
    WHERE is_canonical = TRUE
      AND event_name IN ('TranscoderUpdate', 'TranscoderActivated', 'TranscoderDeactivated');

CREATE INDEX CONCURRENTLY IF NOT EXISTS idx_stake_latest_per_delegator_cover
    ON stake_balances_by_block (
        chain_id,
        delegator_address,
        block_number DESC
    )
    INCLUDE (
        delegate_address,
        block_timestamp,
        block_hash,
        bonded_principal,
        pending_stake,
        pending_fees,
        pending_round,
        source
    );

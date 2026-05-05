-- 026_governance_api_indexes
-- Support proposal lookups and joins by decoded proposalId.

CREATE INDEX idx_events_governor_created_proposal
    ON raw_protocol_events (
        chain_id,
        ((raw_event -> 'decoded' ->> 'proposalId')),
        block_number DESC,
        log_index DESC
    )
    WHERE is_canonical = TRUE
      AND event_name = 'ProposalCreated';

CREATE INDEX idx_events_governor_executed_proposal
    ON raw_protocol_events (
        chain_id,
        ((raw_event -> 'decoded' ->> 'proposalId'))
    )
    WHERE is_canonical = TRUE
      AND event_name = 'ProposalExecuted';

CREATE INDEX idx_events_governor_votecast_proposal
    ON raw_protocol_events (
        chain_id,
        ((raw_event -> 'decoded' ->> 'proposalId'))
    )
    WHERE is_canonical = TRUE
      AND event_name = 'VoteCast';

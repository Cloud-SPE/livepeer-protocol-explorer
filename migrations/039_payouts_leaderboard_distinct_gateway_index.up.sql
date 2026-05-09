-- 039_payouts_leaderboard_distinct_gateway_index
-- Covering partial index for the COUNT(DISTINCT from_address) sub-query that
-- backs /payouts/leaderboard's `distinct_gateways` column. After P0's
-- sargable-timestamp rewrite the leaderboard sub-query lands on
-- idx_events_to_address + a heap fetch for from_address; this index
-- collapses both into a single index-only scan keyed by (to_address,
-- block_timestamp DESC) with from_address INCLUDEd in the leaf.
--
-- Predicate is intentionally restrictive (canonical + finalized +
-- WinningTicketRedeemed) — it keeps the index small (~298K rows) and
-- exactly matches the WHERE clause of the leaderboard sub-query, so the
-- planner picks it without rechecks.

CREATE INDEX IF NOT EXISTS idx_events_winning_ticket_to_addr_time
    ON raw_protocol_events (
        to_address,
        block_timestamp DESC
    )
    INCLUDE (from_address)
    WHERE is_canonical = TRUE
      AND finality = 'finalized'
      AND event_name = 'WinningTicketRedeemed';

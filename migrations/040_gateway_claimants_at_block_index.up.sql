-- 040_gateway_claimants_at_block_index
-- Covering index for /gateways/{addr}/claimants/block/{b}, which executes
--   SELECT DISTINCT ON (claimant_address) ...
--     FROM gateway_claimants_by_block
--    WHERE chain_id = $1 AND gateway_address = $2 AND block_number <= $3
--    ORDER BY claimant_address, block_number DESC
--
-- The pre-existing idx_gateway_claimants_recent leads with gateway_address
-- (no chain_id), so the planner falls back to a Seq Scan over ~138K matching
-- rows + external merge sort to disk (~17 MB).
--
-- This index leads with (chain_id, gateway_address) so the seek is fully
-- selective, then orders by (claimant_address ASC, block_number DESC) which
-- exactly matches the DISTINCT ON sort. INCLUDE'd columns turn the lookup
-- into an index-only scan with no heap fetch.

CREATE INDEX IF NOT EXISTS idx_gateway_claimants_chain_gateway_claimant
    ON gateway_claimants_by_block (
        chain_id,
        gateway_address,
        claimant_address,
        block_number DESC
    )
    INCLUDE (claimable_reserve, claimed_reserve, source);

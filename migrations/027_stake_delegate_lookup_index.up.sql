-- 027_stake_delegate_lookup_index
-- Support transcoder delegator-set snapshots without scanning the full stake table.

CREATE INDEX idx_stake_delegate_block_delegator
    ON stake_balances_by_block (
        chain_id,
        delegate_address,
        block_number DESC,
        delegator_address
    );

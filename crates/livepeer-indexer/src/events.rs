//! v1 event signatures via alloy `sol!`. Static codegen — fast, type-safe.
//! Per SPEC §5.4 the registry is per-block-range; v2 dynamic ABI loading is when
//! upgrade-history matters. v1 deals with current Delta-version events.

use alloy::sol;

sol! {
    // BondingManager — strict-decode (§6.2)
    event Reward(address indexed transcoder, uint256 amount);
    // Bond, Unbond, Rebond, WithdrawStake, EarningsClaimed, TransferBond will land in
    // S6.2 alongside their decode glue.
}

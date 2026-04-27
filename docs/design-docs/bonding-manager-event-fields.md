---
title: BondingManager Event Field Mapping
status: accepted
verified: 2026-04-27
resolves: [Q-OD-7]
source: abi/BondingManager.json
---

# BondingManager Event Field Mapping

Canonical mapping from on-chain BondingManager event fields to `raw_protocol_events.amount_native` for valuation.

Resolves SPEC §22 Q-OD-7. Verified against `abi/BondingManager.json` on 2026-04-27.

## Field layouts (verified)

```
event Bond(
    address indexed newDelegate,
    address indexed oldDelegate,
    address indexed delegator,
    uint256         additionalAmount,    ← LPT inflow (per-event)
    uint256         bondedAmount         ← LPT total post-bond (running)
);

event Unbond(
    address indexed delegate,
    address indexed delegator,
    uint256         unbondingLockId,
    uint256         amount,              ← LPT outflow
    uint256         withdrawRound
);

event Rebond(
    address indexed delegate,
    address indexed delegator,
    uint256         unbondingLockId,
    uint256         amount               ← LPT
);

event WithdrawStake(
    address indexed delegator,
    uint256         unbondingLockId,
    uint256         amount,              ← LPT
    uint256         withdrawRound
);

event Reward(
    address indexed transcoder,
    uint256         amount               ← LPT (newly minted)
);

event EarningsClaimed(
    address indexed delegate,
    address indexed delegator,
    uint256         rewards,             ← LPT (multi-asset)
    uint256         fees,                ← ETH (multi-asset)
    uint256         startRound,
    uint256         endRound
);
```

## Decoder mapping

| Event | `asset` | `amount_native` source field |
|---|---|---|
| `Bond` | `LPT` | `additionalAmount` |
| `Unbond` | `LPT` | `amount` |
| `Rebond` | `LPT` | `amount` |
| `WithdrawStake` | `LPT` | `amount` |
| `Reward` | `LPT` | `amount` |
| `EarningsClaimed` | (multi) | `rewards` (LPT) + `fees` (ETH) → two `event_valuations` rows per SPEC §6.8 |

## Critical pitfall

**`Bond.additionalAmount` is the per-event LPT inflow. `Bond.bondedAmount` is the running total post-bond.** Reading `bondedAmount` instead would over-count LPT flow by orders of magnitude (cumulative vs. delta). The seed's SQLite payload uses snake_case `additional_amount` and `bonded_amount` for the same fields — same trap applies if cross-checking.

## Multi-asset valuation: `EarningsClaimed`

Per SPEC §6.8 the canonical event key `(chain_id, tx_hash, log_index)` is preserved (no synthetic sub-index). One row in `raw_protocol_events` with `asset = NULL` and the breakdown preserved in `raw_event` JSONB. Two rows in `event_valuations`: one with `asset = 'LPT'` for `rewards`, one with `asset = 'ETH'` for `fees`.

## Stake-worker implications

The stake-worker (SPEC §11.10) computes `bonded_principal` from the cumulative flow of:
- `Bond.additionalAmount` — credits
- `Unbond.amount` — debits
- `Rebond.amount` — credits (back from unbonding lock)
- `WithdrawStake.amount` — debits (final exit)
- `EarningsClaimed.rewards` — credits (compounded into stake)
- `Reward.amount` — credit applied per delegator's pro-rata share at the round (computed via BondingManager `pendingStake` lookup)

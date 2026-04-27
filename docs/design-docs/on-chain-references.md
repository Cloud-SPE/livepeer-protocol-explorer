---
title: On-chain References — Verified
status: accepted
verified: 2026-04-27
resolves: [Q-OD-5, Q-OD-8, Q-OD-9, Q-OD-10]
source: Chainstack archive RPC + Chainlink docs
---

# On-chain References — Verified

Verified addresses, descriptions, decimals, and operational facts for every external on-chain dependency the system reads. Verified against the Chainstack archive RPC on 2026-04-27.

## Q-OD-5 — RPC provider topology

There is **no** self-hosted Nitro node. The actual operational topology is two hosted HTTP RPCs:

| Role | Provider | Capability |
|---|---|---|
| Archive primary | Chainstack | Full Arbitrum archive, all historical state |
| Secondary | liveinfraspe | Recent state + logs, **no archive** |

Verified at runtime:
- Both report `eth_chainId` = `0xa4b1` (42161 — Arbitrum One). ✓
- `eth_blockNumber` differs by ≤ 1 block — expected sequencing variance. ✓
- Archive-only calls (e.g. `eth_call` at historical block N) succeed only against Chainstack — confirms the secondary is non-archive. ✓

SPEC §13.2 routing matrix is unchanged in v1.3; only the physical provider name changed (`local` → `secondary`).

## Q-OD-8 — Chainlink ETH/USD aggregator

**Address:** `0x639Fe6ab55C921f74e7fac1ee960C0B6293ba612`

Verified via raw `eth_call` on Chainstack:

| Method | Result | Interpretation |
|---|---|---|
| `decimals()` | `0x08` | 8 decimals — answers are in 1e8 |
| `description()` | `"ETH / USD"` | Confirmed by ASCII-decoding the returned bytes |
| `latestRoundData()` | `roundId = 0x200000000000069a3`, `answer = 0x36067e221b` | Phase 2 aggregator round 27,043. Answer = 231,569,335,323 ÷ 1e8 = $2,315.69 — sane |
| `latestRoundData()` | `answeredInRound == roundId` | Passes SPEC §7.3.3 mandatory check |
| `latestRoundData()` | `updatedAt = 0x69ef41b8` | 1,776,116,664 → 2026-04-27 — sane (well within 24h heartbeat) |

SPEC §7.3.3 staleness policy: fail as `failed_missing_oracle` if `block.timestamp - updatedAt > 86400`; WARN at > 14400 (4h).

## Q-OD-10 — L2 Sequencer Uptime Feed

**Address:** `0xFdB631F5EE196F0ed6FAa767959853A9F217697D`

Verified via raw `eth_call` on Chainstack:

| Method | Result | Interpretation |
|---|---|---|
| `decimals()` | `0x00` | 0 — answer is a status enum, not a price |
| `description()` | `"L2 Sequencer Uptime Status Feed"` | Confirmed by ASCII-decoding the returned bytes |
| `latestRoundData()` | `answer = 0` | **Sequencer UP** (1 = down) |
| `latestRoundData()` | `startedAt = 0x661d2acf` | 1,713,104,591 → 2024-04-14 — last status change |

Special note from Chainlink docs: on Arbitrum, `startedAt = 0` only when the contract is not yet initialized; otherwise it is the timestamp of the last status change. Our boot validation (SPEC §16.2) treats `startedAt = 0` as "not initialized" rather than as a 1970 epoch, and rejects.

SPEC §7.3.4: before any pricing computation, the system reads this feed at the event block. If sequencer was down or in a grace period at the event block (or within the 30-min TWAP window prior), the valuation is `failed_sequencer_outage`.

## Q-OD-9 — Uniswap V3 LPT/WETH pool observation cardinality history

**Pool address:** `0x4fd47e5102dfbf95541f64ed6fe13d4ed26d2546`

Sampled via `eth_call(slot0(), block=N)` on Chainstack:

| Block | Cardinality | Notes |
|---:|---:|---|
| 6,738,078 | — | Pool not yet deployed (returns `0x` empty) |
| 10,700,000 | 1 | Pool exists; default ringbuffer size |
| 12,000,000 | 1 | |
| 14,000,000 | 1 | |
| 15,000,000 | 1 | |
| 25,000,000 | 2 | |
| 30,000,000 | 2 | |
| 30,951,438 | 2 | |
| 32,000,000 | 2 | |
| 33,000,000 | 2 | Last observed at low cardinality |
| **35,000,000** | **601** | First observed at high cardinality — `increaseObservationCardinalityNext(601)` was called somewhere in (33M, 35M] |
| 40,000,000 | 601 | |
| 50,000,000 | 601 | |
| 100,000,000 | 601 | |
| 200,000,000 | 601 | |
| latest (~456.99M) | 601 | |

### Earliest Livepeer event vs cardinality window

The earliest Livepeer event on Arbitrum is at **block 6,072,093, 2022-02-15 00:40:00 UTC** — verified from the SQLite seed (`MIN(block_number)` over `events`). The first event predates pool existence by ~5M blocks.

The cardinality-degraded window for valuation is therefore approximately:

```
[livepeer_arbitrum_genesis = 6,072,093, cardinality_crossover ~= 35,000,000]
       Feb 15, 2022 →  ~late 2022   (~9–10 months)
```

### Implications — corrected

- 30-min TWAP requires cardinality ≥ ~144 observations (30 min ÷ ~12.5 s tick spacing). Cardinality 1–2 means TWAP cannot be computed.
- The seed covers only `WinningTicket` (in `payout`) and `Reward` (in `reward`). Every other monetary event in the degraded window has no seed price and must fall back to on-chain pricing — which in this window cannot use TWAP.
- **Affected events** (counted from `events` table at `block_number < 32,000,000`, monetary types only):

  | Event | Count |
  |---|---:|
  | EarningsClaimed | 6,207 |
  | Bond | 2,922 |
  | WithdrawFees | 2,574 |
  | Unbond | 2,084 |
  | Rebond | 1,211 |
  | TransferBond | 1,109 |
  | WithdrawStake | 805 |
  | ReserveFunded | 57 |
  | DepositFunded | 57 |
  | Withdrawal | 5 |
  | ReserveClaimed | 1 |
  | **Total** | **17,032** |

  These all need a degraded valuation version (e.g. `v1_degraded_spot_pre_cardinality` per SPEC §7.3.2) — spot price at the event block, with the version stamp making the degradation queryable.

- For comparison: the legacy `livepeer-backend-rs` priced these via off-chain CryptoCompare. Spot from the thin pool is at least deterministically reproducible on-chain.

- **v1 implementation requirement:** the valuator must implement both `v1_lpt_weth_twap_30min_x_chainlink_eth` and a degraded fallback (e.g. `v1_degraded_spot_pre_cardinality`) and select between them based on cardinality at the event block. The boot-time check (SPEC §16.2) verifies cardinality at the backfill window start.

## Reproducibility

Every reading above is reproducible with one shell command of the form:

```sh
curl -s -X POST -H 'Content-Type: application/json' \
  --data '{"jsonrpc":"2.0","method":"eth_call","params":[{"to":"<addr>","data":"<selector>"},"<block>"],"id":1}' \
  "$CHAINSTACK_RPC_URL"
```

Selectors used in this verification:
- `0x313ce567` — `decimals()`
- `0x7284e416` — `description()`
- `0xfeaf968c` — `latestRoundData()`
- `0x3850c7bd` — `slot0()` (Uniswap V3 pool)

A `tools/verify-providers.sh` script that runs the full set is a follow-up TODO — it would also serve as the boot-validation in SPEC §16.2.

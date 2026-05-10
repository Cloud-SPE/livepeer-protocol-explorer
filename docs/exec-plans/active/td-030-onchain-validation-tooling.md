# TD-030: On-chain validation tooling

**Status:** Open
**Author:** 2026-05-10
**Severity:** medium
**Source:** Post-TD-027 deploy. Comparing the new API to the legacy backend (`livepeer-backend-rs`) surfaced discrepancies in counts, payouts, and gateway names — but tells us only that the two systems disagree, not which one is right. We need an independent ground-truth source.

## Background

The current state of the new API can be validated against the chain itself: every `orchestrator_profile` row corresponds to values readable via `BondingManager` methods, every `broadcaster_profile` row corresponds to values from `TicketBroker.getSenderInfo`. That comparison is cheap (single `eth_call` per entity), deterministic, and authoritative.

The codebase already has every on-chain reader needed:
- `crates/livepeer-staker/src/profile.rs` — calls `transcoderTotalStake`, `getTranscoder`, `getServiceURI`
- `crates/livepeer-staker/src/gateway.rs` — calls `getSenderInfo`
- `crates/livepeer-staker/src/pending.rs` — calls `getDelegator`, `pendingStake`, `pendingFees`

So validation tooling is a thin orchestration layer over existing helpers, not new RPC plumbing.

## Scope

**In:** validation of current-state derived tables (`orchestrator_profile` matview, `broadcaster_profile` matview) against the corresponding contract methods.

**Out (future TDs):**
- Validation of derived/aggregated data (`orch_payouts_daily`, `event_valuations`) — these are computed, not chain-readable. Requires replay or third-party indexer cross-check.
- Historical validation (state at past blocks) — only some contract methods accept a round; archive RPC required; trickier to reason about.
- Cross-check against The Graph subgraph (`https://api.thegraph.com/subgraphs/name/livepeer/arbitrum-one`) — third independent source, valuable, but a separate concern from on-chain validation.
- Validation of ENS / display names — those are off-chain by definition.

## Phases

### Phase A — `scripts/validate-vs-onchain.sh` (~4 hours)

A bash script that picks N entities, hits both the API and the chain, and reports a diff table. No code changes; runnable today against the live prod API.

**Sample selection:**
- Default: 20 random orchestrators from the active set (`/orchestrators?active_only=true&limit=200`, then random pick) plus 10 random gateways from `/gateways?limit=100`.
- Flags:
  - `--orchs N` (default 20)
  - `--gateways N` (default 10)
  - `--orch-addr <addr>` to target a specific orch (repeatable)
  - `--gateway-addr <addr>` same
  - `--all-active` to validate every active entity (slow, RPC-bound; ~100 orchs + 50 gateways = ~150 calls)
  - `--api-url <url>` (default `https://livepeer-api.xode.app`)
  - `--rpc-url <url>` (default reads from `$ARCHIVE_RPC_URL` env, falls back to `$CHAINSTACK_RPC_URL`)

**Per-orchestrator checks:**

| API field | Contract method | Tolerance |
|---|---|---|
| `total_stake` | `BondingManager.transcoderTotalStake(addr)` | ±0.01 LPT |
| `reward_cut_percent` | `BondingManager.getTranscoder(addr).rewardCut` (raw → percent) | exact (within 0.001%) |
| `fee_share_percent` | `BondingManager.getTranscoder(addr).feeShare` (raw → percent) | exact |
| `service_uri` | `ServiceRegistry.getServiceURI(addr)` (call via L2 ServiceRegistry) | exact string match |
| `is_active` | derive from `BondingManager.transcoderStatus(addr) == Registered` | exact bool match |

**Per-gateway checks:**

| API field | Contract method | Tolerance |
|---|---|---|
| `latest_deposit` | `TicketBroker.getSenderInfo(addr).sender.deposit` | ±0.000001 ETH |
| `latest_reserve` | `TicketBroker.getSenderInfo(addr).reserve.fundsRemaining` | ±0.000001 ETH |
| `unlock_in_progress` | `TicketBroker.getSenderInfo(addr).sender.withdrawRound != 0` | exact bool |

**Dependencies:**
- `bash`, `curl`, `jq`, `python3` (for decimal arithmetic)
- Foundry's `cast` for `eth_call` (preferred — handles ABI encoding/decoding cleanly). Fallback to raw `eth_call` JSON-RPC if `cast` is unavailable.

**Output:**
```
=== validate-vs-onchain @ 2026-05-10T15:30:00Z ===
API:  https://livepeer-api.xode.app
RPC:  <archive_url>

--- ORCHESTRATORS (20 sampled) ---
PASS  0x525419...fcb0e  vires-in-numeris.eth   stake=3,905,078.83 cuts=49/4 active=t
PASS  0xac2e50...41919  ...                    stake=3,156,862.03 cuts=40/10 active=t
FAIL  0x4416a2...3b81b  ...                    stake: api=2,621,452.20 chain=2,621,452.21 (diff +0.01 LPT — tolerance OK)
FAIL  0x4f4758...94dbc  ...                    cut: api=fee_share=0 chain=fee_share=500000 (50% — DRIFT)
...

--- GATEWAYS (10 sampled) ---
PASS  0xca33...7fef     deposit=1.234 reserve=0.0
FAIL  0x1234...abcd     deposit: api=0.5 chain=0.6 (DRIFT 0.1 ETH)

=== SUMMARY ===
Orchestrators: 18 PASS, 2 FAIL (1 within tolerance, 1 DRIFT)
Gateways:      9 PASS, 1 FAIL (DRIFT)
Exit code: 1 (any DRIFT fails the run)
```

**Acceptance:**
- Runs against prod with the default sample (20 orchs + 10 gateways) in under 30 seconds.
- Exit code 0 when all entities pass within tolerance; exit code 1 on any DRIFT.
- DRIFT report includes API value, chain value, and absolute diff.
- Re-runnable against any deployment by changing `--api-url`.

### Phase B — `livepeer-orchestrator validate` subcommand (~1 day)

A native Rust subcommand that does the same thing as Phase A but at full scale, with proper batching, structured output, and reusable helpers. Intended for CI / scheduled runs.

**Subcommand surface:**

```text
livepeer-orchestrator validate [OPTIONS]

OPTIONS:
  --env-config <path>       Standard config (DATABASE_URL, RPC URLs)
  --target <kind>           Which set to validate: orchs | gateways | both [default: both]
  --sample-size <n>         Validate N random entities; 0 = all [default: 0]
  --tolerance-lpt <amount>  Per-orchestrator stake tolerance in LPT [default: 0.01]
  --tolerance-eth <amount>  Per-gateway balance tolerance in ETH [default: 0.000001]
  --concurrency <n>         RPC concurrency [default: 12, matches staker default]
  --output <format>         text | json [default: text]
  --fail-on-drift           Exit non-zero on any DRIFT (vs. just reporting)
```

**Implementation:**

- New file `crates/livepeer-orchestrator/src/validate.rs` (~150 LOC).
- Reuses existing on-chain readers from `livepeer-staker` (`profile::fetch_orch_state`, `gateway::fetch_gateway_state`). These already exist as `pub`-able functions.
- Reads the entity list directly from the matview (`SELECT address FROM orchestrator_profile WHERE is_active`), then fans out RPC reads via `buffer_unordered(concurrency)` matching the staker's pattern.
- Compares row-by-row, accumulates a `Vec<Discrepancy>`, prints summary.
- JSON output schema:
  ```json
  {
    "started_at": "...",
    "ended_at": "...",
    "api_source": "matview",
    "rpc_provider": "chainstack",
    "tolerance_lpt": 0.01,
    "tolerance_eth": 0.000001,
    "orchestrators": {
      "checked": 101,
      "matched": 99,
      "drift": [
        {
          "address": "0x...",
          "field": "total_stake",
          "api_value": "2621452.20",
          "chain_value": "2621452.21",
          "diff_abs": "0.01"
        }
      ]
    },
    "gateways": { ... }
  }
  ```

**Compose integration:**

Add to `docker-compose.prod.yml` under the existing `--profile tools` block:

```yaml
  livepeer-validate:
    image: tztcloud/livepeer-valuation-system:latest
    command:
      - livepeer-orchestrator
      - --env-config
      - config/env/prod.yaml
      - validate
      - --output
      - json
      - --fail-on-drift
    profiles: ["tools"]
    environment:
      DATABASE_URL: ${DATABASE_URL}
      CHAINSTACK_RPC_URL: ${CHAINSTACK_RPC_URL}
    depends_on:
      postgres:
        condition: service_healthy
```

Run as a one-shot:

```bash
docker compose -f docker-compose.prod.yml --profile tools \
  run --rm livepeer-validate > /var/log/validate-$(date +%F).json
```

For ongoing monitoring, wire to a cron and pipe to alert-bot on non-zero exit.

**Acceptance:**
- Default invocation validates all active orchs + all gateways (~150 entities) in under 60 seconds with concurrency 12.
- `--output json` produces machine-parseable output suitable for piping into alerting.
- `--fail-on-drift` exits non-zero on any drift; without the flag, exits 0 even with drift (for "report only" mode).
- Reuses staker's RPC concurrency cap so it doesn't disrupt the running daemon when run alongside it.

### Phase C — CI integration (~30 min)

Add to `.github/workflows/` (or wherever the repo's CI lives):

- New job `validate-onchain` that runs the Rust subcommand against a CI-only API URL (or skips on PR builds and only runs on `main` post-deploy).
- Requires the prod DB connection AND archive RPC URL — these need to be CI secrets.
- Schedule: nightly via `cron: '0 6 * * *'` (UTC) so any drift is caught within 24 h.
- On failure: post to Telegram via the existing alert-bot path (TD-012), or fail the workflow loudly.

**Acceptance:**
- The CI workflow runs nightly and produces a JSON artifact.
- Failure surfaces to whoever watches the CI dashboard.

## Risks

| Risk | Mitigation |
|---|---|
| Validation hits Chainstack RPS limits when running --all (150 calls in <60s) | Default concurrency 12 (same as staker — already sustainable). For very large fleets, reduce concurrency or use the secondary RPC for read-only validation work. |
| Block lag between API matview and the eth_call snapshot causes false-positive drift | The matview is refreshed every 30 s (TD-025). Validate writes a single block_number used for ALL eth_call reads (`--block-tag latest` at start of run), so the comparison is against a consistent point-in-time. Non-zero diff > tolerance is real drift, not a race. |
| Tolerance choice masks real bugs (too loose) or floods on rounding (too strict) | Start with the conservative defaults (0.01 LPT / 0.000001 ETH). Tune based on first run's distribution. |
| Active set definition differs (matview "is_active" vs `BondingManager.transcoderStatus`) | The validate subcommand reports `is_active` mismatch as DRIFT explicitly. If the two definitions diverge, that's a real finding, not a tooling bug. |
| `cast` not available on the host running Phase A | Script falls back to a hand-rolled `eth_call` JSON-RPC POST with `python3 -c "int(hex_str, 16)"` decoding for uint256 fields. Documented in the script header. |

## Estimated effort

- Phase A: 4 h (bash script + helpers + manual smoke test)
- Phase B: 8 h (Rust subcommand + reuse of staker helpers + JSON output + tests)
- Phase C: 0.5 h (CI workflow scaffold)
- **Total: ~12.5 hours** (~1.5 days)

Phase A delivers immediate value; Phase B is the strategic investment. Phase C only matters if you want it monitored continuously.

## Dependencies

- TD-027 (insight APIs) — Resolved. Provides the `/orchestrators/{addr}` and `/gateways/{addr}` endpoints we validate against.
- TD-025 / TD-026 (matview refresh) — Resolved. Ensures the matview-backed API reads are within 30 s of base-table writes.
- Existing on-chain readers in `livepeer-staker` — already production-tested.

## Future-proofing

Three obvious follow-ups, intentionally not in this plan:

1. **Historical validation.** `BondingManager` has `getStakeAtRound(addr, round)` and similar methods that accept a round id. A `--at-round N` flag could validate `orch_stake_by_round` rows against historical contract reads. Requires archive RPC for old rounds; expensive on Chainstack at scale.

2. **Subgraph cross-check.** The Livepeer subgraph (TheGraph) is a third independent indexer. A `livepeer-orchestrator validate-vs-subgraph` subcommand would diff our matview against `https://api.thegraph.com/subgraphs/name/livepeer/arbitrum-one`. Useful when on-chain agrees with us but legacy/explorer doesn't (or vice versa) — tells you whether the disagreement is on-chain truth or off-chain interpretation.

3. **Derived-data validation.** `orch_payouts_daily` and `event_valuations` aren't directly chain-readable. Two options:
   - Replay-based: re-run the rollup against the same base-event window with no other changes; results should be byte-identical (extends `livepeer-orchestrator replay`).
   - Subgraph-based: cross-check the subgraph's payout aggregations against ours.
   Probably warrants its own TD.

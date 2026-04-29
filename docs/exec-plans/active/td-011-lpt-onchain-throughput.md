# TD-011: LPT on-chain pass throughput collapses under sustained load

**Status:** Open. Code-side fixes applied; bottleneck is upstream/protocol-level
and unreproducible from a fresh-IP curl test.
**Severity:** Medium — blocks completing the LPT on-chain valuation backfill,
does not affect correctness or determinism of priced rows.
**Last touched:** 2026-04-29.

## Problem statement

The bulk LPT on-chain pass (`livepeer-valuator backfill-all` → `run_onchain_pass_lpt`)
prices ~333 LPT events/min against a fresh, healthy Chainstack archive endpoint.
After 5–15 minutes of sustained load, throughput collapses to **<10 events/min**
indefinitely:

- Valuator process drops to **0% CPU**
- 38–96 ESTAB Chainstack TCP connections held but **idle** (no data flowing)
- All async tasks in `futex_wait_queue` (sleeping on I/O)
- Direct `curl` from the same host returns **HTTP 200 in 120–250ms** even on
  cold archive reads
- No `429`s, no `Retry-After`, no JSON-RPC `-32016` rate-limit code observed
- Reqwest reports `"error sending request for url …"` on a fraction of requests
  (a connection-layer error, not a protocol-level one)

The discrepancy between healthy curl and a wedged long-lived client is the
defining symptom.

## What's been tried (and ruled out)

| Hypothesis | Evidence against | Commit |
|---|---|---|
| Chainstack rate-limit on the IP | 50 parallel curls = 50 × HTTP 200 in <200ms while valuator stalled. No 429s ever observed. | — |
| Chainstack rate-limit on per-request request shape (TWAP `observe()` is too expensive) | Direct curl of the same `observe()` calldata at the same blocks returns 140-190ms. | — |
| HTTP/2 idle stream drop (Cloudflare closes idle streams; pool serves dead session) | Adding `http2_keep_alive_interval(15s) + http2_keep_alive_while_idle(true) + tcp_keepalive(30s)` made it WORSE — valuator deadlocked at 0% CPU with 42 idle ESTAB sockets. | `0b4430b` (then reverted in `f53f06f`) |
| 1-shot retry on `CoreError::Http` would mask transient drops | Retry never triggers because **first calls don't error fast — they hang silently**. ESTAB connections stay open with no traffic. Retry only helps when the first call's HTTP error returns, which isn't the failure mode here. | `f53f06f` |
| Concurrency overshoot (N=32 → 96 in-flight > Chainstack's 50 cap) | True for N=32 (96 conns blocked, ~0/min). Lowering to N=14 / N=8 did NOT recover; throughput stayed at 8-16/min regardless of concurrency. The throttle penalty (if it is one) is sticky beyond what burst control alone can fix. | various |
| Dead-band events being re-attempted infinitely | Real bug — fixed in `b677c85` by skipping events with prior `failed_*` attempts. Confirmed working: `failed_missing_oracle` stable at 9,421. Did NOT explain the throughput collapse on its own. | `b677c85` |
| PG bottleneck / lock contention | `pg_stat_activity` shows 0 active queries during the stalls. PG pool has 10 conns, plenty idle. | — |
| Bulk-seed-attempts CTE running unnecessarily on warm DB | Real perf bug — `valuation_attempts` had bloated to 5.69M rows, the CTE took 4+ minutes per restart. Fixed in `ba82d57` by skipping when `priced_this_run = 0`. | `ba82d57` |

## What's still uncertain

1. **Cloudflare-side connection-tracker behavior with long-lived clients** —
   Chainstack is fronted by Cloudflare. There may be a per-connection or
   per-flow heuristic that down-prioritizes our long-running client without
   ever returning a visible error. A fresh curl appears as a new client and
   gets the fast path; our reqwest pool keeps the same connections alive and
   gets demoted.

2. **Reqwest internal pool state** — adding HTTP/2 keep-alive PINGs put the
   pool into a worse state, suggesting reqwest's connection bookkeeping has
   subtle interactions with HTTP/2 multiplexing and Cloudflare's response
   patterns. We don't have visibility into reqwest's per-stream state from
   inside our binary.

3. **Whether the issue is sticky to the IP** — the `curl`-vs-process gap might
   indicate the throttle is per-TCP-connection or per-flow rather than per-IP.

## Reproduction

State at last kill (2026-04-29 13:53 UTC):

```
event_valuations:        1,184,682
lpt onchain valuations:     39,942
eth onchain valuations:     18,967
seed valuations:         1,125,773
token_prices_by_block:      79,521
rpc_call_cache:          1,576,339
latest priced LPT block: 22,067,586  (target: 457,212,919)
```

Resume command (no DB wipe needed):

```bash
cd /home/mazup/git-repos/crypto-price-feed
bash scripts/full-run-post-indexer.sh \
  > logs/post-indexer-resume-$(date -u +%Y%m%dT%H%M%SZ).log 2>&1 &
disown
```

Steady-state monitoring:

```bash
# Verify Chainstack health (sub-second response means it's not them):
time curl -s --max-time 5 -X POST -H 'Content-Type: application/json' \
  --data '{"jsonrpc":"2.0","method":"eth_blockNumber","params":[],"id":1}' \
  "$CHAINSTACK_RPC_URL"

# Per-minute cache writes (should sustain >100/min on healthy run):
psql "$DATABASE_URL" -c "
  SELECT date_trunc('minute', created_at) AS m, COUNT(*) AS calls
    FROM rpc_call_cache WHERE created_at > now() - interval '15 minutes'
   GROUP BY 1 ORDER BY 1 DESC LIMIT 15;"

# Count of in-flight Chainstack TCP at sample (should be ~CONCURRENCY × 3 when busy):
ss -tn 2>/dev/null | awk '$1=="ESTAB"' | grep -E '104.18' | wc -l
```

Throughput-collapse signature (when it kicks in):
- Per-minute cache writes drop from 80–150 → 1–10
- Valuator CPU drops to 0%
- ESTAB Chainstack TCP stays at ~CONCURRENCY × 3 (looks healthy)
- Direct curl from the same host still returns in <300ms

## Suggested next-session experiments

In order of effort:

### 1. Periodic connection-pool refresh (small change)

Force reqwest to drop and rebuild its connection pool on a timer (e.g., every
30 minutes). If the issue is Cloudflare demoting long-lived flows, fresh
connections should restore throughput.

```rust
// In Provider::new, replace the cached client with a fresh one every 30 min.
// Either via reqwest's pool_idle_timeout(Duration::from_secs(60)) which
// recycles idle connections, or by holding `Arc<Mutex<reqwest::Client>>` and
// rebuilding it on a tokio interval.
```

If this fixes it, the diagnosis is confirmed and we can document it.

### 2. Force HTTP/1.1 + `Connection: close` (medium change)

Disable HTTP/2 entirely and don't reuse connections. Each call gets a fresh
TCP+TLS handshake. Slower per-request (~50ms TLS overhead vs <1ms pool
reuse) but eliminates any pool/multiplexing pathology.

```rust
reqwest::Client::builder()
    .timeout(timeout)
    .http1_only()
    .pool_max_idle_per_host(0)  // never reuse
    .build()?
```

Cost: ~5× per-call overhead (50ms vs 10ms). At our throughput target of
333 events/min × 4 calls = ~22 calls/sec, the extra TLS handshake load is
manageable but our priced rate could halve.

### 3. Add a second archive provider for fan-out (large change)

Wire up `rpc.secondary` in the config (currently `liveinfraspe`, currently
non-archive). If we get a second archive provider (Alchemy, QuickNode,
Infura, etc.), the bulk pass can round-robin or shadow-call across both,
doubling effective concurrency without risking the throttle on either.

The cache layer would key calls per-block as today; the only complication is
which provider's response wins on a tie (last-write-wins is fine since both
should return identical data for finalized blocks).

This is the only path to ≤1h LPT-pass completion.

### 4. Move workload off this host (last resort)

If 1–3 don't help, the issue may be specific to this host's egress (some
network appliance or firewall doing connection tracking). Run the valuator
from a different machine / VPC and see if throughput sustains.

## Code state on disk

All committed, tree clean, builds clean:

```
ba82d57  valuator/seed: short-circuit attempts upsert when no fresh priced rows
f53f06f  core: revert HTTP/2 keep-alive + add 1-shot retry on HTTP-layer errors
0b4430b  core: HTTP/2 + TCP keep-alive on RPC client; restore LPT pass to N=16  (REVERTED)
2335eb6  valuator: settle LPT pass CONCURRENCY at 14 with empirical lesson learned
b677c85  valuator: skip events with prior failed_* attempts in candidate fetch
ff5d7c1  TD-009 (4/3 — final): cross-event concurrency in on-chain LPT pass
ae89b00  TD-009 (3/3): parallel within-event RPC reads in on-chain pass
b05f2fc  TD-009 (2/3): bulk on-chain valuator + bulk staker refresh-pending
f72bb49  TD-009 (1/3): bulk seed pass + finality watcher sargability
```

Current `CONCURRENCY = 14` in `crates/livepeer-valuator/src/onchain.rs`.
Current `Provider::call` does 1-shot retry on `CoreError::Http`, no keep-alive
config.

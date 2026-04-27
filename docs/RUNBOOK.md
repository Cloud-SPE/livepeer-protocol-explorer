# Runbook

Operational procedures for the Livepeer indexing & valuation system.

> **Status: skeleton.** Sections below are the required outline from SPEC §19. Fill each section as the corresponding code lands.

## 1. Daily operations
- Health-check endpoints (`/health` per service)
- Common log patterns
- Dashboard interpretation
- _TODO_

## 2. Backfill procedures
- Running the seed migration (`livepeer-seed-migrator --source-sqlite <path>`)
- Backfilling a date range (`livepeer-indexer backfill --from-block N --to-block M`)
- Re-valuing under a new version
- _TODO_

## 3. Recovery procedures
- pg_dump restore
- Deep replay from `rpc_call_cache` + seeded SQLite
- Partial recovery of a corrupted single table
- _TODO_

## 4. Failure response
For each alert in SPEC §10.6:
- What it means
- Investigation steps
- Resolution
- Escalation criteria

Critical procedures already drafted in SPEC §19.2:
- §19.2.1 Adding a new ABI version
- §19.2.2 Responding to `failed_rpc_divergence`
- §19.2.3 Responding to determinism violation alert

## 5. Schema changes
- Authoring a migration (`sqlx migrate add <name>`)
- Local testing
- Deploying
- Destructive migrations (`--allow-destructive`)
- _TODO_

## 6. ABI updates
Procedure when Livepeer upgrades a tracked contract:
1. Identify upgrade block from `Controller.SetContractInfo`.
2. Fetch new implementation ABI from Arbiscan.
3. Compute sha256, insert `contract_abi_registry` row.
4. Update prior row's `to_block`.
5. Restart services.
6. Run `livepeer-indexer recover-decode-failures`.
7. Regenerate determinism fixture if affected.

See SPEC §19.2.1 for the full procedure.

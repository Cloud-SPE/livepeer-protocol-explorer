# Migrations

Managed with `sqlx-cli`. Each migration is a pair of SQL files:

```
001_<name>.up.sql
001_<name>.down.sql
```

Authoritative schema: [SPEC §11](../docs/product-specs/v1-livepeer-indexer.md#11-database-schema-consolidated-ddl). Migration `001` and onward must reproduce the consolidated DDL exactly.

## Discipline (SPEC §11.1)

1. Migrations are **immutable once merged**. Edits = a new migration.
2. `down` migrations exist for development; **never run in production**. Production rollback is forward-only.
3. Destructive migrations (drops, narrowings, type changes) are prefixed `_destructive_` and require `--allow-destructive` to apply.
4. Migrations are **idempotent on re-run** — use `IF NOT EXISTS` where appropriate.
5. Each migration includes a comment block at the top: purpose, ticket reference, author, runtime impact.

## Planned migration sequence

| # | Migration | SPEC ref |
|---|---|---|
| 001 | create_indexer_checkpoints | §11.2 |
| 002 | create_contract_abi_registry | §11.3 |
| 003 | create_raw_protocol_events | §11.4 |
| 004 | create_seeded_event_prices | §11.5 |
| 005 | create_token_prices_by_block | §11.6 |
| 006 | create_event_valuations | §11.7 |
| 007 | create_valuation_attempts | §11.8 |
| 008 | create_decode_failures | §11.9 |
| 009 | create_stake_balances_by_block | §11.10 |
| 010 | create_delegator_registry | §11.11 |
| 011 | create_rpc_call_cache | §11.12 |
| 012 | create_rpc_divergence_failures | §11.13 |
| 013 | create_reorg_events | §11.14 |
| 014 | create_reorg_mutations | §11.15 |

## Authoring

```sh
cargo install sqlx-cli --version 0.8.3 --no-default-features --features rustls,postgres
sqlx migrate add -r <name>
```

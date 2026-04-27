# Tests

Integration tests and the determinism replay test live here.

- `fixtures/` — committed test fixtures: `rpc_cache.json`, `seed.sqlite`, `expected_hashes.json`. See [DETERMINISM.md](../docs/DETERMINISM.md).
- `replay/` (TODO) — the replay determinism test (SPEC §12.4) — the load-bearing CI gate.
- Per-crate unit tests live next to their code under each crate's `src/`.

# Tests

Integration tests and the determinism replay fixture live here.

- `fixtures/` — one or more committed replay cases, each containing:
  - `seed.sqlite`
  - `rpc_cache.csv`
  - `replay_checkpoints.csv`
  - `fixture.env`
  - `expected_hashes.json`
- The CI gate is script-driven via `scripts/run-determinism-replay.sh`.
- Per-crate unit tests live next to their code under each crate's `src/`.

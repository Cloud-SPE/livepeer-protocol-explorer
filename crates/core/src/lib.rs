// Shared crate for the Livepeer indexing & valuation system.
//
// Intended modules (none implemented yet — scaffold only):
//   config       - layered config loader (static yaml + env yaml + .env)
//   db           - sqlx pool, migration runner, schema-version check
//   rpc          - alloy provider, routing matrix, cache, cross-check
//   abi          - ABI registry, hash verification, decoder
//   types        - canonical event/valuation/stake types
//   metrics      - prometheus registry + standard counters/gauges/histograms
//   error        - shared error enum (thiserror)
//
// The product spec is at docs/product-specs/v1-livepeer-indexer.md.

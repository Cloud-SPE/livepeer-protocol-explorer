/// <reference types="vite/client" />

// Per-deploy config now lives in the API server's `/config.json` env-driven
// handler (see `crates/livepeer-api/src/routes/operational.rs`). Vite-time
// env vars are no longer used to inject app config. This file just keeps
// the standard Vite type reference around.

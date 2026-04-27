//! JSON-RPC client + cache + cross-check.
//!
//! Per SPEC §13.2, `Provider` does not embed the routing matrix; that's the indexer's
//! job. This module just provides the typed primitives. The cache (§11.12) and the
//! cross-check (§7.6) hang off these primitives.

pub mod cache;
pub mod cross_check;
pub mod provider;

pub use provider::{BlockTag, Provider};

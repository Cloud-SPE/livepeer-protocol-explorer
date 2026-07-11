//! External-surface adapters: read-only Postgres, Prometheus scrape, and
//! (Phase 2) the Docker socket proxy. Each is independently fallible so a
//! single dead surface degrades gracefully instead of failing every tool.

pub mod db;
pub mod docker;
pub mod metrics;
pub mod sql_guard;

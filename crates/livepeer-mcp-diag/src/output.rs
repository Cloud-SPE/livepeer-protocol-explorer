//! Token-budget helpers. Tool output is consumed by an LLM, so every
//! unbounded surface (SQL rows, log tails) is hard-capped and long cells are
//! truncated. Callers append an explicit "truncated" marker so nothing silently
//! looks complete when it isn't.

use serde_json::Value;

/// Max rows returned by `raw_sql`.
pub const MAX_RAW_ROWS: i64 = 200;
/// Max characters kept per string cell before truncation.
pub const MAX_CELL_CHARS: usize = 200;
/// Default / max lines returned by `worker_logs`.
pub const DEFAULT_LOG_LINES: usize = 200;
pub const MAX_LOG_LINES: usize = 1_000;
/// Byte ceiling for a `worker_logs` response body.
pub const MAX_LOG_BYTES: usize = 64 * 1024;

/// Recursively truncate every string in a JSON value to `MAX_CELL_CHARS`,
/// appending an ellipsis marker with the original length.
pub fn truncate_cells(v: &mut Value) {
    match v {
        Value::String(s) => {
            if s.chars().count() > MAX_CELL_CHARS {
                let kept: String = s.chars().take(MAX_CELL_CHARS).collect();
                let orig = s.chars().count();
                *s = format!("{kept}…[+{} chars]", orig - MAX_CELL_CHARS);
            }
        }
        Value::Array(arr) => arr.iter_mut().for_each(truncate_cells),
        Value::Object(map) => map.values_mut().for_each(truncate_cells),
        _ => {}
    }
}

/// Cap a log tail to at most `requested` lines (clamped to `MAX_LOG_LINES`) and
/// `MAX_LOG_BYTES`. Returns the kept lines (most recent) and whether the tail
/// was truncated.
pub fn cap_log_lines(lines: Vec<String>, requested: usize) -> (Vec<String>, bool) {
    let want = requested.clamp(1, MAX_LOG_LINES);
    let mut truncated = lines.len() > want;
    // Keep the most recent `want` lines.
    let start = lines.len().saturating_sub(want);
    let mut kept: Vec<String> = lines[start..].to_vec();

    // Enforce the byte ceiling by dropping from the front (oldest) if needed.
    let mut total: usize = kept.iter().map(|l| l.len() + 1).sum();
    while total > MAX_LOG_BYTES && kept.len() > 1 {
        let removed = kept.remove(0);
        total -= removed.len() + 1;
        truncated = true;
    }
    (kept, truncated)
}

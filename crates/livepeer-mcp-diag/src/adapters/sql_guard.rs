//! Defense-in-depth guard for the `raw_sql` tool.
//!
//! IMPORTANT: this string check is NOT the security boundary. It is trivially
//! bypassable (a `WITH` CTE can call a volatile/data-modifying function, a
//! `SELECT` can invoke a function with side effects). The real boundary is the
//! `diag_ro` Postgres role + `default_transaction_read_only` session guard in
//! `adapters::db`. This guard exists only to reject obvious mistakes early and
//! to keep the wrapped-`LIMIT` transform in `raw_select` well-formed (single
//! statement, no trailing semicolon).

/// Validate and normalize a raw SQL statement. Returns the trimmed statement
/// (trailing `;` removed) on success, or a human-readable rejection reason.
pub fn validate(sql: &str) -> Result<String, String> {
    let trimmed = sql.trim();
    if trimmed.is_empty() {
        return Err("empty query".to_string());
    }

    // Strip a single trailing semicolon so the query can be wrapped as a
    // subquery. Anything after it (a second statement) is rejected below.
    let without_trailing = trimmed.strip_suffix(';').unwrap_or(trimmed).trim();

    // Reject multi-statement input: no `;` may remain in the body. This is a
    // coarse check — it also rejects semicolons inside string literals, which
    // is acceptable for a diagnostics escape hatch.
    if without_trailing.contains(';') {
        return Err("multiple statements are not allowed (found ';' in body)".to_string());
    }

    // Must be a read shape: SELECT or WITH (CTE) only.
    let lead: String = without_trailing
        .chars()
        .take_while(|c| c.is_ascii_alphabetic())
        .collect::<String>()
        .to_ascii_uppercase();
    if lead != "SELECT" && lead != "WITH" {
        return Err(format!(
            "only SELECT/WITH queries are permitted (statement starts with '{lead}')"
        ));
    }

    Ok(without_trailing.to_string())
}

#[cfg(test)]
mod tests {
    use super::validate;

    #[test]
    fn accepts_select_and_with() {
        assert_eq!(validate("SELECT 1").unwrap(), "SELECT 1");
        assert_eq!(validate("  select 1 ;  ").unwrap(), "select 1");
        assert!(validate("WITH x AS (SELECT 1) SELECT * FROM x").is_ok());
    }

    #[test]
    fn rejects_writes_and_multistatement() {
        assert!(validate("UPDATE t SET x=1").is_err());
        assert!(validate("INSERT INTO t VALUES (1)").is_err());
        assert!(validate("DELETE FROM t").is_err());
        assert!(validate("SELECT 1; DROP TABLE t").is_err());
        assert!(validate("").is_err());
    }
}

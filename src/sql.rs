//! Pulling clean, runnable SQL out of a (possibly chatty) LLM reply.
//!
//! Extracts clean SQL from an LLM reply (`extract_sql` + `is_sql_valid`). The LLM often wraps
//! SQL in markdown fences or adds prose; we extract just the statement.

use once_cell::sync::Lazy;
use regex::Regex;

// (?is) = case-insensitive + dot-matches-newline. Patterns are tried in order.
static RE_CTAS: Lazy<Regex> =
    Lazy::new(|| Regex::new(r"(?is)\bCREATE\s+TABLE\b.*?\bAS\b.*?;").unwrap());
static RE_WITH: Lazy<Regex> = Lazy::new(|| Regex::new(r"(?is)\bWITH\b\s+.*?;").unwrap());
static RE_SELECT: Lazy<Regex> = Lazy::new(|| Regex::new(r"(?is)\bSELECT\b\s+.*?;").unwrap());
static RE_SQL_FENCE: Lazy<Regex> =
    Lazy::new(|| Regex::new(r"(?is)```sql\s*\n(.*?)```").unwrap());
static RE_ANY_FENCE: Lazy<Regex> = Lazy::new(|| Regex::new(r"(?is)```(.*?)```").unwrap());

/// Extract the SQL statement from an LLM response.
///
/// Tries, in order: CREATE TABLE ... AS, a WITH (CTE) statement, a SELECT
/// statement, a ```sql fenced block, then any fenced block. If nothing matches,
/// returns the whole (trimmed) response.
pub fn extract_sql(response: &str) -> String {
    if let Some(m) = RE_CTAS.find_iter(response).last() {
        return m.as_str().trim().to_string();
    }
    if let Some(m) = RE_WITH.find_iter(response).last() {
        return m.as_str().trim().to_string();
    }
    if let Some(m) = RE_SELECT.find_iter(response).last() {
        return m.as_str().trim().to_string();
    }
    if let Some(c) = RE_SQL_FENCE.captures_iter(response).last() {
        return c[1].trim().to_string();
    }
    if let Some(c) = RE_ANY_FENCE.captures_iter(response).last() {
        return c[1].trim().to_string();
    }
    response.trim().to_string()
}

/// Whether we should actually run this SQL. Like OpenDbPylot, we only auto-run reads
/// (SELECT / WITH), never writes — a safety guard.
pub fn is_sql_valid(sql: &str) -> bool {
    let upper = sql.trim_start().to_uppercase();
    upper.starts_with("SELECT") || upper.starts_with("WITH")
}

/// Whether this SQL is safe to execute — i.e. read-only. This is the security gate
/// for the `run_sql` tool: the LLM (or anything injected into it) must not be able
/// to modify or destroy the database.
///
/// - A plain `SELECT` is read-only.
/// - A `WITH` (CTE) is read-only UNLESS it embeds a data-modifying statement
///   (Postgres allows `WITH t AS (DELETE ...) SELECT ...`) — those are rejected.
/// - Anything else (DROP/DELETE/UPDATE/INSERT/ALTER/CREATE/TRUNCATE/…) is rejected.
pub fn is_read_only(sql: &str) -> bool {
    if !regex_read_only(sql) {
        return false;
    }
    // Second layer: the AST check can only make the verdict stricter, never
    // looser (a parse failure falls back to the regex verdict above). Catches
    // e.g. multi-statement smuggling: "SELECT 1; DROP TABLE x".
    !matches!(crate::schema::is_read_only_ast(sql), Some(false))
}

fn regex_read_only(sql: &str) -> bool {
    let trimmed = sql.trim_start();
    let upper = trimmed.to_uppercase();

    if upper.starts_with("SELECT") {
        return true;
    }
    if upper.starts_with("WITH") {
        static RE_WRITE: Lazy<Regex> = Lazy::new(|| {
            Regex::new(r"(?i)\b(INSERT|UPDATE|DELETE|DROP|ALTER|CREATE|TRUNCATE|REPLACE|MERGE|GRANT|REVOKE)\b")
                .unwrap()
        });
        return !RE_WRITE.is_match(trimmed);
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strips_sql_fence() {
        let r = "Here you go:\n```sql\nSELECT * FROM users;\n```";
        assert_eq!(extract_sql(r), "SELECT * FROM users;");
    }

    #[test]
    fn plain_select() {
        assert_eq!(extract_sql("SELECT 1 FROM t;"), "SELECT 1 FROM t;");
    }

    #[test]
    fn validity() {
        assert!(is_sql_valid("SELECT 1"));
        assert!(is_sql_valid("  with x as (...) select 1"));
        assert!(!is_sql_valid("DROP TABLE users"));
    }

    #[test]
    fn read_only_gate_blocks_writes() {
        // Reads are allowed.
        assert!(is_read_only("SELECT * FROM orders"));
        assert!(is_read_only("  select 1"));
        assert!(is_read_only("WITH t AS (SELECT 1) SELECT * FROM t"));
        // A plain SELECT containing the word 'delete' as data is still fine.
        assert!(is_read_only("SELECT * FROM orders WHERE status = 'deleted'"));

        // Writes are blocked.
        assert!(!is_read_only("DROP TABLE users"));
        assert!(!is_read_only("DELETE FROM orders"));
        assert!(!is_read_only("UPDATE orders SET total = 0"));
        assert!(!is_read_only("INSERT INTO orders VALUES (1)"));
        assert!(!is_read_only("ALTER TABLE orders ADD COLUMN x INT"));
        assert!(!is_read_only("TRUNCATE orders"));
        // Data-modifying CTE (Postgres) is blocked.
        assert!(!is_read_only("WITH t AS (DELETE FROM orders RETURNING *) SELECT * FROM t"));
        // Multi-statement smuggling is blocked by the AST layer.
        assert!(!is_read_only("SELECT 1; DROP TABLE users"));
    }
}

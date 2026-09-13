//! Portable categorical hints: "orders.status has values: pending, shipped, …".
//!
//! # Why this matters for accuracy
//!
//! A question like *"how many orders were cancelled?"* needs a literal the model
//! has never seen. Response guideline 2 tells it to emit an `intermediate_sql`
//! probe to go and find the distinct values — an extra round-trip, and one that
//! is refused outright when `allow_llm_to_see_data` is off, which is the safe
//! default. Either way the answer is worse than it needs to be.
//!
//! Giving the model the distinct values of low-cardinality columns up front
//! removes that round-trip for the common case, and is the single cheapest
//! accuracy win available in this pipeline: the value is *right there* in the
//! prompt, so the model neither guesses a spelling nor asks to look.
//!
//! # Why it is here rather than per-backend
//!
//! `SqlRunner::categorical_hints` has a no-op default, and only the SQLite
//! runner overrode it. So the feature worked on the demo database and silently
//! did nothing on PostgreSQL, MySQL and DuckDB — the backends anyone actually
//! runs in production. This implementation is written against `introspect_schema`
//! plus `run_sql`, so every backend that can do those two things gets it.

use anyhow::Result;
use sqlparser::ast::Statement;
use sqlparser::dialect::GenericDialect;
use sqlparser::parser::Parser;

use super::SqlRunner;

/// Tables larger than this are skipped: enumerating a column on a very large
/// table is not worth the scan, and a column with few distinct values across
/// millions of rows is usually an id-like code the model cannot use anyway.
pub const MAX_TABLE_ROWS: usize = 200_000;

/// A column with more distinct values than this is not categorical, and listing
/// them would crowd out the DDL in the prompt budget.
pub const MAX_DISTINCT: usize = 50;

/// Don't enumerate more columns than this in total, so a wide schema cannot
/// produce a prompt that is nothing but value lists.
pub const MAX_COLUMNS: usize = 40;

/// How an identifier is quoted for the target dialect.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Quoting {
    /// `"name"` — PostgreSQL, SQLite, DuckDB, and the SQL standard.
    DoubleQuote,
    /// `` `name` `` — MySQL in its default mode.
    Backtick,
}

impl Quoting {
    /// Quote an identifier, escaping any embedded quote character.
    ///
    /// Identifiers come from the database's own catalogue rather than from user
    /// input, but a table named `we"ird` would otherwise produce SQL that fails
    /// to parse — and escaping is what keeps this from being an injection point
    /// if a caller ever passes something less trusted.
    pub fn quote(self, identifier: &str) -> String {
        match self {
            Quoting::DoubleQuote => format!("\"{}\"", identifier.replace('"', "\"\"")),
            Quoting::Backtick => format!("`{}`", identifier.replace('`', "``")),
        }
    }
}

/// A column worth enumerating.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TextColumn {
    pub table: String,
    pub column: String,
}

/// Pull text-like columns out of `CREATE TABLE` DDL.
///
/// Only text columns are considered: numeric and date columns rarely carry the
/// kind of named category a question refers to by word, and enumerating them
/// fills the prompt with noise.
pub fn text_columns(ddl_list: &[String]) -> Vec<TextColumn> {
    let dialect = GenericDialect {};
    let mut out = Vec::new();

    for ddl in ddl_list {
        let Ok(statements) = Parser::parse_sql(&dialect, ddl) else {
            continue;
        };
        for statement in statements {
            let Statement::CreateTable(create) = statement else {
                continue;
            };
            let table = last_segment(&create.name.to_string());
            for column in &create.columns {
                if is_text_type(&column.data_type.to_string()) {
                    out.push(TextColumn {
                        table: table.clone(),
                        column: column.name.value.clone(),
                    });
                }
            }
        }
    }
    out
}

/// Whether a declared SQL type holds text.
///
/// Matches on the rendered type name rather than the `DataType` enum so that a
/// dialect-specific spelling (`character varying`, `nvarchar2`, an unrecognised
/// domain type) is still caught.
pub fn is_text_type(rendered: &str) -> bool {
    let t = rendered.to_ascii_uppercase();
    // SQLite lets a column have no declared type at all, and stores text in it.
    if t.trim().is_empty() {
        return true;
    }
    // Guard against `TEXT[]`, `JSON`, and other containers: their values are not
    // a small set of names the model can filter on.
    if t.contains('[') || t.contains("JSON") || t.contains("XML") || t.contains("BLOB") {
        return false;
    }
    t.contains("CHAR") || t.contains("TEXT") || t.contains("CLOB") || t.contains("STRING")
        || t.contains("ENUM")
}

fn last_segment(name: &str) -> String {
    name.rsplit('.')
        .next()
        .unwrap_or(name)
        .trim_matches(|c| c == '"' || c == '`' || c == '[' || c == ']')
        .to_string()
}

/// SQL that counts a table's rows, stopping at `cap + 1`.
///
/// Counting over a bounded subquery scans at most `cap + 1` rows, so this stays
/// cheap on a billion-row table — it answers "bigger than the cap?", which is
/// all the decision needs.
pub fn bounded_count_sql(q: Quoting, table: &str, cap: usize) -> String {
    format!(
        "SELECT COUNT(*) FROM (SELECT 1 FROM {} LIMIT {}) AS bounded",
        q.quote(table),
        cap + 1
    )
}

/// SQL that counts a column's distinct values, stopping at `cap + 1`.
pub fn bounded_distinct_sql(q: Quoting, table: &str, column: &str, cap: usize) -> String {
    let col = q.quote(column);
    format!(
        "SELECT COUNT(*) FROM (SELECT DISTINCT {col} FROM {} WHERE {col} IS NOT NULL LIMIT {}) AS bounded",
        q.quote(table),
        cap + 1
    )
}

/// SQL that lists a column's distinct values.
pub fn distinct_values_sql(q: Quoting, table: &str, column: &str, limit: usize) -> String {
    let col = q.quote(column);
    format!(
        "SELECT DISTINCT {col} FROM {} WHERE {col} IS NOT NULL ORDER BY {col} LIMIT {limit}",
        q.quote(table)
    )
}

/// Render one hint line.
pub fn format_hint(table: &str, column: &str, values: &[String]) -> String {
    format!(
        "Column {table}.{column} contains these values: {}.",
        values.join(", ")
    )
}

/// Build categorical hints for any runner that can introspect and query.
///
/// Every step is best-effort: a permission error on one table costs that
/// table's hints, never the whole set. Hints are an accuracy aid, and failing
/// the query because one column could not be sampled would be a far worse
/// trade than returning the hints we did get.
pub async fn collect(runner: &dyn SqlRunner, q: Quoting) -> Result<Vec<String>> {
    let ddl = runner.introspect_schema().await?;
    let columns = text_columns(&ddl);

    let mut hints = Vec::new();
    let mut skipped_tables: Vec<String> = Vec::new();
    let mut current_table = String::new();

    for candidate in columns {
        if hints.len() >= MAX_COLUMNS {
            break;
        }

        // One row-count per table, not per column.
        if candidate.table != current_table {
            current_table = candidate.table.clone();
            let sql = bounded_count_sql(q, &candidate.table, MAX_TABLE_ROWS);
            let too_big = match runner.run_sql(&sql).await {
                Ok(result) => first_number(&result).is_none_or(|n| n > MAX_TABLE_ROWS as i64),
                // Unreadable table: skip it rather than failing the whole run.
                Err(_) => true,
            };
            if too_big {
                skipped_tables.push(candidate.table.clone());
            }
        }
        if skipped_tables.contains(&candidate.table) {
            continue;
        }

        let sql = bounded_distinct_sql(q, &candidate.table, &candidate.column, MAX_DISTINCT);
        let Ok(result) = runner.run_sql(&sql).await else {
            continue;
        };
        let Some(distinct) = first_number(&result) else {
            continue;
        };
        // A constant column tells the model nothing; a high-cardinality one is
        // not a category and would crowd out the DDL.
        if distinct < 2 || distinct as usize > MAX_DISTINCT {
            continue;
        }

        let sql = distinct_values_sql(q, &candidate.table, &candidate.column, MAX_DISTINCT);
        let Ok(result) = runner.run_sql(&sql).await else {
            continue;
        };
        let values: Vec<String> = result.rows.iter().filter_map(|r| r.first().cloned()).collect();
        if values.len() < 2 {
            continue;
        }
        hints.push(format_hint(&candidate.table, &candidate.column, &values));
    }

    Ok(hints)
}

/// Read the first cell of the first row as an integer.
fn first_number(result: &super::QueryResult) -> Option<i64> {
    result.rows.first()?.first()?.trim().parse().ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    // ── Identifier quoting ───────────────────────────────────────────

    #[test]
    fn identifiers_are_quoted_for_the_dialect() {
        assert_eq!(Quoting::DoubleQuote.quote("orders"), "\"orders\"");
        assert_eq!(Quoting::Backtick.quote("orders"), "`orders`");
    }

    #[test]
    fn an_embedded_quote_is_escaped_not_passed_through() {
        // Otherwise the generated SQL fails to parse — or worse, if an
        // identifier ever comes from somewhere less trusted than the catalogue.
        assert_eq!(Quoting::DoubleQuote.quote(r#"we"ird"#), r#""we""ird""#);
        assert_eq!(Quoting::Backtick.quote("we`ird"), "`we``ird`");
    }

    #[test]
    fn a_quote_cannot_terminate_the_identifier_early() {
        // The payload stays inside one quoted identifier, so the statement is
        // still a single read-only SELECT rather than two statements.
        let sql = bounded_count_sql(Quoting::DoubleQuote, r#"x" ; DROP TABLE y; --"#, 10);

        assert!(
            crate::sql::is_read_only(&sql),
            "escaping failed and a second statement got through: {sql}"
        );

        let parsed = sqlparser::parser::Parser::parse_sql(&GenericDialect {}, &sql)
            .expect("the escaped identifier should still parse");
        assert_eq!(parsed.len(), 1, "exactly one statement: {sql}");
    }

    // ── Type classification ──────────────────────────────────────────

    #[test]
    fn text_types_are_recognised_across_dialects() {
        for t in [
            "TEXT",
            "VARCHAR(255)",
            "character varying",
            "CHAR(2)",
            "NVARCHAR2(50)",
            "CLOB",
            "STRING",
            "ENUM('a','b')",
        ] {
            assert!(is_text_type(t), "{t} should count as text");
        }
    }

    #[test]
    fn an_untyped_sqlite_column_counts_as_text() {
        // SQLite allows `CREATE TABLE t (x)`, and stores text in it.
        assert!(is_text_type(""));
        assert!(is_text_type("   "));
    }

    #[test]
    fn non_text_types_are_skipped() {
        for t in ["INTEGER", "BIGINT", "REAL", "NUMERIC(10,2)", "DATE", "TIMESTAMP", "BOOLEAN"] {
            assert!(!is_text_type(t), "{t} should not count as text");
        }
    }

    #[test]
    fn containers_are_skipped_even_though_they_hold_text() {
        // Their values are not a small set of names a question can filter on.
        for t in ["TEXT[]", "JSON", "JSONB", "XML", "BLOB"] {
            assert!(!is_text_type(t), "{t} should not be enumerated");
        }
    }

    // ── DDL parsing ──────────────────────────────────────────────────

    #[test]
    fn text_columns_are_extracted_from_ddl() {
        let ddl = vec![
            "CREATE TABLE orders (id INTEGER, status TEXT, total REAL);".to_string(),
            "CREATE TABLE customers (id INTEGER, name VARCHAR(100), country CHAR(2));".to_string(),
        ];
        let cols = text_columns(&ddl);

        assert_eq!(cols.len(), 3, "{cols:?}");
        assert!(cols.contains(&TextColumn { table: "orders".into(), column: "status".into() }));
        assert!(cols.contains(&TextColumn { table: "customers".into(), column: "country".into() }));
        assert!(
            !cols.iter().any(|c| c.column == "total"),
            "a REAL column is not text"
        );
    }

    #[test]
    fn a_schema_qualified_table_keeps_only_its_name() {
        let ddl = vec!["CREATE TABLE public.orders (status TEXT);".to_string()];
        assert_eq!(text_columns(&ddl)[0].table, "orders");
    }

    #[test]
    fn unparseable_ddl_is_skipped_rather_than_failing_the_run() {
        let ddl = vec![
            "this is not sql at all".to_string(),
            "CREATE TABLE good (status TEXT);".to_string(),
        ];
        let cols = text_columns(&ddl);
        assert_eq!(cols.len(), 1, "the parseable table must still be found");
    }

    #[test]
    fn an_empty_schema_yields_no_columns() {
        assert!(text_columns(&[]).is_empty());
    }

    // ── Generated SQL ────────────────────────────────────────────────

    #[test]
    fn the_row_count_is_bounded_so_it_stays_cheap_on_huge_tables() {
        let sql = bounded_count_sql(Quoting::DoubleQuote, "orders", 200_000);
        assert!(sql.contains("LIMIT 200001"), "{sql}");
        assert!(sql.contains("COUNT(*)"));
    }

    #[test]
    fn the_distinct_count_is_bounded_too() {
        let sql = bounded_distinct_sql(Quoting::DoubleQuote, "orders", "status", 50);
        assert!(sql.contains("LIMIT 51"), "{sql}");
        assert!(sql.contains("IS NOT NULL"), "nulls are not a category: {sql}");
    }

    #[test]
    fn generated_sql_is_read_only() {
        // These run against a live database, so they must pass the same gate
        // everything else does.
        let statements = [
            bounded_count_sql(Quoting::DoubleQuote, "t", 10),
            bounded_distinct_sql(Quoting::DoubleQuote, "t", "c", 10),
            distinct_values_sql(Quoting::DoubleQuote, "t", "c", 10),
        ];
        for sql in statements {
            assert!(crate::sql::is_read_only(&sql), "not read-only: {sql}");
        }
    }

    #[test]
    fn the_value_list_is_ordered_for_stable_output() {
        // An unordered list would make hints, and therefore prompts, differ
        // between runs — which makes an accuracy regression impossible to
        // attribute.
        let sql = distinct_values_sql(Quoting::DoubleQuote, "orders", "status", 50);
        assert!(sql.contains("ORDER BY"), "{sql}");
    }

    #[test]
    fn mysql_sql_uses_backticks_throughout() {
        let sql = bounded_distinct_sql(Quoting::Backtick, "orders", "status", 50);
        assert!(sql.contains("`orders`"));
        assert!(sql.contains("`status`"));
        assert!(!sql.contains('"'));
    }

    // ── Hint rendering ───────────────────────────────────────────────

    #[test]
    fn a_hint_names_the_column_and_lists_the_values() {
        let hint = format_hint("orders", "status", &["pending".into(), "shipped".into()]);
        assert!(hint.contains("orders.status"));
        assert!(hint.contains("pending, shipped"));
    }

    // ── End to end against a real SQLite database ────────────────────

    #[tokio::test]
    async fn hints_are_collected_from_a_live_database() {
        let path = std::env::temp_dir().join(format!("hints_live_{}.db", std::process::id()));
        let _ = std::fs::remove_file(&path);
        let db = crate::sqlrunner::sqlite::SqliteRunner::new(path.to_string_lossy().to_string());

        db.run_sql("CREATE TABLE orders (id INTEGER, status TEXT, note TEXT)").await.unwrap();
        db.run_sql(
            "INSERT INTO orders VALUES (1,'pending','a'),(2,'shipped','b'),(3,'pending','c')",
        )
        .await
        .unwrap();

        let hints = collect(&db, Quoting::DoubleQuote).await.unwrap();

        assert!(
            hints.iter().any(|h| h.contains("orders.status")
                && h.contains("pending")
                && h.contains("shipped")),
            "expected a status hint, got {hints:?}"
        );
        let _ = std::fs::remove_file(&path);
    }

    #[tokio::test]
    async fn a_column_where_every_row_differs_is_not_enumerated() {
        // An id-like column is not a category; listing it would be pure noise.
        let path = std::env::temp_dir().join(format!("hints_uniq_{}.db", std::process::id()));
        let _ = std::fs::remove_file(&path);
        let db = crate::sqlrunner::sqlite::SqliteRunner::new(path.to_string_lossy().to_string());

        db.run_sql("CREATE TABLE t (code TEXT)").await.unwrap();
        let values: Vec<String> = (0..(MAX_DISTINCT + 10)).map(|i| format!("('v{i}')")).collect();
        db.run_sql(&format!("INSERT INTO t VALUES {}", values.join(","))).await.unwrap();

        let hints = collect(&db, Quoting::DoubleQuote).await.unwrap();
        assert!(
            !hints.iter().any(|h| h.contains("t.code")),
            "a high-cardinality column must be skipped: {hints:?}"
        );
        let _ = std::fs::remove_file(&path);
    }

    #[tokio::test]
    async fn a_constant_column_is_not_enumerated() {
        let path = std::env::temp_dir().join(format!("hints_const_{}.db", std::process::id()));
        let _ = std::fs::remove_file(&path);
        let db = crate::sqlrunner::sqlite::SqliteRunner::new(path.to_string_lossy().to_string());

        db.run_sql("CREATE TABLE t (kind TEXT)").await.unwrap();
        db.run_sql("INSERT INTO t VALUES ('same'),('same'),('same')").await.unwrap();

        let hints = collect(&db, Quoting::DoubleQuote).await.unwrap();
        assert!(
            !hints.iter().any(|h| h.contains("t.kind")),
            "one distinct value tells the model nothing: {hints:?}"
        );
        let _ = std::fs::remove_file(&path);
    }

    #[tokio::test]
    async fn an_empty_database_yields_no_hints_rather_than_an_error() {
        let path = std::env::temp_dir().join(format!("hints_empty_{}.db", std::process::id()));
        let _ = std::fs::remove_file(&path);
        let db = crate::sqlrunner::sqlite::SqliteRunner::new(path.to_string_lossy().to_string());
        db.run_sql("CREATE TABLE t (id INTEGER)").await.unwrap();

        assert!(collect(&db, Quoting::DoubleQuote).await.unwrap().is_empty());
        let _ = std::fs::remove_file(&path);
    }
}

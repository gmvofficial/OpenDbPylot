//! Pre-execution schema validation: catch hallucinated tables/columns *before*
//! the query hits the database, so the repair loop gets a precise error without
//! a wasted round-trip.
//!
//! Deliberately conservative: it only reports an issue when it is *sure* the
//! query is wrong. Anything it can't fully resolve — parse failures, derived
//! tables, CTE columns, exotic constructs — silently passes and is left to the
//! database itself. A parser gap must never block a working query.

use std::collections::{HashMap, HashSet};
use std::ops::ControlFlow;

use sqlparser::ast::{
    Expr, Query, SelectItem, SetExpr, Statement, TableFactor, Visit, Visitor,
};
use sqlparser::dialect::{Dialect, GenericDialect, MySqlDialect, PostgreSqlDialect, SQLiteDialect};
use sqlparser::parser::Parser;

/// Map a prompt-dialect name ("SQLite", "PostgreSQL", "MySQL") to a parser dialect.
fn parser_dialect(name: &str) -> Box<dyn Dialect> {
    match name {
        "PostgreSQL" => Box::new(PostgreSqlDialect {}),
        "MySQL" => Box::new(MySqlDialect {}),
        "SQLite" => Box::new(SQLiteDialect {}),
        _ => Box::new(GenericDialect {}),
    }
}

/// Take the last dot-segment of a (possibly qualified) name and strip quoting,
/// e.g. `"main"."Orders"` → `orders`. String-based so it survives sqlparser's
/// `ObjectName` representation changes between versions.
fn last_segment_lower(name: &str) -> String {
    name.rsplit('.')
        .next()
        .unwrap_or(name)
        .trim_matches(|c| c == '"' || c == '`' || c == '\'' || c == '[' || c == ']')
        .to_lowercase()
}

/// What the database actually contains: table → columns (all lowercased).
pub struct SchemaIndex {
    tables: HashMap<String, Vec<String>>,
    /// False when some DDL failed to parse — then the table list may be missing
    /// entries, so unknown-table reporting is disabled (column checks on tables
    /// we *did* parse stay on).
    complete: bool,
}

impl SchemaIndex {
    /// An index that knows nothing — every validation is a no-op.
    pub fn empty() -> Self {
        Self { tables: HashMap::new(), complete: false }
    }

    pub fn is_empty(&self) -> bool {
        self.tables.is_empty()
    }

    /// Build from DDL strings (one `CREATE TABLE …` per table, as returned by
    /// `SqlRunner::introspect_schema`). Unparseable DDL degrades gracefully.
    pub fn from_ddl(ddl_list: &[String], dialect: &str) -> Self {
        let d = parser_dialect(dialect);
        let generic = GenericDialect {};
        let mut tables = HashMap::new();
        let mut complete = true;

        for ddl in ddl_list {
            let parsed = Parser::parse_sql(d.as_ref(), ddl)
                .or_else(|_| Parser::parse_sql(&generic, ddl));
            let Ok(statements) = parsed else {
                complete = false;
                continue;
            };
            let mut found = false;
            for stmt in statements {
                if let Statement::CreateTable(ct) = stmt {
                    let name = last_segment_lower(&ct.name.to_string());
                    let cols = ct
                        .columns
                        .iter()
                        .map(|c| c.name.value.to_lowercase())
                        .collect::<Vec<_>>();
                    tables.insert(name, cols);
                    found = true;
                }
            }
            if !found {
                complete = false; // DDL string that wasn't a CREATE TABLE we understood
            }
        }
        Self { tables, complete }
    }

    /// Validate a query against the schema. Returns human-readable issues
    /// (empty = nothing provably wrong). See module docs for the philosophy.
    pub fn validate(&self, sql: &str, dialect: &str) -> Vec<String> {
        if self.tables.is_empty() {
            return Vec::new();
        }
        let d = parser_dialect(dialect);
        let Ok(statements) = Parser::parse_sql(d.as_ref(), sql) else {
            return Vec::new(); // parser gap → let the database judge
        };

        let mut c = Collector::default();
        for stmt in &statements {
            let _ = stmt.visit(&mut c);
        }

        let mut issues = Vec::new();

        // Resolve table references into a scope: alias-or-name → (table, columns).
        let mut scope: HashMap<String, (String, &Vec<String>)> = HashMap::new();
        let mut wildcard: HashSet<String> = c.wildcard_aliases.clone();
        for (name, alias) in &c.tables {
            if c.ctes.contains(name) {
                // Reference to a CTE — its columns are opaque to us.
                wildcard.insert(alias.clone().unwrap_or_else(|| name.clone()));
                continue;
            }
            match self.tables.get_key_value(name) {
                Some((table, cols)) => {
                    let key = alias.clone().unwrap_or_else(|| name.clone());
                    // Same alias bound to different tables in different scopes —
                    // we can't tell which one a qualified column means: skip it.
                    if let Some((existing, _)) = scope.get(&key) {
                        if existing != table {
                            scope.remove(&key);
                            wildcard.insert(key);
                            continue;
                        }
                    }
                    scope.insert(key, (table.clone(), cols));
                }
                None => {
                    if self.complete {
                        issues.push(format!(
                            "unknown table '{name}'{}",
                            suggest(name, self.tables.keys())
                        ));
                    }
                }
            }
        }

        // Qualified columns (alias.column) — checkable whenever the alias resolves.
        for (qualifier, column) in &c.qualified_columns {
            if wildcard.contains(qualifier) || c.ctes.contains(qualifier) {
                continue;
            }
            if let Some((table, cols)) = scope.get(qualifier) {
                if !cols.contains(column) {
                    issues.push(format!(
                        "table '{table}' has no column '{column}'{}",
                        suggest(column, cols.iter())
                    ));
                }
            }
            // Unknown qualifier (schema-qualified, exotic) → skip.
        }

        // Bare columns — only checkable when the whole scope is known real tables.
        if !c.has_opaque_scope && wildcard.is_empty() && !scope.is_empty() {
            let known: HashSet<&String> =
                scope.values().flat_map(|(_, cols)| cols.iter()).collect();
            for column in &c.bare_columns {
                if c.select_aliases.contains(column) || known.contains(column) {
                    continue;
                }
                let all_cols: Vec<&String> = known.iter().copied().collect();
                issues.push(format!(
                    "no table in this query has a column named '{column}'{}",
                    suggest(column, all_cols.into_iter())
                ));
            }
        }

        issues.sort();
        issues.dedup();
        issues
    }
}

/// AST-level read-only check: `Some(true)` iff every parsed statement is a plain
/// query. `None` when parsing fails (caller falls back to the regex gate).
pub fn is_read_only_ast(sql: &str) -> Option<bool> {
    let statements = Parser::parse_sql(&GenericDialect {}, sql).ok()?;
    if statements.is_empty() {
        return None;
    }
    Some(statements.iter().all(|s| matches!(s, Statement::Query(_))))
}

/// `" (did you mean 'x'?)"` when a close match exists, else `""`.
fn suggest<'a>(input: &str, candidates: impl Iterator<Item = &'a String>) -> String {
    let mut best: Option<(usize, &String)> = None;
    for cand in candidates {
        let dist = edit_distance(input, cand);
        if best.is_none_or(|(d, _)| dist < d) {
            best = Some((dist, cand));
        }
    }
    match best {
        // Only suggest genuinely close names (≤2 edits and not most of the word).
        Some((d, cand)) if d > 0 && d <= 2 && d * 3 <= cand.len() => {
            format!(" (did you mean '{cand}'?)")
        }
        _ => String::new(),
    }
}

/// Classic Levenshtein distance, O(len_a × len_b) — names are short.
fn edit_distance(a: &str, b: &str) -> usize {
    let a: Vec<char> = a.chars().collect();
    let b: Vec<char> = b.chars().collect();
    let mut prev: Vec<usize> = (0..=b.len()).collect();
    for (i, ca) in a.iter().enumerate() {
        let mut cur = vec![i + 1];
        for (j, cb) in b.iter().enumerate() {
            let cost = if ca == cb { 0 } else { 1 };
            cur.push((prev[j] + cost).min(prev[j + 1] + 1).min(cur[j] + 1));
        }
        prev = cur;
    }
    prev[b.len()]
}

/// Walks the AST collecting everything `validate` needs. Resolution happens
/// after the walk, so visit order doesn't matter.
#[derive(Default)]
struct Collector {
    /// (table name, alias) for every real-table reference.
    tables: Vec<(String, Option<String>)>,
    /// CTE names — references to them look like table refs but aren't.
    ctes: HashSet<String>,
    /// Aliases of derived tables — their columns are opaque.
    wildcard_aliases: HashSet<String>,
    /// True when the query contains scopes we can't enumerate (derived tables,
    /// table functions, …) — disables bare-column checking.
    has_opaque_scope: bool,
    /// `SELECT expr AS alias` names — legal bare identifiers in ORDER BY etc.
    select_aliases: HashSet<String>,
    bare_columns: HashSet<String>,
    qualified_columns: HashSet<(String, String)>,
}

impl Visitor for Collector {
    type Break = ();

    fn pre_visit_query(&mut self, query: &Query) -> ControlFlow<Self::Break> {
        if let Some(with) = &query.with {
            for cte in &with.cte_tables {
                self.ctes.insert(cte.alias.name.value.to_lowercase());
            }
        }
        if let SetExpr::Select(select) = query.body.as_ref() {
            for item in &select.projection {
                if let SelectItem::ExprWithAlias { alias, .. } = item {
                    self.select_aliases.insert(alias.value.to_lowercase());
                }
            }
        }
        ControlFlow::Continue(())
    }

    fn pre_visit_table_factor(&mut self, tf: &TableFactor) -> ControlFlow<Self::Break> {
        match tf {
            TableFactor::Table { name, alias, .. } => {
                self.tables.push((
                    last_segment_lower(&name.to_string()),
                    alias.as_ref().map(|a| a.name.value.to_lowercase()),
                ));
            }
            TableFactor::Derived { alias, .. } => {
                if let Some(a) = alias {
                    self.wildcard_aliases.insert(a.name.value.to_lowercase());
                }
                self.has_opaque_scope = true;
            }
            _ => self.has_opaque_scope = true,
        }
        ControlFlow::Continue(())
    }

    fn pre_visit_expr(&mut self, expr: &Expr) -> ControlFlow<Self::Break> {
        match expr {
            Expr::Identifier(id) => {
                self.bare_columns.insert(id.value.to_lowercase());
            }
            Expr::CompoundIdentifier(parts) if parts.len() == 2 => {
                self.qualified_columns
                    .insert((parts[0].value.to_lowercase(), parts[1].value.to_lowercase()));
            }
            _ => {}
        }
        ControlFlow::Continue(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn demo_index() -> SchemaIndex {
        SchemaIndex::from_ddl(
            &[
                "CREATE TABLE customers (id INTEGER PRIMARY KEY, name TEXT, country TEXT, city TEXT, signup_date TEXT);".into(),
                "CREATE TABLE orders (id INTEGER PRIMARY KEY, customer_id INTEGER, order_date TEXT, status TEXT);".into(),
                "CREATE TABLE products (id INTEGER PRIMARY KEY, name TEXT, category TEXT, price REAL);".into(),
            ],
            "SQLite",
        )
    }

    #[test]
    fn builds_index_from_ddl() {
        let idx = demo_index();
        assert!(!idx.is_empty());
        assert!(idx.complete);
        assert_eq!(idx.tables["orders"].len(), 4);
    }

    #[test]
    fn valid_queries_pass() {
        let idx = demo_index();
        for sql in [
            "SELECT * FROM orders",
            "SELECT o.status, COUNT(*) AS n FROM orders o GROUP BY o.status ORDER BY n DESC",
            "SELECT c.country FROM orders o JOIN customers c ON c.id = o.customer_id",
            "SELECT name, price FROM products WHERE category = 'Books'",
            // select alias used in ORDER BY is not a column — must not be flagged
            "SELECT country, COUNT(*) AS cnt FROM customers GROUP BY country ORDER BY cnt",
        ] {
            assert_eq!(idx.validate(sql, "SQLite"), Vec::<String>::new(), "false positive on: {sql}");
        }
    }

    #[test]
    fn unknown_table_is_flagged_with_suggestion() {
        let idx = demo_index();
        let issues = idx.validate("SELECT * FROM orderz", "SQLite");
        assert_eq!(issues.len(), 1);
        assert!(issues[0].contains("unknown table 'orderz'"));
        assert!(issues[0].contains("did you mean 'orders'"));
    }

    #[test]
    fn unknown_qualified_column_is_flagged() {
        let idx = demo_index();
        let issues = idx.validate("SELECT o.total FROM orders o", "SQLite");
        assert_eq!(issues.len(), 1);
        assert!(issues[0].contains("table 'orders' has no column 'total'"), "{issues:?}");
    }

    #[test]
    fn unknown_bare_column_is_flagged() {
        let idx = demo_index();
        let issues = idx.validate("SELECT statuss FROM orders", "SQLite");
        assert_eq!(issues.len(), 1);
        assert!(issues[0].contains("'statuss'"));
        assert!(issues[0].contains("did you mean 'status'"));
    }

    #[test]
    fn cte_references_are_not_unknown_tables() {
        let idx = demo_index();
        let sql = "WITH totals AS (SELECT customer_id, COUNT(*) AS n FROM orders GROUP BY customer_id)
                   SELECT * FROM totals WHERE n > 3";
        assert_eq!(idx.validate(sql, "SQLite"), Vec::<String>::new());
    }

    #[test]
    fn derived_tables_disable_bare_column_checks() {
        let idx = demo_index();
        // `total` only exists inside the derived table — must not be flagged.
        let sql = "SELECT AVG(total) FROM (SELECT COUNT(*) AS total FROM orders GROUP BY customer_id) t";
        assert_eq!(idx.validate(sql, "SQLite"), Vec::<String>::new());
    }

    #[test]
    fn parse_failures_never_block() {
        let idx = demo_index();
        assert_eq!(idx.validate("SELECT FROM WHERE !!", "SQLite"), Vec::<String>::new());
    }

    #[test]
    fn empty_index_is_a_noop() {
        let idx = SchemaIndex::empty();
        assert_eq!(idx.validate("SELECT * FROM anything", "SQLite"), Vec::<String>::new());
    }

    #[test]
    fn ast_read_only_check() {
        assert_eq!(is_read_only_ast("SELECT 1"), Some(true));
        assert_eq!(is_read_only_ast("DROP TABLE x"), Some(false));
        // Multi-statement smuggling is caught at the AST level.
        assert_eq!(is_read_only_ast("SELECT 1; DROP TABLE x"), Some(false));
    }
}

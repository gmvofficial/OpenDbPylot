//! `VisualizeDataTool` — turns the results of a previous `run_sql` call into a
//! Plotly chart spec. It reads the structured result file that `run_sql` stashed
//! (by filename), so large datasets never travel through the LLM token stream.
//!
//! The charting heuristic is the one previously hard-coded in the server's
//! `chart_spec`: first column = labels, first all-numeric column = bar values.

use std::sync::Arc;

use anyhow::Result;
use async_trait::async_trait;
use serde_json::{json, Value};

use crate::capabilities::file_system::FileSystem;
use crate::core::tool::{Tool, ToolContext, ToolResult};

pub struct VisualizeDataTool {
    fs: Arc<dyn FileSystem>,
}

impl VisualizeDataTool {
    pub fn new(fs: Arc<dyn FileSystem>) -> Self {
        Self { fs }
    }
}

// OpenDbPylot brand colors.
const TEAL: &str = "#15a8a8";
const ORANGE: &str = "#fe5d26";
const MAGENTA: &str = "#bf1363";

/// Is every cell in column `j` a number?
fn col_is_numeric(rows: &[Vec<String>], j: usize) -> bool {
    !rows.is_empty() && rows.iter().all(|r| r.get(j).is_some_and(|c| c.trim().parse::<f64>().is_ok()))
}

/// Does column `j` look like a date/time axis (by name or `YYYY-MM[-DD]` values)?
fn col_is_temporal(name: &str, rows: &[Vec<String>], j: usize) -> bool {
    let n = name.to_lowercase();
    if ["date", "month", "year", "day", "time", "week", "quarter", "period"]
        .iter()
        .any(|k| n.contains(k))
    {
        return true;
    }
    rows.iter().take(6).all(|r| r.get(j).is_some_and(|c| looks_like_year_month(c)))
}

fn looks_like_year_month(s: &str) -> bool {
    let b = s.as_bytes();
    b.len() >= 7 && b[4] == b'-' && s[..4].bytes().all(|c| c.is_ascii_digit())
}

/// Pick an appropriate Plotly chart for the data's shape. Returns `(kind, spec)`
/// or `None` if nothing sensible applies. Heuristic set:
/// histogram (1 numeric), scatter (2 numeric), line (temporal + numeric), bar.
fn build_chart(columns: &[String], rows: &[Vec<String>], title: &str) -> Option<(&'static str, Value)> {
    if columns.is_empty() || rows.len() < 2 {
        return None;
    }
    let ncols = columns.len();
    let numeric: Vec<usize> = (0..ncols).filter(|&j| col_is_numeric(rows, j)).collect();
    let t = |fallback: &str| if title.is_empty() { fallback.to_string() } else { title.to_string() };

    // Single numeric column → histogram (distribution).
    if ncols == 1 && numeric == [0] {
        let xs: Vec<f64> = rows.iter().map(|r| r[0].trim().parse().unwrap_or(0.0)).collect();
        return Some((
            "histogram",
            json!({
                "data": [{ "type": "histogram", "x": xs, "marker": { "color": TEAL } }],
                "layout": { "title": t(&columns[0]), "xaxis": { "title": columns[0] }, "yaxis": { "title": "Count" } }
            }),
        ));
    }

    // Exactly two numeric columns → scatter (relationship).
    if ncols == 2 && numeric.len() == 2 {
        let xs: Vec<f64> = rows.iter().map(|r| r[0].trim().parse().unwrap_or(0.0)).collect();
        let ys: Vec<f64> = rows.iter().map(|r| r[1].trim().parse().unwrap_or(0.0)).collect();
        return Some((
            "scatter",
            json!({
                "data": [{ "type": "scatter", "mode": "markers", "x": xs, "y": ys, "marker": { "color": MAGENTA } }],
                "layout": { "title": t("Scatter"), "xaxis": { "title": columns[0] }, "yaxis": { "title": columns[1] } }
            }),
        ));
    }

    // Label column (first) + a numeric measure → line if temporal, else bar.
    let value_col = numeric.iter().copied().find(|&j| j != 0)?;
    let labels: Vec<String> = rows.iter().map(|r| r.first().cloned().unwrap_or_default()).collect();
    let values: Vec<f64> = rows.iter().map(|r| r.get(value_col).and_then(|c| c.trim().parse().ok()).unwrap_or(0.0)).collect();
    let axes = json!({ "title": t(&columns[value_col]), "xaxis": { "title": columns[0] }, "yaxis": { "title": columns[value_col] } });

    if col_is_temporal(&columns[0], rows, 0) {
        Some((
            "line",
            json!({
                "data": [{ "type": "scatter", "mode": "lines+markers", "x": labels, "y": values, "line": { "color": TEAL } }],
                "layout": axes
            }),
        ))
    } else {
        Some((
            "bar",
            json!({
                "data": [{ "type": "bar", "x": labels, "y": values, "marker": { "color": ORANGE } }],
                "layout": axes
            }),
        ))
    }
}

#[async_trait]
impl Tool for VisualizeDataTool {
    fn name(&self) -> &str {
        "visualize_data"
    }

    fn description(&self) -> &str {
        "Create a chart from the results of a previous run_sql call. Pass the \
         filename that run_sql returned."
    }

    fn args_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "filename": {
                    "type": "string",
                    "description": "The query_results_*.json filename returned by run_sql."
                },
                "title": {
                    "type": "string",
                    "description": "Optional chart title."
                }
            },
            "required": ["filename"]
        })
    }

    async fn execute(&self, _ctx: &ToolContext, args: Value) -> Result<ToolResult> {
        let filename = args["filename"].as_str().unwrap_or("");
        if filename.is_empty() {
            return Ok(ToolResult::error("No filename provided."));
        }

        let content = match self.fs.read_file(filename).await {
            Ok(c) => c,
            Err(e) => return Ok(ToolResult::error(format!("Could not read {filename}: {e}"))),
        };

        let data: Value = serde_json::from_str(&content).unwrap_or_else(|_| json!({}));
        let columns: Vec<String> = serde_json::from_value(data["columns"].clone()).unwrap_or_default();
        let rows: Vec<Vec<String>> = serde_json::from_value(data["rows"].clone()).unwrap_or_default();
        let title = args["title"].as_str().unwrap_or("");

        match build_chart(&columns, &rows, title) {
            Some((kind, spec)) => {
                let ui = json!({ "kind": "chart", "spec": spec });
                Ok(ToolResult::ok(format!("Created a {kind} chart from the query results.")).with_ui(ui))
            }
            None => Ok(ToolResult::error(
                "The data isn't suitable for a chart (need at least 2 rows and a numeric column).",
            )),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::capabilities::file_system::MemoryFileSystem;

    #[tokio::test]
    async fn charts_a_stashed_result_file() {
        let fs: Arc<dyn FileSystem> = Arc::new(MemoryFileSystem::new());
        fs.write_file(
            "query_results_abc.json",
            &json!({ "columns": ["country", "n"], "rows": [["USA", "2"], ["UK", "1"]] }).to_string(),
        )
        .await
        .unwrap();

        let tool = VisualizeDataTool::new(fs);
        let res = tool
            .execute(&ToolContext::default(), json!({ "filename": "query_results_abc.json" }))
            .await
            .unwrap();

        assert!(res.success);
        let ui = res.ui.expect("expected a chart ui payload");
        assert_eq!(ui["kind"], "chart");
        // country (categorical) + n (numeric) → bar chart.
        assert_eq!(ui["spec"]["data"][0]["type"], "bar");
        assert_eq!(ui["spec"]["data"][0]["x"][0], "USA");
        assert_eq!(ui["spec"]["data"][0]["y"][0], 2.0);
    }

    #[tokio::test]
    async fn monthly_data_becomes_a_line_chart() {
        let fs: Arc<dyn FileSystem> = Arc::new(MemoryFileSystem::new());
        fs.write_file(
            "query_results_ts.json",
            &json!({ "columns": ["month", "revenue"],
                     "rows": [["2024-01", "100"], ["2024-02", "150"], ["2024-03", "120"]] })
            .to_string(),
        )
        .await
        .unwrap();

        let tool = VisualizeDataTool::new(fs);
        let res = tool
            .execute(&ToolContext::default(), json!({ "filename": "query_results_ts.json" }))
            .await
            .unwrap();

        assert!(res.success);
        let ui = res.ui.unwrap();
        // temporal first column → line (scatter + lines mode).
        assert_eq!(ui["spec"]["data"][0]["type"], "scatter");
        assert_eq!(ui["spec"]["data"][0]["mode"], "lines+markers");
    }

    #[tokio::test]
    async fn missing_file_is_reported() {
        let fs: Arc<dyn FileSystem> = Arc::new(MemoryFileSystem::new());
        let tool = VisualizeDataTool::new(fs);
        let res = tool
            .execute(&ToolContext::default(), json!({ "filename": "nope.json" }))
            .await
            .unwrap();
        assert!(!res.success);
    }
}

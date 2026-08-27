use std::collections::HashSet;
use std::io::IsTerminal;

use anyhow::Result;
use console::style;
use serde_json::json;
use serde_json::{Map, Value};

use crate::client::ApiClient;
use crate::org;

#[derive(Default)]
struct QuerySummary {
    rows: Option<u64>,
    elapsed_seconds: Option<f64>,
    rows_read: Option<u64>,
    bytes_read: Option<u64>,
    hints: Vec<String>,
}

pub fn query(
    client: &ApiClient,
    database: &str,
    organization: Option<&str>,
    cluster: Option<&str>,
    sql: &str,
    limit: Option<u64>,
    json_mode: bool,
) -> Result<()> {
    let sql = match limit {
        Some(n) => format!("{} LIMIT {}", sql.trim().trim_end_matches(';'), n),
        None => sql.to_string(),
    };

    let body = json!({ "sql": sql });

    let path = org::database_scoped_path(database, "/query", organization, cluster);
    let raw = client.post_raw(&path, &body)?;

    if let Ok(value) = serde_json::from_str::<Value>(&raw) {
        if json_mode {
            println!("{}", serde_json::to_string_pretty(&value)?);
        } else if !print_json_as_table(&value) {
            println!("{}", serde_json::to_string_pretty(&value)?);
        }
    } else {
        print!("{}", raw);
    }

    Ok(())
}

fn print_json_as_table(value: &Value) -> bool {
    let Some((columns, rows, summary)) = extract_rows_and_columns(value) else {
        return false;
    };
    let displayed_rows = rows.len();
    let displayed_columns = columns.len();
    let colorize_muted = std::io::stdout().is_terminal();

    if columns.is_empty() {
        println!("No rows returned.");
        print_query_summary(&summary, displayed_rows, displayed_columns, colorize_muted);
        return true;
    }

    let mut rendered_rows = Vec::with_capacity(rows.len());
    for row in rows {
        let cells = columns
            .iter()
            .map(|name| format_cell_value(row.get(name)))
            .collect::<Vec<_>>();
        rendered_rows.push(cells);
    }

    println!();
    println!(
        "{}",
        render_clickhouse_table(&columns, &rendered_rows, colorize_muted)
    );
    print_query_summary(&summary, displayed_rows, displayed_columns, colorize_muted);
    true
}

fn render_clickhouse_table(
    columns: &[String],
    rows: &[Vec<String>],
    colorize_muted: bool,
) -> String {
    let mut widths = columns
        .iter()
        .map(|col| col.chars().count())
        .collect::<Vec<_>>();
    for row in rows {
        for (idx, cell) in row.iter().enumerate() {
            widths[idx] = widths[idx].max(cell.chars().count());
        }
    }

    let row_num_width = rows.len().max(1).to_string().len();
    let gutter = " ".repeat(row_num_width + 2);
    let mut lines = Vec::with_capacity(rows.len() + 2);

    let top = columns
        .iter()
        .enumerate()
        .map(|(idx, col)| {
            let col_width = widths[idx];
            let header_width = col.chars().count();
            let trailing = col_width + 1 - header_width;
            format!("─{}{}", col, "─".repeat(trailing))
        })
        .collect::<Vec<_>>()
        .join("┬");
    lines.push(format!("{gutter}┌{top}┐"));

    for (idx, row) in rows.iter().enumerate() {
        let row_prefix_plain = format!("{:>width$}. ", idx + 1, width = row_num_width);
        let row_prefix = if colorize_muted {
            style(row_prefix_plain).color256(245).to_string()
        } else {
            row_prefix_plain
        };
        let body = row
            .iter()
            .enumerate()
            .map(|(col_idx, cell)| {
                let pad = widths[col_idx].saturating_sub(cell.chars().count());
                format!(" {cell}{} ", " ".repeat(pad))
            })
            .collect::<Vec<_>>()
            .join("│");
        lines.push(format!("{row_prefix}│{body}│"));
    }

    let bottom = widths
        .iter()
        .map(|width| "─".repeat(width + 2))
        .collect::<Vec<_>>()
        .join("┴");
    lines.push(format!("{gutter}└{bottom}┘"));

    lines.join("\n")
}

fn print_query_summary(
    summary: &QuerySummary,
    displayed_rows: usize,
    columns_count: usize,
    colorize_muted: bool,
) {
    let mut printed_footer = false;
    if let Some(footer) = format_query_footer(summary, displayed_rows, columns_count) {
        println!();
        if colorize_muted {
            println!("{}", style(footer).color256(245));
        } else {
            println!("{footer}");
        }
        printed_footer = true;
    }

    if !summary.hints.is_empty() {
        if !printed_footer {
            println!();
        }
        print_query_hints(&summary.hints, colorize_muted);
    }
}

fn print_query_hints(hints: &[String], colorize_warning: bool) {
    println!();
    if colorize_warning {
        println!("{}", style("Hints:").yellow());
    } else {
        println!("Hints:");
    }

    for hint in hints {
        let line = format!("- {hint}");
        if colorize_warning {
            println!("{}", style(line).yellow());
        } else {
            println!("{line}");
        }
    }
}

fn format_query_footer(
    summary: &QuerySummary,
    displayed_rows: usize,
    columns_count: usize,
) -> Option<String> {
    let rows_in_set = summary
        .rows
        .or(summary.rows_read)
        .or(Some(displayed_rows as u64))?;
    let bytes_processed = summary.bytes_read.unwrap_or(0);
    let elapsed_ms = summary.elapsed_seconds.unwrap_or(0.0) * 1000.0;

    Some(format!(
        "{} processed, {} rows x {} columns ({})",
        format_bytes_compact(bytes_processed),
        format_count_compact(rows_in_set),
        format_count_compact(columns_count as u64),
        format_duration_compact(elapsed_ms)
    ))
}

fn extract_rows_and_columns<'a>(
    value: &'a Value,
) -> Option<(Vec<String>, Vec<&'a Map<String, Value>>, QuerySummary)> {
    let (rows, mut columns, summary) = match value {
        Value::Object(obj) => {
            let rows = obj.get("data")?.as_array()?;
            let columns = obj
                .get("meta")
                .and_then(Value::as_array)
                .map(|meta| {
                    meta.iter()
                        .filter_map(|entry| entry.get("name").and_then(Value::as_str))
                        .map(str::to_owned)
                        .collect::<Vec<_>>()
                })
                .unwrap_or_default();
            let statistics = obj.get("statistics").and_then(Value::as_object);
            let summary = QuerySummary {
                rows: parse_u64(obj.get("rows")),
                elapsed_seconds: parse_f64(statistics.and_then(|stats| stats.get("elapsed"))),
                rows_read: parse_u64(statistics.and_then(|stats| stats.get("rows_read"))),
                bytes_read: parse_u64(statistics.and_then(|stats| stats.get("bytes_read"))),
                hints: extract_hints(obj.get("hints")),
            };
            (rows, columns, summary)
        }
        Value::Array(rows) => (rows, Vec::new(), QuerySummary::default()),
        _ => return None,
    };

    let mut row_objects = Vec::with_capacity(rows.len());
    for row in rows {
        let obj = row.as_object()?;
        row_objects.push(obj);
    }

    let mut seen = columns.iter().cloned().collect::<HashSet<_>>();
    for row in &row_objects {
        for key in row.keys() {
            if seen.insert(key.clone()) {
                columns.push(key.clone());
            }
        }
    }

    Some((columns, row_objects, summary))
}

fn format_cell_value(value: Option<&Value>) -> String {
    match value {
        Some(Value::Null) | None => "null".to_string(),
        Some(Value::Bool(v)) => v.to_string(),
        Some(Value::Number(v)) => v.to_string(),
        Some(Value::String(v)) => v.clone(),
        Some(other) => other.to_string(),
    }
}

fn parse_u64(value: Option<&Value>) -> Option<u64> {
    match value {
        Some(Value::Number(n)) => n.as_u64(),
        Some(Value::String(s)) => s.parse::<u64>().ok(),
        _ => None,
    }
}

fn parse_f64(value: Option<&Value>) -> Option<f64> {
    match value {
        Some(Value::Number(n)) => n.as_f64(),
        Some(Value::String(s)) => s.parse::<f64>().ok(),
        _ => None,
    }
}

fn extract_hints(value: Option<&Value>) -> Vec<String> {
    match value {
        Some(Value::String(s)) => vec![s.clone()],
        Some(Value::Array(items)) => items
            .iter()
            .filter_map(hint_value_to_string)
            .filter(|s| !s.trim().is_empty())
            .collect(),
        Some(other) => hint_value_to_string(other).into_iter().collect(),
        None => Vec::new(),
    }
}

fn hint_value_to_string(value: &Value) -> Option<String> {
    match value {
        Value::Null => None,
        Value::String(s) => Some(s.clone()),
        Value::Bool(v) => Some(v.to_string()),
        Value::Number(v) => Some(v.to_string()),
        other => Some(other.to_string()),
    }
}

fn format_count_compact(value: u64) -> String {
    if value < 1_000 {
        return value.to_string();
    }

    const UNITS: [&str; 5] = ["", "k", "M", "B", "T"];
    let mut scaled = value as f64;
    let mut unit_index = 0usize;
    while scaled >= 1_000.0 && unit_index < UNITS.len() - 1 {
        scaled /= 1_000.0;
        unit_index += 1;
    }

    // Prevent outputs like `1000k` by promoting after rounding.
    let mut rounded = round_to_places(scaled, 1);
    while rounded >= 1_000.0 && unit_index < UNITS.len() - 1 {
        scaled /= 1_000.0;
        unit_index += 1;
        rounded = round_to_places(scaled, 1);
    }

    format!("{}{}", format_decimal_trim(rounded, 1), UNITS[unit_index])
}

fn format_bytes_compact(bytes: u64) -> String {
    const UNITS: [&str; 6] = ["B", "KB", "MB", "GB", "TB", "PB"];
    if bytes < 1_000 {
        return format!("{bytes} B");
    }

    let mut size = bytes as f64;
    let mut unit_index = 0usize;
    while size >= 1_000.0 && unit_index < UNITS.len() - 1 {
        size /= 1_000.0;
        unit_index += 1;
    }

    // Prevent outputs like `1000 KB` by promoting after rounding.
    let mut rounded = round_to_places(size, 1);
    while rounded >= 1_000.0 && unit_index < UNITS.len() - 1 {
        size /= 1_000.0;
        unit_index += 1;
        rounded = round_to_places(size, 1);
    }

    format!("{} {}", format_decimal_trim(rounded, 1), UNITS[unit_index])
}

fn format_duration_compact(elapsed_ms: f64) -> String {
    if elapsed_ms >= 100.0 {
        format!("{:.2}s", elapsed_ms / 1_000.0)
    } else {
        format!("{elapsed_ms:.2}ms")
    }
}

fn format_decimal_trim(value: f64, places: usize) -> String {
    let mut s = format!("{value:.places$}");
    if let Some(dot_idx) = s.find('.') {
        while s.ends_with('0') {
            s.pop();
        }
        if s.len() == dot_idx + 1 {
            s.pop();
        }
    }
    s
}

fn round_to_places(value: f64, places: usize) -> f64 {
    let factor = 10_f64.powi(places as i32);
    (value * factor).round() / factor
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::{
        extract_rows_and_columns, format_query_footer, render_clickhouse_table, QuerySummary,
    };

    #[test]
    fn extract_rows_uses_meta_order_and_appends_missing_keys() {
        let value = json!({
            "meta": [{"name": "id"}, {"name": "name"}],
            "data": [
                {"id": 1, "name": "alice"},
                {"id": 2, "name": "bob", "extra": true}
            ]
        });

        let (columns, rows, summary) = extract_rows_and_columns(&value).expect("tabular json");
        assert_eq!(columns, vec!["id", "name", "extra"]);
        assert_eq!(rows.len(), 2);
        assert!(summary.rows.is_none());
        assert!(summary.elapsed_seconds.is_none());
        assert!(summary.rows_read.is_none());
        assert!(summary.bytes_read.is_none());
        assert!(summary.hints.is_empty());
    }

    #[test]
    fn extract_rows_supports_top_level_array() {
        let value = json!([
            {"a": 1},
            {"a": 2, "b": 3}
        ]);

        let (columns, rows, summary) = extract_rows_and_columns(&value).expect("tabular json");
        assert_eq!(columns, vec!["a", "b"]);
        assert_eq!(rows.len(), 2);
        assert!(summary.rows.is_none());
        assert!(summary.elapsed_seconds.is_none());
        assert!(summary.rows_read.is_none());
        assert!(summary.bytes_read.is_none());
        assert!(summary.hints.is_empty());
    }

    #[test]
    fn extract_rows_includes_rows_and_statistics_summary() {
        let value = json!({
            "rows": 1,
            "statistics": {
                "bytes_read": 25,
                "elapsed": 0.000571999,
                "rows_read": 1
            },
            "data": [{"id": 1}]
        });

        let (_, _, summary) = extract_rows_and_columns(&value).expect("tabular json");
        assert_eq!(summary.rows, Some(1));
        assert_eq!(summary.bytes_read, Some(25));
        assert_eq!(summary.rows_read, Some(1));
        assert_eq!(summary.elapsed_seconds, Some(0.000571999));
        assert!(summary.hints.is_empty());
    }

    #[test]
    fn extract_rows_includes_hints_summary() {
        let value = json!({
            "rows": 1,
            "hints": ["slow query", "consider index"],
            "data": [{"id": 1}]
        });

        let (_, _, summary) = extract_rows_and_columns(&value).expect("tabular json");
        assert_eq!(
            summary.hints,
            vec!["slow query".to_string(), "consider index".to_string()]
        );
    }

    #[test]
    fn extract_rows_rejects_non_object_rows() {
        let value = json!({"data": [1, 2, 3]});
        assert!(extract_rows_and_columns(&value).is_none());
    }

    #[test]
    fn footer_includes_summary_stats_with_human_readable_values() {
        let summary = QuerySummary {
            rows: Some(5),
            elapsed_seconds: Some(0.0047),
            rows_read: Some(15420),
            bytes_read: Some(15360),
            hints: Vec::new(),
        };

        let footer = format_query_footer(&summary, 5, 20).expect("footer");
        assert_eq!(footer, "15.4 KB processed, 5 rows x 20 columns (4.70ms)");
    }

    #[test]
    fn footer_uses_displayed_row_count_when_rows_summary_missing() {
        let summary = QuerySummary::default();
        let footer = format_query_footer(&summary, 2, 3).expect("footer");
        assert_eq!(footer, "0 B processed, 2 rows x 3 columns (0.00ms)");
    }

    #[test]
    fn footer_uses_compact_format_for_large_counts_and_seconds() {
        let summary = QuerySummary {
            rows: Some(1_250_000),
            elapsed_seconds: Some(2.5),
            rows_read: Some(2_000_000),
            bytes_read: Some(3 * 1024 * 1024),
            hints: Vec::new(),
        };

        let footer = format_query_footer(&summary, 10, 20_000).expect("footer");
        assert_eq!(footer, "3.1 MB processed, 1.3M rows x 20k columns (2.50s)");
    }

    #[test]
    fn footer_uses_rows_read_when_rows_missing() {
        let summary = QuerySummary {
            rows: None,
            elapsed_seconds: Some(0.35),
            rows_read: Some(15_420),
            bytes_read: Some(25),
            hints: Vec::new(),
        };

        let footer = format_query_footer(&summary, 0, 2).expect("footer");
        assert_eq!(footer, "25 B processed, 15.4k rows x 2 columns (0.35s)");
    }

    #[test]
    fn footer_promotes_count_unit_when_rounding_crosses_boundary() {
        let summary = QuerySummary {
            rows: Some(999_950),
            elapsed_seconds: Some(0.35),
            rows_read: None,
            bytes_read: Some(25),
            hints: Vec::new(),
        };

        let footer = format_query_footer(&summary, 0, 2).expect("footer");
        assert_eq!(footer, "25 B processed, 1M rows x 2 columns (0.35s)");
    }

    #[test]
    fn footer_promotes_byte_unit_when_rounding_crosses_boundary() {
        let summary = QuerySummary {
            rows: Some(1),
            elapsed_seconds: Some(0.35),
            rows_read: None,
            bytes_read: Some(999_950),
            hints: Vec::new(),
        };

        let footer = format_query_footer(&summary, 0, 2).expect("footer");
        assert_eq!(footer, "1 MB processed, 1 rows x 2 columns (0.35s)");
    }

    #[test]
    fn clickhouse_table_renderer_formats_header_and_row_numbers() {
        let columns = vec!["user_id".to_string(), "email".to_string()];
        let rows = vec![
            vec![
                "9ec3b48a-1b3e-44b9-8442-88c01022e78d".to_string(),
                "a@example.com".to_string(),
            ],
            vec![
                "9abbcf75-c1d1-4246-b9d0-7dea86b67297".to_string(),
                "b@example.com".to_string(),
            ],
        ];

        let rendered = render_clickhouse_table(&columns, &rows, false);
        assert!(rendered.contains("┌─user_id"));
        assert!(rendered.contains("1. │"));
        assert!(rendered.contains("2. │"));
        assert!(rendered.contains("└"));
    }
}

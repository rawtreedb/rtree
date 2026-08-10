use anyhow::{bail, Result};
use chrono::Utc;
use serde::{Deserialize, Serialize};

use crate::client::ApiClient;
use crate::org;
use crate::output;

#[derive(Debug, Deserialize, Serialize)]
pub struct LogEntry {
    pub time: String,
    #[serde(rename = "type")]
    pub log_type: String,
    pub status: String,
    pub query: String,
    pub exception: String,
    pub rows: u64,
    pub duration_ms: u64,
    pub bytes: u64,
    pub tables: Vec<String>,
    pub projections: Vec<String>,
    pub hints: Vec<String>,
}

#[derive(Debug, Deserialize, Serialize)]
pub struct LogsResponse {
    pub logs: Vec<LogEntry>,
    pub has_more: bool,
    pub next_offset: Option<u64>,
}

fn parse_duration(s: &str) -> Result<chrono::Duration> {
    let s = s.trim();
    if s.len() < 2 {
        bail!(
            "invalid duration '{}'. Use a number followed by m, h, d, or w (e.g., 1h, 30m)",
            s
        );
    }

    let (num_str, unit) = match s.char_indices().next_back() {
        Some((i, _)) => s.split_at(i),
        None => bail!(
            "invalid duration '{}'. Use a number followed by m, h, d, or w (e.g., 1h, 30m)",
            s
        ),
    };
    let num: i64 = num_str.parse().map_err(|_| {
        anyhow::anyhow!(
            "invalid duration '{}'. Use a number followed by m, h, d, or w (e.g., 1h, 30m)",
            s
        )
    })?;

    if num <= 0 {
        bail!(
            "invalid duration '{}'. Duration must be a positive number (e.g., 1h, 30m)",
            s
        );
    }

    match unit {
        "m" => Ok(chrono::Duration::minutes(num)),
        "h" => Ok(chrono::Duration::hours(num)),
        "d" => Ok(chrono::Duration::days(num)),
        "w" => Ok(chrono::Duration::weeks(num)),
        _ => bail!(
            "invalid duration unit '{}'. Use m (minutes), h (hours), d (days), or w (weeks)",
            unit
        ),
    }
}

fn resolve_time_range(
    since: Option<&str>,
    until: Option<&str>,
    start_time: Option<&str>,
    end_time: Option<&str>,
) -> Result<(String, String)> {
    let now = Utc::now();

    // Absolute timestamps mode
    if start_time.is_some() || end_time.is_some() {
        let end = end_time
            .map(|s| s.to_string())
            .unwrap_or_else(|| now.format("%Y-%m-%dT%H:%M:%SZ").to_string());
        let start = start_time.map(|s| s.to_string()).unwrap_or_else(|| {
            // Default to 24h before end_time, not 24h before now
            chrono::DateTime::parse_from_rfc3339(&end)
                .map(|dt| {
                    (dt - chrono::Duration::hours(24))
                        .format("%Y-%m-%dT%H:%M:%SZ")
                        .to_string()
                })
                .unwrap_or_else(|_| {
                    (now - chrono::Duration::hours(24))
                        .format("%Y-%m-%dT%H:%M:%SZ")
                        .to_string()
                })
        });
        if start > end {
            bail!(
                "invalid time range: --start-time ({}) is after --end-time ({})",
                start,
                end
            );
        }
        return Ok((start, end));
    }

    // Relative duration mode (or defaults)
    let since_delta = match since {
        Some(s) => parse_duration(s)?,
        None => chrono::Duration::hours(24),
    };
    let until_delta = match until {
        Some(u) => parse_duration(u)?,
        None => chrono::Duration::zero(),
    };

    if since_delta < until_delta {
        bail!(
            "invalid time range: --since ({}) must be greater than --until ({})",
            since.unwrap_or("24h"),
            until.unwrap_or("0")
        );
    }

    let start = (now - since_delta).format("%Y-%m-%dT%H:%M:%SZ").to_string();
    let end = (now - until_delta).format("%Y-%m-%dT%H:%M:%SZ").to_string();

    Ok((start, end))
}

fn build_query_string(
    start_time: &str,
    end_time: &str,
    log_type: Option<&str>,
    tables: &[String],
    status: Option<&str>,
    limit: u64,
    offset: u64,
) -> String {
    let mut params = vec![
        format!("start_time={}", urlencoding::encode(start_time)),
        format!("end_time={}", urlencoding::encode(end_time)),
        format!("limit={}", limit),
        format!("offset={}", offset),
    ];

    let search = build_search_filter(log_type, tables, status);
    if let Some(search) = search {
        params.push(format!("search={}", urlencoding::encode(&search)));
    }

    params.join("&")
}

fn build_search_filter(
    log_type: Option<&str>,
    tables: &[String],
    status: Option<&str>,
) -> Option<String> {
    let mut filters = Vec::new();

    if let Some(log_type) = log_type.map(str::trim).filter(|value| !value.is_empty()) {
        filters.push(format!("type:{log_type}"));
    }
    if let Some(status) = status.map(str::trim).filter(|value| !value.is_empty()) {
        filters.push(format!("status:{status}"));
    }
    let tables: Vec<&str> = tables
        .iter()
        .map(|table| table.trim())
        .filter(|table| !table.is_empty())
        .collect();
    if !tables.is_empty() {
        filters.push(format!("table:{}", tables.join(",")));
    }

    (!filters.is_empty()).then(|| filters.join(" "))
}

fn truncate_query(query: &str, max_len: usize) -> String {
    let normalized: String = query.split_whitespace().collect::<Vec<_>>().join(" ");
    if normalized.len() <= max_len {
        normalized
    } else {
        let mut end = max_len - 3;
        while !normalized.is_char_boundary(end) {
            end -= 1;
        }
        format!("{}...", &normalized[..end])
    }
}

fn format_bytes(bytes: u64) -> String {
    const UNITS: [&str; 6] = ["B", "KB", "MB", "GB", "TB", "PB"];
    if bytes < 1000 {
        return format!("{bytes} B");
    }
    let mut size = bytes as f64;
    let mut unit_index = 0usize;
    while size >= 1000.0 && unit_index < UNITS.len() - 1 {
        size /= 1000.0;
        unit_index += 1;
    }
    format!("{size:.1} {}", UNITS[unit_index])
}

fn format_log_line(entry: &LogEntry) -> String {
    let time = if entry.time.len() >= 19 {
        &entry.time[..19]
    } else {
        &entry.time
    };
    let status_str = if entry.status.eq_ignore_ascii_case("OK") {
        "OK"
    } else {
        "ERROR"
    };
    let query = truncate_query(&entry.query, 80);
    let bytes = format_bytes(entry.bytes);

    let mut line = format!(
        "{}  {:<6}  {:<5}  {:>7}  {:>10}  {:>8}  {}",
        time,
        entry.log_type,
        status_str,
        format!("{}ms", entry.duration_ms),
        format!("{} rows", entry.rows),
        bytes,
        query
    );

    if !entry.exception.is_empty() {
        line.push_str(&format!("  [{}]", entry.exception));
    }

    line
}

fn fetch_logs(
    client: &ApiClient,
    database: &str,
    organization: Option<&str>,
    start_time: &str,
    end_time: &str,
    log_type: Option<&str>,
    tables: &[String],
    status: Option<&str>,
    limit: u64,
    offset: u64,
) -> Result<LogsResponse> {
    let query_string = build_query_string(
        start_time, end_time, log_type, tables, status, limit, offset,
    );
    let path =
        org::database_scoped_path(database, &format!("/logs?{}", query_string), organization);
    client.get(&path)
}

#[allow(clippy::too_many_arguments)]
pub fn logs(
    client: &ApiClient,
    database: &str,
    organization: Option<&str>,
    log_type: Option<&str>,
    tables: &[String],
    status: Option<&str>,
    limit: u64,
    offset: u64,
    since: Option<&str>,
    until: Option<&str>,
    start_time: Option<&str>,
    end_time: Option<&str>,
    json_mode: bool,
) -> Result<()> {
    let (resolved_start, resolved_end) = resolve_time_range(since, until, start_time, end_time)?;

    let resp = fetch_logs(
        client,
        database,
        organization,
        &resolved_start,
        &resolved_end,
        log_type,
        tables,
        status,
        limit,
        offset,
    )?;

    output::print_result(&resp, json_mode, |resp| {
        if resp.logs.is_empty() {
            println!("No logs found for the specified time range.");
        } else {
            for entry in &resp.logs {
                println!("{}", format_log_line(entry));
            }
            if resp.has_more {
                println!(
                    "\n... more logs available (use --offset {} to continue)",
                    resp.next_offset.unwrap_or(0)
                );
            }
        }
    });

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_duration_minutes() {
        let d = parse_duration("30m").unwrap();
        assert_eq!(d, chrono::Duration::minutes(30));
    }

    #[test]
    fn parse_duration_hours() {
        let d = parse_duration("1h").unwrap();
        assert_eq!(d, chrono::Duration::hours(1));
    }

    #[test]
    fn parse_duration_days() {
        let d = parse_duration("7d").unwrap();
        assert_eq!(d, chrono::Duration::days(7));
    }

    #[test]
    fn parse_duration_weeks() {
        let d = parse_duration("2w").unwrap();
        assert_eq!(d, chrono::Duration::weeks(2));
    }

    #[test]
    fn parse_duration_rejects_invalid_unit() {
        let err = parse_duration("5x").unwrap_err();
        assert!(format!("{err:#}").contains("invalid duration unit"));
    }

    #[test]
    fn parse_duration_rejects_empty() {
        let err = parse_duration("").unwrap_err();
        assert!(format!("{err:#}").contains("invalid duration"));
    }

    #[test]
    fn parse_duration_rejects_no_number() {
        let err = parse_duration("h").unwrap_err();
        assert!(format!("{err:#}").contains("invalid duration"));
    }

    #[test]
    fn truncate_query_short_query_unchanged() {
        let q = truncate_query("SELECT 1", 80);
        assert_eq!(q, "SELECT 1");
    }

    #[test]
    fn truncate_query_long_query_truncated() {
        let q = truncate_query(&"x".repeat(100), 80);
        assert_eq!(q.len(), 80);
        assert!(q.ends_with("..."));
    }

    #[test]
    fn truncate_query_normalizes_whitespace() {
        let q = truncate_query("SELECT\n  *\n  FROM\n  events", 80);
        assert_eq!(q, "SELECT * FROM events");
    }

    #[test]
    fn format_bytes_small() {
        assert_eq!(format_bytes(0), "0 B");
        assert_eq!(format_bytes(136), "136 B");
        assert_eq!(format_bytes(999), "999 B");
    }

    #[test]
    fn format_bytes_large() {
        assert_eq!(format_bytes(1000), "1.0 KB");
        assert_eq!(format_bytes(1000000), "1.0 MB");
    }

    #[test]
    fn build_query_string_minimal() {
        let qs = build_query_string(
            "2026-03-28 00:00:00",
            "2026-03-29 00:00:00",
            None,
            &[],
            None,
            50,
            0,
        );
        assert!(qs.contains("start_time="));
        assert!(qs.contains("end_time="));
        assert!(qs.contains("limit=50"));
        assert!(qs.contains("offset=0"));
        assert!(!qs.contains("type="));
        assert!(!qs.contains("search="));
    }

    #[test]
    fn build_query_string_with_filters() {
        let qs = build_query_string(
            "2026-03-28 00:00:00",
            "2026-03-29 00:00:00",
            Some("select"),
            &[String::from("events")],
            Some("error"),
            100,
            50,
        );
        assert!(qs.contains("search=type%3Aselect%20status%3Aerror%20table%3Aevents"));
        assert!(!qs.contains("&type=select"));
        assert!(!qs.contains("&table=events"));
        assert!(!qs.contains("&status=error"));
        assert!(qs.contains("limit=100"));
        assert!(qs.contains("offset=50"));
    }

    #[test]
    fn build_query_string_supports_multiple_tables() {
        let qs = build_query_string(
            "2026-03-28 00:00:00",
            "2026-03-29 00:00:00",
            Some("insert"),
            &[String::from("events"), String::from("audit")],
            None,
            50,
            0,
        );

        assert!(qs.contains("search=type%3Ainsert%20table%3Aevents%2Caudit"));
    }

    #[test]
    fn build_search_filter_ignores_empty_filters() {
        assert_eq!(
            build_search_filter(Some("select"), &[String::from(" ")], Some("")),
            Some("type:select".to_string())
        );
        assert_eq!(build_search_filter(None, &[], None), None);
    }

    #[test]
    fn resolve_time_range_defaults_to_24h() {
        let (start, end) = resolve_time_range(None, None, None, None).unwrap();
        // Both should be valid UTC timestamps ending with Z
        assert!(start.ends_with('Z'));
        assert!(end.ends_with('Z'));
        // Start should be before end
        assert!(start < end);
    }

    #[test]
    fn resolve_time_range_absolute() {
        let (start, end) = resolve_time_range(
            None,
            None,
            Some("2026-03-28 00:00:00"),
            Some("2026-03-29 00:00:00"),
        )
        .unwrap();
        assert_eq!(start, "2026-03-28 00:00:00");
        assert_eq!(end, "2026-03-29 00:00:00");
    }

    #[test]
    fn resolve_time_range_since_only() {
        let (start, end) = resolve_time_range(Some("1h"), None, None, None).unwrap();
        assert!(start.ends_with('Z'));
        assert!(end.ends_with('Z'));
        assert!(start < end);
    }

    #[test]
    fn format_log_line_success() {
        let entry = LogEntry {
            time: "2026-03-28 18:51:19.401393".to_string(),
            log_type: "select".to_string(),
            status: "OK".to_string(),
            query: "SELECT count() FROM events".to_string(),
            exception: String::new(),
            rows: 1,
            duration_ms: 3,
            bytes: 136,
            tables: vec!["events".to_string()],
            projections: vec![],
            hints: vec![],
        };
        let line = format_log_line(&entry);
        assert!(line.contains("2026-03-28 18:51:19"));
        assert!(line.contains("select"));
        assert!(line.contains("OK"));
        assert!(line.contains("3ms"));
        assert!(line.contains("1 rows"));
        assert!(line.contains("136 B"));
        assert!(line.contains("SELECT count() FROM events"));
        assert!(!line.contains("["));
    }

    #[test]
    fn format_log_line_error() {
        let entry = LogEntry {
            time: "2026-03-28 18:53:44.000000".to_string(),
            log_type: "select".to_string(),
            status: "ExceptionBeforeStart".to_string(),
            query: "SELECT * FROM nonexistent".to_string(),
            exception: "Table nonexistent doesn't exist".to_string(),
            rows: 0,
            duration_ms: 0,
            bytes: 0,
            tables: vec![],
            projections: vec![],
            hints: vec![],
        };
        let line = format_log_line(&entry);
        assert!(line.contains("ERROR"));
        assert!(line.contains("[Table nonexistent doesn't exist]"));
    }
}

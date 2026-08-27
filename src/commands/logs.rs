use anyhow::{bail, Result};
use chrono::Utc;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

use crate::client::ApiClient;
use crate::org;
use crate::output;

#[derive(Debug, Deserialize, Serialize)]
pub struct LogEntry {
    pub uuid: String,
    pub level: String,
    pub date: String,
    pub status: u16,
    pub pathname: String,
    pub latency: u64,
    pub headers: BTreeMap<String, String>,
    pub message: Option<String>,
    pub percentile: Option<f64>,
    pub id: String,
    pub time: String,
    pub method: String,
    pub host: String,
    pub path: String,
    pub route: String,
    pub status_code: u16,
    pub source: String,
    pub duration_ms: u64,
    pub user_agent: String,
    pub client_version: String,
    pub request_size_bytes: u64,
    pub response_size_bytes: u64,
    pub content_encoding: String,
    pub handler_ms: u64,
    pub app_overhead_ms: u64,
    pub error: String,
    pub error_code: String,
    pub error_message: String,
    pub error_hint: String,
    pub trace_id: String,
    pub span_id: String,
    pub parent_span_id: String,
    pub database: String,
    pub user_id: String,
    pub user_email: String,
    pub key_name: String,
    pub organization_id: String,
    pub organization_name: String,
    pub cluster_id: String,
    pub cluster_name: String,
    pub url_query: String,
    pub request_body: String,
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

    if start_time.is_some() || end_time.is_some() {
        let end = end_time
            .map(ToOwned::to_owned)
            .unwrap_or_else(|| now.format("%Y-%m-%dT%H:%M:%SZ").to_string());
        let start = start_time.map(ToOwned::to_owned).unwrap_or_else(|| {
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

fn append_param(params: &mut Vec<String>, name: &str, value: &str) {
    params.push(format!("{name}={}", urlencoding::encode(value)));
}

fn append_csv_param<T: ToString>(params: &mut Vec<String>, name: &str, values: &[T]) {
    if values.is_empty() {
        return;
    }
    let value = values
        .iter()
        .map(ToString::to_string)
        .collect::<Vec<_>>()
        .join(",");
    append_param(params, name, &value);
}

#[allow(clippy::too_many_arguments)]
fn build_query_string(
    start_time: &str,
    end_time: &str,
    search: Option<&str>,
    methods: &[String],
    status_codes: &[u16],
    levels: &[String],
    sources: &[String],
    user_agent: Option<&str>,
    host: Option<&str>,
    paths: &[String],
    min_duration_ms: Option<u64>,
    max_duration_ms: Option<u64>,
    log_databases: &[String],
    limit: u64,
    offset: u64,
) -> String {
    let mut params = Vec::new();
    append_param(&mut params, "start_time", start_time);
    append_param(&mut params, "end_time", end_time);
    if let Some(search) = search.map(str::trim).filter(|value| !value.is_empty()) {
        append_param(&mut params, "search", search);
    }
    append_csv_param(&mut params, "methods", methods);
    append_csv_param(&mut params, "status_codes", status_codes);
    append_csv_param(&mut params, "levels", levels);
    append_csv_param(&mut params, "sources", sources);
    if let Some(user_agent) = user_agent.map(str::trim).filter(|value| !value.is_empty()) {
        append_param(&mut params, "user_agent", user_agent);
    }
    if let Some(host) = host.map(str::trim).filter(|value| !value.is_empty()) {
        append_param(&mut params, "host", host);
    }
    append_csv_param(&mut params, "paths", paths);
    if let Some(min_duration_ms) = min_duration_ms {
        append_param(&mut params, "min_duration_ms", &min_duration_ms.to_string());
    }
    if let Some(max_duration_ms) = max_duration_ms {
        append_param(&mut params, "max_duration_ms", &max_duration_ms.to_string());
    }
    append_csv_param(&mut params, "log_databases", log_databases);
    append_param(&mut params, "limit", &limit.to_string());
    append_param(&mut params, "offset", &offset.to_string());
    params.join("&")
}

fn truncate_text(value: &str, max_len: usize) -> String {
    let normalized = value.split_whitespace().collect::<Vec<_>>().join(" ");
    if normalized.len() <= max_len {
        return normalized;
    }
    if max_len <= 3 {
        return normalized.chars().take(max_len).collect();
    }

    let mut end = max_len - 3;
    while !normalized.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}...", &normalized[..end])
}

fn format_log_line(entry: &LogEntry) -> String {
    let time = if entry.date.is_empty() {
        &entry.time
    } else {
        &entry.date
    };
    let path = if entry.pathname.is_empty() {
        &entry.path
    } else {
        &entry.pathname
    };
    let status = if entry.status_code == 0 {
        entry.status
    } else {
        entry.status_code
    };

    let mut line = format!(
        "{}  {:>3}  {:<6}  {:>8}  {:<4}  {}",
        time,
        status,
        entry.method,
        format!("{}ms", entry.duration_ms),
        entry.source,
        truncate_text(path, 100)
    );

    let error = if !entry.error_message.is_empty() {
        &entry.error_message
    } else if !entry.error.is_empty() {
        &entry.error
    } else {
        ""
    };
    if !error.is_empty() {
        line.push_str(&format!("  [{}]", truncate_text(error, 160)));
    }

    line
}

#[allow(clippy::too_many_arguments)]
fn fetch_logs(
    client: &ApiClient,
    database: &str,
    organization: Option<&str>,
    cluster: Option<&str>,
    start_time: &str,
    end_time: &str,
    search: Option<&str>,
    methods: &[String],
    status_codes: &[u16],
    levels: &[String],
    sources: &[String],
    user_agent: Option<&str>,
    host: Option<&str>,
    paths: &[String],
    min_duration_ms: Option<u64>,
    max_duration_ms: Option<u64>,
    log_databases: &[String],
    limit: u64,
    offset: u64,
) -> Result<LogsResponse> {
    let query_string = build_query_string(
        start_time,
        end_time,
        search,
        methods,
        status_codes,
        levels,
        sources,
        user_agent,
        host,
        paths,
        min_duration_ms,
        max_duration_ms,
        log_databases,
        limit,
        offset,
    );
    let path = org::database_scoped_path(
        database,
        &format!("/logs?{query_string}"),
        organization,
        cluster,
    );
    client.get(&path)
}

#[allow(clippy::too_many_arguments)]
pub fn logs(
    client: &ApiClient,
    database: &str,
    organization: Option<&str>,
    cluster: Option<&str>,
    search: Option<&str>,
    methods: &[String],
    status_codes: &[u16],
    levels: &[String],
    sources: &[String],
    user_agent: Option<&str>,
    host: Option<&str>,
    paths: &[String],
    min_duration_ms: Option<u64>,
    max_duration_ms: Option<u64>,
    log_databases: &[String],
    limit: u64,
    offset: u64,
    since: Option<&str>,
    until: Option<&str>,
    start_time: Option<&str>,
    end_time: Option<&str>,
    json_mode: bool,
) -> Result<()> {
    if let (Some(min), Some(max)) = (min_duration_ms, max_duration_ms) {
        if min > max {
            bail!(
                "invalid duration filter: --min-duration-ms ({min}) is greater than --max-duration-ms ({max})"
            );
        }
    }

    let (resolved_start, resolved_end) = resolve_time_range(since, until, start_time, end_time)?;
    let resp = fetch_logs(
        client,
        database,
        organization,
        cluster,
        &resolved_start,
        &resolved_end,
        search,
        methods,
        status_codes,
        levels,
        sources,
        user_agent,
        host,
        paths,
        min_duration_ms,
        max_duration_ms,
        log_databases,
        limit,
        offset,
    )?;

    output::print_result(&resp, json_mode, |resp| {
        if resp.logs.is_empty() {
            println!("No request logs found for the specified time range.");
        } else {
            for entry in &resp.logs {
                println!("{}", format_log_line(entry));
            }
            if resp.has_more {
                println!(
                    "\n... more request logs available (use --offset {} to continue)",
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

    fn sample_log_entry() -> LogEntry {
        LogEntry {
            uuid: "request-123".to_string(),
            level: "success".to_string(),
            date: "2026-08-10T12:00:00.123Z".to_string(),
            status: 200,
            pathname: "/v1/query".to_string(),
            latency: 12,
            headers: BTreeMap::new(),
            message: None,
            percentile: None,
            id: "request-123".to_string(),
            time: "2026-08-10T12:00:00.123Z".to_string(),
            method: "POST".to_string(),
            host: "api.rawtree.dev".to_string(),
            path: "/v1/query".to_string(),
            route: "/v1/query".to_string(),
            status_code: 200,
            source: "cli".to_string(),
            duration_ms: 12,
            user_agent: "rawtree-cli/0.6.4".to_string(),
            client_version: "0.6.4".to_string(),
            request_size_bytes: 42,
            response_size_bytes: 128,
            content_encoding: String::new(),
            handler_ms: 10,
            app_overhead_ms: 2,
            error: String::new(),
            error_code: String::new(),
            error_message: String::new(),
            error_hint: String::new(),
            trace_id: "trace-123".to_string(),
            span_id: "span-123".to_string(),
            parent_span_id: String::new(),
            database: "events".to_string(),
            user_id: String::new(),
            user_email: String::new(),
            key_name: "ci".to_string(),
            organization_id: "org-123".to_string(),
            organization_name: "acme".to_string(),
            cluster_id: "cluster-123".to_string(),
            cluster_name: "prod".to_string(),
            url_query: "format=json".to_string(),
            request_body: String::new(),
        }
    }

    #[test]
    fn parse_duration_minutes() {
        assert_eq!(
            parse_duration("30m").unwrap(),
            chrono::Duration::minutes(30)
        );
    }

    #[test]
    fn parse_duration_rejects_invalid_unit() {
        let err = parse_duration("5x").unwrap_err();
        assert!(format!("{err:#}").contains("invalid duration unit"));
    }

    #[test]
    fn build_query_string_includes_request_log_filters() {
        let qs = build_query_string(
            "2026-08-10T00:00:00Z",
            "2026-08-10T01:00:00Z",
            Some("request-123"),
            &["GET".to_string(), "POST".to_string()],
            &[200, 500],
            &["success".to_string(), "error".to_string()],
            &["cli".to_string()],
            Some("rawtree-cli"),
            Some("api.rawtree.dev"),
            &["/v1/query".to_string(), "/v1/logs".to_string()],
            Some(10),
            Some(1000),
            &["events".to_string()],
            50,
            0,
        );

        assert!(qs.contains("search=request-123"));
        assert!(qs.contains("methods=GET%2CPOST"));
        assert!(qs.contains("status_codes=200%2C500"));
        assert!(qs.contains("levels=success%2Cerror"));
        assert!(qs.contains("sources=cli"));
        assert!(qs.contains("user_agent=rawtree-cli"));
        assert!(qs.contains("host=api.rawtree.dev"));
        assert!(qs.contains("paths=%2Fv1%2Fquery%2C%2Fv1%2Flogs"));
        assert!(qs.contains("min_duration_ms=10"));
        assert!(qs.contains("max_duration_ms=1000"));
        assert!(qs.contains("log_databases=events"));
        assert!(qs.contains("limit=50"));
        assert!(qs.contains("offset=0"));
    }

    #[test]
    fn build_query_string_omits_empty_optional_filters() {
        let qs = build_query_string(
            "start",
            "end",
            None,
            &[],
            &[],
            &[],
            &[],
            Some(" "),
            Some(""),
            &[],
            None,
            None,
            &[],
            50,
            0,
        );
        assert_eq!(qs, "start_time=start&end_time=end&limit=50&offset=0");
    }

    #[test]
    fn resolve_time_range_defaults_to_24h() {
        let (start, end) = resolve_time_range(None, None, None, None).unwrap();
        assert!(start.ends_with('Z'));
        assert!(end.ends_with('Z'));
        assert!(start < end);
    }

    #[test]
    fn resolve_time_range_rejects_reversed_absolute_range() {
        let err = resolve_time_range(
            None,
            None,
            Some("2026-08-10T02:00:00Z"),
            Some("2026-08-10T01:00:00Z"),
        )
        .unwrap_err();
        assert!(format!("{err:#}").contains("after --end-time"));
    }

    #[test]
    fn format_log_line_includes_request_metadata() {
        let line = format_log_line(&sample_log_entry());
        assert!(line.contains("2026-08-10T12:00:00.123Z"));
        assert!(line.contains("200"));
        assert!(line.contains("POST"));
        assert!(line.contains("12ms"));
        assert!(line.contains("cli"));
        assert!(line.contains("/v1/query"));
        assert!(!line.contains("["));
    }

    #[test]
    fn format_log_line_includes_error_message() {
        let mut entry = sample_log_entry();
        entry.status = 500;
        entry.status_code = 500;
        entry.level = "error".to_string();
        entry.error_message = "database unavailable".to_string();
        let line = format_log_line(&entry);
        assert!(line.contains("500"));
        assert!(line.contains("[database unavailable]"));
    }
}

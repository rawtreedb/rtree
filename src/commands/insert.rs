use std::fs;
use std::io::{BufRead, BufReader, IsTerminal};
use std::path::Path;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::mpsc;
use std::sync::Arc;
use std::thread;
use std::time::Duration;

use anyhow::{bail, Context, Result};
use indicatif::{ProgressBar, ProgressStyle};
use serde::de::Error as DeError;
use serde::Deserialize;
use serde_json::{json, Value};

use crate::client::ApiClient;
use crate::org;
use crate::output;

const BATCH_SIZE: usize = 5000;
const READ_BUF_SIZE: usize = 1024 * 1024;

fn num_senders() -> usize {
    std::thread::available_parallelism()
        .map(|n| n.get())
        .unwrap_or(4)
}

#[derive(Deserialize)]
struct InsertResponse {
    inserted: usize,
}

#[derive(Deserialize)]
struct UrlInsertEvent {
    #[serde(rename = "type")]
    event_type: Option<String>,
    query_id: Option<String>,
    #[serde(default, deserialize_with = "deserialize_opt_u64")]
    written_rows: Option<u64>,
    #[serde(default, deserialize_with = "deserialize_opt_u64")]
    written_bytes: Option<u64>,
    #[serde(default, deserialize_with = "deserialize_opt_u64")]
    read_rows: Option<u64>,
    #[serde(default, deserialize_with = "deserialize_opt_u64")]
    read_bytes: Option<u64>,
    #[serde(default, deserialize_with = "deserialize_opt_u64")]
    total_rows_to_read: Option<u64>,
    #[serde(default, deserialize_with = "deserialize_opt_u64")]
    elapsed_ms: Option<u64>,
    #[serde(default, deserialize_with = "deserialize_opt_u64")]
    elapsed_ns: Option<u64>,
    #[serde(default, deserialize_with = "deserialize_opt_u64")]
    memory_usage: Option<u64>,
    message: Option<String>,
    #[serde(default, deserialize_with = "deserialize_opt_u64_usize")]
    inserted: Option<usize>,
}

impl UrlInsertEvent {
    fn effective_rows(&self) -> Option<u64> {
        self.written_rows.or(self.read_rows)
    }

    fn effective_bytes(&self) -> Option<u64> {
        self.written_bytes.or(self.read_bytes)
    }
}

#[derive(Debug)]
struct UrlInsertSummary {
    inserted: usize,
    processed_bytes: Option<u64>,
    query_id: Option<String>,
    elapsed_ms: Option<u64>,
}

fn is_jsonl(path: &str) -> bool {
    Path::new(path)
        .extension()
        .map(|ext| ext.eq_ignore_ascii_case("jsonl"))
        .unwrap_or(false)
}

fn deserialize_opt_u64<'de, D>(deserializer: D) -> Result<Option<u64>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let value = Option::<Value>::deserialize(deserializer)?;
    match value {
        None | Some(Value::Null) => Ok(None),
        Some(Value::Number(n)) => n
            .as_u64()
            .ok_or_else(|| DeError::custom("invalid u64 number"))
            .map(Some),
        Some(Value::String(s)) => s
            .parse::<u64>()
            .map(Some)
            .map_err(|_| DeError::custom("invalid u64 string")),
        Some(other) => Err(DeError::custom(format!(
            "expected number|string|null, got {other}"
        ))),
    }
}

fn deserialize_opt_u64_usize<'de, D>(deserializer: D) -> Result<Option<usize>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    deserialize_opt_u64(deserializer)?
        .map(|v| usize::try_from(v).map_err(|_| DeError::custom("u64 does not fit in usize")))
        .transpose()
}

fn format_with_metric_suffix(value: f64, unit: &str) -> String {
    if value >= 1_000_000_000.0 {
        format!("{:.2} billion {unit}", value / 1_000_000_000.0)
    } else if value >= 1_000_000.0 {
        format!("{:.2} million {unit}", value / 1_000_000.0)
    } else if value >= 1_000.0 {
        format!("{:.2} thousand {unit}", value / 1_000.0)
    } else {
        format!("{value:.0} {unit}")
    }
}

fn format_rows_value(rows: u64) -> String {
    format_with_metric_suffix(rows as f64, "rows")
}

fn format_rows_rate(rows: u64, elapsed_ms: Option<u64>) -> Option<String> {
    let elapsed = elapsed_ms?;
    if elapsed == 0 {
        return None;
    }

    let rows_per_second = (rows as f64) / ((elapsed as f64) / 1000.0);
    Some(format_with_metric_suffix(rows_per_second, "rows/s."))
}

fn format_url_insert_progress_line(rows: u64, elapsed_ms: Option<u64>) -> String {
    let rows_text = format_rows_value(rows);
    if let Some(rate) = format_rows_rate(rows, elapsed_ms) {
        format!("Progress: {rows_text} ({rate})")
    } else {
        format!("Progress: {rows_text}")
    }
}

fn format_bytes_value(bytes: u64) -> String {
    let units = ["B", "KB", "MB", "GB", "TB"];
    let mut value = bytes as f64;
    let mut i = 0usize;
    while value >= 1000.0 && i < units.len() - 1 {
        value /= 1000.0;
        i += 1;
    }

    if i == 0 {
        format!("{bytes} {}", units[i])
    } else {
        format!("{value:.2} {}", units[i])
    }
}

fn format_bytes_rate(bytes: u64, elapsed_ms: Option<u64>) -> Option<String> {
    let elapsed = elapsed_ms?;
    if elapsed == 0 {
        return None;
    }

    let bytes_per_second = (bytes as f64) / ((elapsed as f64) / 1000.0);
    Some(format!(
        "{}/s.",
        format_bytes_value(bytes_per_second as u64)
    ))
}

fn format_url_insert_progress_line_with_bytes(
    rows: u64,
    bytes: Option<u64>,
    elapsed_ms: Option<u64>,
) -> String {
    let base_rows = format_rows_value(rows);
    let rows_rate = format_rows_rate(rows, elapsed_ms);

    if let Some(bytes_value) = bytes {
        let bytes_text = format_bytes_value(bytes_value);
        let bytes_rate = format_bytes_rate(bytes_value, elapsed_ms);
        match (rows_rate, bytes_rate) {
            (Some(rr), Some(br)) => {
                format!("Progress: {base_rows}, {bytes_text} ({rr}, {br})")
            }
            (Some(rr), None) => format!("Progress: {base_rows}, {bytes_text} ({rr})"),
            (None, Some(br)) => format!("Progress: {base_rows}, {bytes_text} ({br})"),
            (None, None) => format!("Progress: {base_rows}, {bytes_text}"),
        }
    } else if let Some(rr) = rows_rate {
        format!("Progress: {base_rows} ({rr})")
    } else {
        format!("Progress: {base_rows}")
    }
}

fn parse_url_insert_event_line(trimmed: &str) -> Result<(UrlInsertEvent, bool)> {
    const CH_PROGRESS_PREFIX: &str = "X-ClickHouse-Progress:";
    if let Some(raw_json) = trimmed.strip_prefix(CH_PROGRESS_PREFIX) {
        let mut event: UrlInsertEvent =
            serde_json::from_str(raw_json.trim()).with_context(|| {
                format!(
                    "invalid URL insert stream event payload after {}",
                    CH_PROGRESS_PREFIX
                )
            })?;
        if event.event_type.is_none() {
            event.event_type = Some("progress".to_string());
        }
        return Ok((event, true));
    }

    let event: UrlInsertEvent = serde_json::from_str(trimmed)
        .with_context(|| format!("invalid URL insert stream event: {}", trimmed))?;
    if event.event_type.is_none()
        && event.inserted.is_none()
        && (event.effective_rows().is_some()
            || event.effective_bytes().is_some()
            || event.total_rows_to_read.is_some())
    {
        let mut inferred = event;
        inferred.event_type = Some("progress".to_string());
        return Ok((inferred, true));
    }
    Ok((event, false))
}

fn format_elapsed_seconds(elapsed_ms: u64) -> String {
    format!("{:.3}", (elapsed_ms as f64) / 1000.0)
}

fn build_url_insert_completion_lines(
    inserted: usize,
    processed_bytes: Option<u64>,
    elapsed_ms: Option<u64>,
) -> Vec<String> {
    let mut lines = Vec::with_capacity(2);

    if let Some(elapsed) = elapsed_ms {
        lines.push(format!(
            "{} rows in set. Elapsed: {} sec.",
            inserted,
            format_elapsed_seconds(elapsed)
        ));
    } else {
        lines.push(format!("{inserted} rows in set."));
    }

    let inserted_u64 = inserted as u64;
    let processed_rows = format_rows_value(inserted_u64);
    let rows_rate = format_rows_rate(inserted_u64, elapsed_ms);
    let bytes_rate = processed_bytes.and_then(|b| format_bytes_rate(b, elapsed_ms));
    if let Some(bytes) = processed_bytes {
        let bytes_text = format_bytes_value(bytes);
        match (rows_rate, bytes_rate) {
            (Some(rr), Some(br)) => {
                lines.push(format!(
                    "Processed {processed_rows}, {bytes_text} ({rr}, {br})."
                ));
            }
            (Some(rr), None) => {
                lines.push(format!("Processed {processed_rows}, {bytes_text} ({rr})."));
            }
            (None, Some(br)) => {
                lines.push(format!("Processed {processed_rows}, {bytes_text} ({br})."));
            }
            (None, None) => {
                lines.push(format!("Processed {processed_rows}, {bytes_text}."));
            }
        }
    } else if let Some(rate) = rows_rate {
        lines.push(format!("Processed {processed_rows} ({rate})."));
    } else {
        lines.push(format!("Processed {processed_rows}."));
    }

    lines
}

/// Build the JSON request body into a reusable buffer by string concatenation.
fn build_body_into(body: &mut String, lines: &[String]) {
    body.clear();
    body.push('[');
    for (i, line) in lines.iter().enumerate() {
        if i > 0 {
            body.push(',');
        }
        body.push_str(line);
    }
    body.push(']');
}

fn print_inserted(count: usize, json_mode: bool) {
    output::print_result(&json!({"inserted": count}), json_mode, |_| {
        println!("Inserted {} row(s).", count)
    });
}

fn append_transform_param(path: &str, transform: Option<&str>) -> String {
    match transform {
        Some(t) => {
            let sep = if path.contains('?') { '&' } else { '?' };
            format!("{path}{sep}transform={}", urlencoding::encode(t))
        }
        None => path.to_string(),
    }
}

pub fn insert(
    client: &ApiClient,
    database: &str,
    organization: Option<&str>,
    cluster: Option<&str>,
    table: &str,
    data: Option<&str>,
    file: Option<&str>,
    url: Option<&str>,
    transform: Option<&str>,
    json_mode: bool,
) -> Result<()> {
    let base_path =
        org::database_scoped_path(database, &format!("/tables/{table}"), organization, cluster);
    let api_path = append_transform_param(&base_path, transform);

    // Small inline data — send in one request
    if let Some(raw) = data {
        let json_data: Value = serde_json::from_str(raw).context("invalid JSON in --data")?;
        let resp: InsertResponse = client.post(&api_path, &json_data)?;
        print_inserted(resp.inserted, json_mode);
        return Ok(());
    }

    if let Some(raw_url) = url {
        return insert_from_url(
            client,
            database,
            organization,
            cluster,
            table,
            raw_url,
            transform,
            json_mode,
        );
    }

    let path =
        file.ok_or_else(|| anyhow::anyhow!("provide exactly one of --data, --file, or --url"))?;

    if is_jsonl(path) {
        insert_jsonl_streaming(
            client,
            database,
            organization,
            cluster,
            table,
            path,
            transform,
            json_mode,
        )?;
    } else {
        // Non-JSONL files: parse entire file (assumed to be a JSON array or object)
        let contents =
            fs::read_to_string(path).with_context(|| format!("failed to read file '{}'", path))?;
        let json_data: Value = serde_json::from_str(&contents).context("invalid JSON in file")?;
        let resp: InsertResponse = client.post(&api_path, &json_data)?;
        print_inserted(resp.inserted, json_mode);
    }

    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn insert_from_url(
    client: &ApiClient,
    database: &str,
    organization: Option<&str>,
    cluster: Option<&str>,
    table: &str,
    url: &str,
    transform: Option<&str>,
    json_mode: bool,
) -> Result<()> {
    let base_path = build_url_ingest_path(database, organization, cluster, table, url);
    let path = append_transform_param(&base_path, transform);
    let resp = client.post_empty_stream(&path)?;
    let summary = consume_url_insert_stream(resp, json_mode)?;
    if json_mode {
        print_inserted(summary.inserted, true);
    } else {
        for line in build_url_insert_completion_lines(
            summary.inserted,
            summary.processed_bytes,
            summary.elapsed_ms,
        ) {
            println!("{}", line);
        }
        if let Some(query_id) = summary.query_id {
            println!("Query ID: {}", query_id);
        }
    }
    Ok(())
}

fn build_url_ingest_path(
    database: &str,
    organization: Option<&str>,
    cluster: Option<&str>,
    table: &str,
    url: &str,
) -> String {
    let encoded_url = urlencoding::encode(url);
    org::database_scoped_path(
        database,
        &format!("/tables/{table}?url={encoded_url}"),
        organization,
        cluster,
    )
}

fn consume_url_insert_stream(
    resp: reqwest::blocking::Response,
    json_mode: bool,
) -> Result<UrlInsertSummary> {
    let reader = BufReader::new(resp);
    consume_url_insert_stream_reader(reader, json_mode)
}

fn consume_url_insert_stream_reader<R: BufRead>(
    mut reader: R,
    json_mode: bool,
) -> Result<UrlInsertSummary> {
    let show_progress = !json_mode && std::io::stderr().is_terminal();
    let url_insert_pb = if show_progress {
        let pb = ProgressBar::new_spinner();
        pb.set_style(
            ProgressStyle::with_template("{spinner} {msg}")
                .unwrap()
                .tick_strings(&[
                    "\x1b[31m↖\x1b[0m",
                    "\x1b[33m↗\x1b[0m",
                    "\x1b[34m↘\x1b[0m",
                    "\x1b[35m↙\x1b[0m",
                    " ",
                ]),
        );
        pb.enable_steady_tick(Duration::from_millis(120));
        pb.set_message(format_url_insert_progress_line(0, None));
        pb
    } else {
        ProgressBar::hidden()
    };

    let mut line = String::new();
    let mut saw_any = false;
    let mut saw_done = false;
    let mut saw_native_progress = false;
    let mut query_id: Option<String> = None;
    let mut done_rows: Option<u64> = None;
    let mut done_bytes: Option<u64> = None;
    let mut last_progress_rows: Option<u64> = None;
    let mut last_progress_bytes: Option<u64> = None;
    let mut elapsed_ms: Option<u64> = None;
    let mut last_progress_elapsed_ms: Option<u64> = None;
    let mut legacy_inserted: Option<usize> = None;
    loop {
        line.clear();
        let read = reader
            .read_line(&mut line)
            .context("failed reading URL insert stream")?;
        if read == 0 {
            break;
        }
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        saw_any = true;

        let (event, is_native_progress) = parse_url_insert_event_line(trimmed)?;
        if is_native_progress {
            saw_native_progress = true;
        }

        if let Some(inserted) = event.inserted {
            legacy_inserted = Some(inserted);
            continue;
        }

        let event_elapsed_ms = event
            .elapsed_ms
            .or(event.elapsed_ns.map(|ns| ns / 1_000_000));
        let _ = event.total_rows_to_read;
        let _ = event.memory_usage;

        match event.event_type.as_deref() {
            Some("started") => {
                if query_id.is_none() {
                    query_id = event.query_id.clone();
                }
                if show_progress {
                    url_insert_pb.set_message(format_url_insert_progress_line(0, None));
                }
            }
            Some("progress") => {
                if query_id.is_none() {
                    query_id = event.query_id.clone();
                }
                if let Some(rows) = event.effective_rows() {
                    let progress_bytes = event.effective_bytes().or(last_progress_bytes);
                    let progress_elapsed_ms = event_elapsed_ms.or(last_progress_elapsed_ms);
                    last_progress_rows = Some(rows);
                    last_progress_bytes = progress_bytes;
                    last_progress_elapsed_ms = progress_elapsed_ms;
                    if show_progress {
                        let progress_line = format_url_insert_progress_line_with_bytes(
                            rows,
                            progress_bytes,
                            progress_elapsed_ms,
                        );
                        url_insert_pb.set_message(progress_line);
                    }
                }
            }
            Some("done") => {
                saw_done = true;
                if query_id.is_none() {
                    query_id = event.query_id.clone();
                }
                done_rows = event.effective_rows().or(last_progress_rows);
                done_bytes = event.effective_bytes().or(last_progress_bytes);
                elapsed_ms = event_elapsed_ms.or(last_progress_elapsed_ms);
                if show_progress {
                    if let Some(rows) = done_rows {
                        let progress_line = format_url_insert_progress_line_with_bytes(
                            rows, done_bytes, elapsed_ms,
                        );
                        url_insert_pb.set_message(progress_line);
                    } else {
                        url_insert_pb.set_message("Progress: done");
                    }
                }
            }
            Some("error") => {
                if show_progress {
                    url_insert_pb.finish_and_clear();
                }
                let message = event
                    .message
                    .unwrap_or_else(|| "URL insert failed".to_string());
                if let Some(qid) = event.query_id {
                    bail!("URL insert failed (query_id: {}): {}", qid, message);
                }
                bail!("URL insert failed: {}", message);
            }
            Some(_) | None => {}
        }
    }

    if show_progress {
        url_insert_pb.finish_and_clear();
    }

    if let Some(inserted) = legacy_inserted {
        return Ok(UrlInsertSummary {
            inserted,
            processed_bytes: None,
            query_id: None,
            elapsed_ms: None,
        });
    }

    if !saw_any {
        bail!("empty response from URL insert endpoint");
    }
    if !saw_done && !(saw_native_progress && last_progress_rows.is_some()) {
        bail!("URL insert stream ended without a done event");
    }

    let inserted_u64 = done_rows.or(last_progress_rows).unwrap_or(0);
    let inserted = usize::try_from(inserted_u64)
        .context("inserted row count from stream exceeds supported platform size")?;

    Ok(UrlInsertSummary {
        inserted,
        processed_bytes: done_bytes.or(last_progress_bytes),
        query_id,
        elapsed_ms: elapsed_ms.or(last_progress_elapsed_ms),
    })
}

/// Stream JSONL: reader thread reads raw lines into batches, sender threads
/// build JSON by string concatenation, gzip-compress, and POST to server.
#[allow(clippy::too_many_arguments)]
fn insert_jsonl_streaming(
    client: &ApiClient,
    database: &str,
    organization: Option<&str>,
    cluster: Option<&str>,
    table: &str,
    path: &str,
    transform: Option<&str>,
    json_mode: bool,
) -> Result<()> {
    let file_size = fs::metadata(path)
        .with_context(|| format!("failed to stat file '{}'", path))?
        .len();

    let show_progress = std::io::stderr().is_terminal() && !json_mode;
    let pb = if show_progress {
        let pb = ProgressBar::new(file_size);
        pb.set_style(
            ProgressStyle::with_template(
                "{spinner:.green} [{elapsed_precise}] [{wide_bar:.cyan/blue}] {bytes}/{total_bytes} ({bytes_per_sec}, {eta})",
            )
            .unwrap()
            .progress_chars("#>-"),
        );
        pb
    } else {
        ProgressBar::hidden()
    };

    let senders = num_senders();
    let (tx, rx) = mpsc::sync_channel::<Vec<String>>(senders * 4);
    let rx = Arc::new(std::sync::Mutex::new(rx));
    let inserted = Arc::new(AtomicUsize::new(0));
    let failed = Arc::new(AtomicBool::new(false));
    let first_error: Arc<std::sync::Mutex<Option<String>>> = Arc::new(std::sync::Mutex::new(None));

    // Spawn sender threads — each reuses a body buffer, compresses, and POSTs
    let base_url =
        org::database_scoped_path(database, &format!("/tables/{table}"), organization, cluster);
    let url = append_transform_param(&base_url, transform);
    let mut handles = Vec::with_capacity(senders);
    for _ in 0..senders {
        let rx = Arc::clone(&rx);
        let inserted = Arc::clone(&inserted);
        let failed = Arc::clone(&failed);
        let first_error = Arc::clone(&first_error);
        let client = ApiClient::new(client.base_url.clone(), client.token.clone());
        let url = url.clone();

        handles.push(thread::spawn(move || {
            let mut body = String::with_capacity(512 * 1024);
            loop {
                let batch = {
                    let lock = rx.lock().unwrap();
                    lock.recv()
                };
                let batch = match batch {
                    Ok(b) => b,
                    Err(_) => break,
                };
                let batch_len = batch.len();
                build_body_into(&mut body, &batch);
                match client.post_compressed::<InsertResponse>(&url, &body) {
                    Ok(resp) => {
                        inserted.fetch_add(resp.inserted, Ordering::Relaxed);
                    }
                    Err(e) => {
                        failed.store(true, Ordering::Relaxed);
                        let mut err = first_error.lock().unwrap();
                        if err.is_none() {
                            *err = Some(format!("{:#}", e));
                        }
                    }
                }
                drop(batch);
                let _ = batch_len;
            }
        }));
    }

    // Reader: read raw lines into batches using read_line for buffer reuse
    let file = fs::File::open(path).with_context(|| format!("failed to read file '{}'", path))?;
    let mut reader = BufReader::with_capacity(READ_BUF_SIZE, file);
    let mut current_batch: Vec<String> = Vec::with_capacity(BATCH_SIZE);
    let mut line_buf = String::new();
    let mut has_rows = false;

    loop {
        if failed.load(Ordering::Relaxed) {
            break;
        }

        line_buf.clear();
        let bytes_read = reader
            .read_line(&mut line_buf)
            .context("failed to read line")?;
        if bytes_read == 0 {
            break; // EOF
        }

        pb.inc(bytes_read as u64);

        let trimmed = line_buf.trim_end();
        if trimmed.is_empty() {
            continue;
        }

        has_rows = true;
        current_batch.push(trimmed.to_string());

        if current_batch.len() >= BATCH_SIZE {
            let batch = std::mem::replace(&mut current_batch, Vec::with_capacity(BATCH_SIZE));
            if tx.send(batch).is_err() {
                break;
            }
        }
    }

    if !current_batch.is_empty() {
        let _ = tx.send(current_batch);
    }

    if !has_rows {
        drop(tx);
        for h in handles {
            let _ = h.join();
        }
        bail!("JSONL file is empty");
    }

    drop(tx);
    for h in handles {
        let _ = h.join();
    }

    pb.finish_and_clear();

    let total_inserted = inserted.load(Ordering::Relaxed);
    print_inserted(total_inserted, json_mode);

    let err = first_error.lock().unwrap();
    if let Some(ref msg) = *err {
        bail!("Failed to insert some batches: {}", msg);
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{
        build_url_ingest_path, build_url_insert_completion_lines, consume_url_insert_stream_reader,
        format_rows_rate, format_rows_value, format_url_insert_progress_line,
        format_url_insert_progress_line_with_bytes,
    };
    use std::io::Cursor;

    #[test]
    fn url_ingest_path_uses_query_param_route() {
        let path = build_url_ingest_path(
            "events_2",
            None,
            Some("production"),
            "events",
            "https://example.com/a b.ndjson",
        );

        assert_eq!(
            path,
            "/v1/tables/events?url=https%3A%2F%2Fexample.com%2Fa%20b.ndjson&database=events_2&cluster=production"
        );
    }

    #[test]
    fn url_insert_stream_uses_done_written_rows() {
        let payload = r#"{"type":"started","query_id":"q1","elapsed_ms":0}
{"type":"progress","query_id":"q1","written_rows":10,"elapsed_ms":100}
{"type":"done","query_id":"q1","written_rows":12,"elapsed_ms":150}
"#;
        let summary = consume_url_insert_stream_reader(Cursor::new(payload), true)
            .expect("stream should parse");
        assert_eq!(summary.inserted, 12);
        assert_eq!(summary.query_id.as_deref(), Some("q1"));
        assert_eq!(summary.elapsed_ms, Some(150));
    }

    #[test]
    fn url_insert_stream_uses_progress_rows_when_done_rows_missing() {
        let payload = r#"{"type":"started","query_id":"q1","elapsed_ms":0}
{"type":"progress","query_id":"q1","written_rows":7,"elapsed_ms":100}
{"type":"done","query_id":"q1","written_rows":null,"elapsed_ms":150}
"#;
        let summary = consume_url_insert_stream_reader(Cursor::new(payload), true)
            .expect("stream should parse");
        assert_eq!(summary.inserted, 7);
    }

    #[test]
    fn url_insert_stream_errors_on_error_event() {
        let payload = r#"{"type":"started","query_id":"q1","elapsed_ms":0}
{"type":"error","query_id":"q1","message":"boom"}
"#;
        let err =
            consume_url_insert_stream_reader(Cursor::new(payload), true).expect_err("must fail");
        assert!(format!("{:#}", err).contains("boom"));
    }

    #[test]
    fn url_insert_stream_parses_in_non_json_mode() {
        let payload = r#"{"type":"started","query_id":"q1","elapsed_ms":0}
{"type":"progress","query_id":"q1","written_rows":4,"elapsed_ms":20}
{"type":"done","query_id":"q1","written_rows":4,"elapsed_ms":25}
"#;
        let summary = consume_url_insert_stream_reader(Cursor::new(payload), false)
            .expect("stream should parse");
        assert_eq!(summary.inserted, 4);
        assert_eq!(summary.elapsed_ms, Some(25));
    }

    #[test]
    fn progress_formatter_uses_clickhouse_like_row_units() {
        assert_eq!(format_rows_value(999), "999 rows");
        assert_eq!(format_rows_value(1_234), "1.23 thousand rows");
        assert_eq!(format_rows_value(11_640_000), "11.64 million rows");
    }

    #[test]
    fn progress_formatter_computes_rows_per_second() {
        let rate = format_rows_rate(11_640_000, Some(26_364)).expect("rate should exist");
        assert_eq!(rate, "441.51 thousand rows/s.");
        assert_eq!(format_rows_rate(10, Some(0)), None);
        assert_eq!(format_rows_rate(10, None), None);
    }

    #[test]
    fn progress_line_includes_rows_and_rate_when_available() {
        let line = format_url_insert_progress_line(11_640_000, Some(26_364));
        assert_eq!(
            line,
            "Progress: 11.64 million rows (441.51 thousand rows/s.)"
        );

        let no_rate = format_url_insert_progress_line(12, None);
        assert_eq!(no_rate, "Progress: 12 rows");
    }

    #[test]
    fn progress_line_with_bytes_includes_dual_rates_when_available() {
        let line =
            format_url_insert_progress_line_with_bytes(1_000_000, Some(200_000_000), Some(2_000));
        assert_eq!(
            line,
            "Progress: 1.00 million rows, 200.00 MB (500.00 thousand rows/s., 100.00 MB/s.)"
        );
    }

    #[test]
    fn completion_lines_include_elapsed_and_processed_rate() {
        let lines = build_url_insert_completion_lines(32_719, None, Some(2_480));
        assert_eq!(lines.len(), 2);
        assert_eq!(lines[0], "32719 rows in set. Elapsed: 2.480 sec.");
        assert_eq!(
            lines[1],
            "Processed 32.72 thousand rows (13.19 thousand rows/s.)."
        );
    }

    #[test]
    fn completion_lines_without_elapsed_skip_rate() {
        let lines = build_url_insert_completion_lines(12, None, None);
        assert_eq!(lines.len(), 2);
        assert_eq!(lines[0], "12 rows in set.");
        assert_eq!(lines[1], "Processed 12 rows.");
    }

    #[test]
    fn completion_lines_with_bytes_include_dual_rates() {
        let lines = build_url_insert_completion_lines(32_719, Some(12_790_000), Some(2_480));
        assert_eq!(lines.len(), 2);
        assert_eq!(lines[0], "32719 rows in set. Elapsed: 2.480 sec.");
        assert_eq!(
            lines[1],
            "Processed 32.72 thousand rows, 12.79 MB (13.19 thousand rows/s., 5.16 MB/s.)."
        );
    }

    #[test]
    fn url_insert_stream_accepts_clickhouse_progress_headers_without_done() {
        let payload = r#"X-ClickHouse-Progress: {"read_rows":"261636","read_bytes":"2093088","total_rows_to_read":"1000000","elapsed_ns":"14050417","memory_usage":"22205975"}
X-ClickHouse-Progress: {"read_rows":"654090","read_bytes":"5232720","total_rows_to_read":"1000000","elapsed_ns":"27948667","memory_usage":"83400279"}
X-ClickHouse-Progress: {"read_rows":"1000000","read_bytes":"8000000","total_rows_to_read":"1000000","elapsed_ns":"38002417","memory_usage":"80715679"}
"#;

        let summary = consume_url_insert_stream_reader(Cursor::new(payload), true)
            .expect("header-style progress should parse");
        assert_eq!(summary.inserted, 1_000_000);
        assert_eq!(summary.processed_bytes, Some(8_000_000));
        assert_eq!(summary.elapsed_ms, Some(38));
    }

    #[test]
    fn url_insert_stream_accepts_events_with_both_read_and_written_fields() {
        let payload = r#"{"elapsed_ns":"3510333127","memory_usage":"2846240784","query_id":"0e957306-1826-45bf-812f-45836cabad04","read_bytes":"50725776","read_rows":"132436","type":"progress","written_bytes":"834982334","written_rows":"98798"}
{"elapsed_ns":"3520000000","memory_usage":"2846240784","query_id":"0e957306-1826-45bf-812f-45836cabad04","read_bytes":"50725776","read_rows":"132436","type":"done","written_bytes":"834982334","written_rows":"98798"}
"#;

        let summary = consume_url_insert_stream_reader(Cursor::new(payload), true)
            .expect("mixed read/written fields should parse");
        assert_eq!(summary.inserted, 98_798);
        assert_eq!(summary.processed_bytes, Some(834_982_334));
        assert_eq!(summary.elapsed_ms, Some(3520));
    }

    #[test]
    fn url_insert_stream_preserves_previous_bytes_and_elapsed_on_partial_progress() {
        let payload = r#"{"type":"progress","query_id":"q1","written_rows":10,"written_bytes":1000,"elapsed_ms":100}
{"type":"progress","query_id":"q1","written_rows":12}
{"type":"done","query_id":"q1","written_rows":12}
"#;

        let summary = consume_url_insert_stream_reader(Cursor::new(payload), true)
            .expect("partial progress should preserve previous stats");
        assert_eq!(summary.inserted, 12);
        assert_eq!(summary.processed_bytes, Some(1000));
        assert_eq!(summary.elapsed_ms, Some(100));
    }
}

use anyhow::Result;
use comfy_table::{Cell, CellAlignment};
use serde::Deserialize;
use serde_json::json;

use super::table_output::new_cli_table;
use crate::client::ApiClient;
use crate::org;
use crate::output;

#[derive(Deserialize)]
struct TablesResponse {
    tables: Vec<TableInfo>,
}

#[derive(Deserialize)]
struct TableInfo {
    name: String,
    #[serde(alias = "rows")]
    total_rows: u64,
    #[serde(alias = "size")]
    total_bytes: u64,
}

#[derive(Deserialize)]
struct ColumnInfo {
    name: String,
    #[serde(rename = "type")]
    col_type: String,
}

#[derive(Deserialize)]
struct DescribeTableResponse {
    name: String,
    created_at: String,
    #[serde(alias = "rows")]
    total_rows: u64,
    #[serde(alias = "size")]
    total_bytes: u64,
    columns: Vec<ColumnInfo>,
}

pub fn list(
    client: &ApiClient,
    database: &str,
    organization: Option<&str>,
    cluster: Option<&str>,
    json_mode: bool,
) -> Result<()> {
    let list_path = org::database_scoped_path(database, "/tables", organization, cluster);
    let resp: TablesResponse = client.get(&list_path)?;
    output::print_result(
        &json!({
            "tables": resp.tables.iter().map(|t| json!({
                "name": t.name,
                "total_rows": t.total_rows,
                "total_bytes": t.total_bytes,
            })).collect::<Vec<_>>()
        }),
        json_mode,
        |_| {
            if resp.tables.is_empty() {
                println!("No tables yet. Insert data to auto-create a table.");
            } else {
                let mut table = new_cli_table();
                table.set_header(vec!["table", "rows", "size"]);
                for t in &resp.tables {
                    table.add_row(vec![
                        Cell::new(&t.name),
                        Cell::new(t.total_rows.to_string()).set_alignment(CellAlignment::Right),
                        Cell::new(format_bytes(t.total_bytes)).set_alignment(CellAlignment::Right),
                    ]);
                }
                println!("{table}");
            }
        },
    );
    Ok(())
}

pub fn describe(
    client: &ApiClient,
    database: &str,
    organization: Option<&str>,
    cluster: Option<&str>,
    table: &str,
    json_mode: bool,
) -> Result<()> {
    let describe_path =
        org::database_scoped_path(database, &format!("/tables/{table}"), organization, cluster);
    let resp: DescribeTableResponse = client.get(&describe_path)?;
    output::print_result(
        &json!({
            "table": resp.name,
            "created_at": resp.created_at,
            "total_rows": resp.total_rows,
            "total_bytes": resp.total_bytes,
            "columns": resp.columns.iter().map(|c| json!({
                "name": c.name,
                "type": c.col_type,
            })).collect::<Vec<_>>(),
        }),
        json_mode,
        |_| {
            println!("Table: {}", resp.name);
            println!("Rows: {}", resp.total_rows);
            println!("Size: {}", format_bytes(resp.total_bytes));
            println!("Created at: {}", resp.created_at);
            println!();

            let mut columns = new_cli_table();
            columns.set_header(vec!["column", "type"]);
            for col in &resp.columns {
                columns.add_row(vec![Cell::new(&col.name), Cell::new(&col.col_type)]);
            }

            println!("{columns}");
        },
    );
    Ok(())
}

fn format_bytes(bytes: u64) -> String {
    const UNITS: [&str; 6] = ["B", "KiB", "MiB", "GiB", "TiB", "PiB"];
    if bytes < 1024 {
        return format!("{bytes} B");
    }

    let mut size = bytes as f64;
    let mut unit_index = 0usize;
    while size >= 1024.0 && unit_index < UNITS.len() - 1 {
        size /= 1024.0;
        unit_index += 1;
    }

    format!("{size:.1} {}", UNITS[unit_index])
}

#[cfg(test)]
mod tests {
    use super::{DescribeTableResponse, TablesResponse};

    #[test]
    fn tables_response_accepts_new_field_names() {
        let payload = r#"{
            "tables": [{
                "name": "events",
                "total_rows": 1200,
                "total_bytes": 98304
            }]
        }"#;

        let resp: TablesResponse = serde_json::from_str(payload).expect("valid payload");
        assert_eq!(resp.tables.len(), 1);
        assert_eq!(resp.tables[0].total_rows, 1200);
        assert_eq!(resp.tables[0].total_bytes, 98304);
    }

    #[test]
    fn describe_response_accepts_legacy_field_names() {
        let payload = r#"{
            "name": "events",
            "created_at": "2026-01-01 10:00:00",
            "rows": 1200,
            "size": 98304,
            "columns": [{"name": "event", "type": "String"}]
        }"#;

        let resp: DescribeTableResponse = serde_json::from_str(payload).expect("valid payload");
        assert_eq!(resp.total_rows, 1200);
        assert_eq!(resp.total_bytes, 98304);
        assert_eq!(resp.columns.len(), 1);
    }
}

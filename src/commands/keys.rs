use anyhow::Result;
use serde::Deserialize;
use serde_json::json;

use crate::client::ApiClient;
use crate::org;
use crate::output;

#[derive(Deserialize)]
struct ApiKeyItem {
    id: String,
    token: String,
    name: String,
    permission: String,
    created_at: String,
}

#[derive(Deserialize)]
struct ListApiKeysResponse {
    database: Option<ApiKeyDatabaseRef>,
    organization: Option<ApiKeyOrganizationRef>,
    keys: Vec<ApiKeyItem>,
}

#[derive(Deserialize)]
struct CreateApiKeyResponse {
    id: String,
    token: String,
    name: String,
    database: Option<ApiKeyDatabaseRef>,
    organization: Option<ApiKeyOrganizationRef>,
    permission: String,
}

#[derive(Deserialize)]
struct ApiKeyDatabaseRef {
    name: String,
}

#[derive(Deserialize)]
struct ApiKeyOrganizationRef {
    name: String,
}

#[derive(Deserialize)]
struct DeleteApiKeyResponse {
    deleted: bool,
}

pub fn list(
    client: &ApiClient,
    database: &str,
    organization: Option<&str>,
    cluster: Option<&str>,
    json_mode: bool,
) -> Result<()> {
    let path = org::database_scoped_path(database, "/keys", organization, cluster);
    let resp: ListApiKeysResponse = client.get(&path)?;
    output::print_result(
        &json!({
            "database": resp.database.as_ref().map(|p| json!({"name": p.name})),
            "organization": resp.organization.as_ref().map(|o| json!({"name": o.name})),
            "keys": resp.keys.iter().map(|k| json!({
                "id": k.id,
                "token": k.token,
                "name": k.name,
                "permission": k.permission,
                "created_at": k.created_at,
            })).collect::<Vec<_>>()
        }),
        json_mode,
        |_| {
            if resp.keys.is_empty() {
                println!("No API keys.");
            } else {
                for k in &resp.keys {
                    println!(
                        "{:<38} {:<12} {:<14} {}  created={}",
                        k.id, k.name, k.permission, k.token, k.created_at
                    );
                }
            }
        },
    );
    Ok(())
}

pub fn create(
    client: &ApiClient,
    database: &str,
    organization: Option<&str>,
    cluster: Option<&str>,
    name: &str,
    permission: &str,
    json_mode: bool,
) -> Result<()> {
    let body = json!({ "name": name, "permission": permission });
    let path = org::database_scoped_path(database, "/keys", organization, cluster);
    let resp: CreateApiKeyResponse = client.post(&path, &body)?;
    output::print_result(
        &json!({
            "id": resp.id,
            "token": resp.token,
            "name": resp.name,
            "database": resp.database.as_ref().map(|p| json!({"name": p.name})),
            "organization": resp.organization.as_ref().map(|o| json!({"name": o.name})),
            "permission": resp.permission,
        }),
        json_mode,
        |_| {
            println!("API key created:");
            println!("  id:         {}", resp.id);
            println!("  token:      {}", resp.token);
            println!("  name:       {}", resp.name);
            println!("  permission: {}", resp.permission);
        },
    );
    Ok(())
}

pub fn delete(
    client: &ApiClient,
    database: &str,
    organization: Option<&str>,
    cluster: Option<&str>,
    id_or_token: &str,
    json_mode: bool,
) -> Result<()> {
    let encoded_key = urlencoding::encode(id_or_token);
    let path = org::database_scoped_path(
        database,
        &format!("/keys/{encoded_key}"),
        organization,
        cluster,
    );
    let resp: DeleteApiKeyResponse = client.delete(&path)?;
    output::print_result(
        &json!({"deleted": resp.deleted, "id_or_token": id_or_token}),
        json_mode,
        |_| {
            if resp.deleted {
                println!("API key '{}' deleted.", id_or_token);
            }
        },
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{CreateApiKeyResponse, ListApiKeysResponse};

    #[test]
    fn list_response_accepts_new_key_fields() {
        let payload = r#"{
            "database": {"name": "analytics"},
            "organization": {"name": "team_alpha"},
            "keys": [{
                "id": "key-1",
                "token": "rt_***abcd",
                "name": "ci",
                "permission": "read_write",
                "created_at": "2026-01-01 10:00:00"
            }]
        }"#;

        let resp: ListApiKeysResponse = serde_json::from_str(payload).expect("valid payload");
        assert_eq!(
            resp.database.as_ref().map(|p| p.name.as_str()),
            Some("analytics")
        );
        assert_eq!(
            resp.organization.as_ref().map(|o| o.name.as_str()),
            Some("team_alpha")
        );
        assert_eq!(resp.keys.len(), 1);
        assert_eq!(resp.keys[0].id, "key-1");
        assert_eq!(resp.keys[0].token, "rt_***abcd");
        assert_eq!(resp.keys[0].name, "ci");
    }

    #[test]
    fn create_response_accepts_new_key_fields() {
        let payload = r#"{
            "id": "key-1",
            "token": "rt_abcd",
            "name": "ci",
            "database": {"name": "analytics"},
            "organization": {"name": "team_alpha"},
            "permission": "read_write"
        }"#;

        let resp: CreateApiKeyResponse = serde_json::from_str(payload).expect("valid payload");
        assert_eq!(resp.id, "key-1");
        assert_eq!(resp.token, "rt_abcd");
        assert_eq!(resp.name, "ci");
        assert_eq!(
            resp.database.as_ref().map(|p| p.name.as_str()),
            Some("analytics")
        );
        assert_eq!(
            resp.organization.as_ref().map(|o| o.name.as_str()),
            Some("team_alpha")
        );
    }
}

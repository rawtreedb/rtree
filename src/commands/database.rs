use anyhow::Result;
use serde::Deserialize;
use serde_json::json;

use crate::cli::{S3StorageArgs, S3StorageMetadata};
use crate::client::ApiClient;
use crate::config;
use crate::org;
use crate::output;

#[derive(Deserialize)]
struct DatabaseItem {
    name: String,
    #[serde(default)]
    organization: Option<OrganizationRef>,
    #[serde(default, alias = "custom_s3")]
    s3_storage: Option<S3StorageMetadata>,
}

#[derive(Deserialize)]
struct ListDatabasesResponse {
    #[serde(default)]
    organization: Option<OrganizationRef>,
    databases: Vec<DatabaseItem>,
}

#[derive(Clone, Deserialize)]
struct OrganizationRef {
    name: String,
}

#[derive(Deserialize)]
struct CreateDatabaseResponse {
    organization: OrganizationRef,
    database: DatabaseItem,
}

impl CreateDatabaseResponse {
    fn resolved_organization_name(&self) -> Option<&str> {
        Some(self.organization.name.as_str())
    }
}

fn apply_database_create_config(
    cfg: &mut config::Config,
    resp: &CreateDatabaseResponse,
    cluster: Option<&str>,
) {
    cfg.default_database = Some(resp.database.name.clone());
    cfg.default_organization = resp.resolved_organization_name().map(ToString::to_string);
    if let Some(cluster) = cluster {
        cfg.default_cluster = Some(cluster.to_string());
    }
}

fn database_create_collection_path(organization: Option<&str>, cluster: Option<&str>) -> String {
    org::databases_collection_path(organization, cluster)
}

fn create_database_response(
    client: &ApiClient,
    name: &str,
    organization: Option<&str>,
    cluster: Option<&str>,
    s3_storage: &S3StorageArgs,
) -> Result<CreateDatabaseResponse> {
    let path = database_create_collection_path(organization, cluster);
    let body = create_database_request_body(name, s3_storage)?;
    client.post(&path, &body)
}

fn create_database_request_body(
    name: &str,
    s3_storage: &S3StorageArgs,
) -> Result<serde_json::Value> {
    let mut body = json!({ "name": name });
    if let Some(s3_storage) = s3_storage.to_json()? {
        body["s3_storage"] = s3_storage;
    }
    Ok(body)
}

fn create_and_persist(
    client: &ApiClient,
    name: &str,
    organization: Option<&str>,
    cluster: Option<&str>,
    s3_storage: &S3StorageArgs,
) -> Result<CreateDatabaseResponse> {
    let resp = create_database_response(client, name, organization, cluster, s3_storage)?;
    let mut cfg = config::load()?;
    apply_database_create_config(&mut cfg, &resp, cluster);
    config::save(&cfg)?;
    Ok(resp)
}

pub fn list(
    client: &ApiClient,
    organization: Option<&str>,
    cluster: Option<&str>,
    json_mode: bool,
) -> Result<()> {
    let path = org::databases_collection_path(organization, cluster);
    let resp: ListDatabasesResponse = client.get(&path)?;
    output::print_result(
        &json!({
            "databases": resp.databases.iter().map(|p| json!({
                "name": p.name,
                "organization": p
                    .organization
                    .as_ref()
                    .or(resp.organization.as_ref())
                    .map(|org| json!({"name": org.name})),
                "s3_storage": p.s3_storage,
            })).collect::<Vec<_>>()
        }),
        json_mode,
        |_| {
            if resp.databases.is_empty() {
                println!("No databases yet. Create one with `rtree database create <name>`.");
            } else {
                for p in &resp.databases {
                    let organization = p
                        .organization
                        .as_ref()
                        .or(resp.organization.as_ref())
                        .map(|org| org.name.as_str())
                        .unwrap_or("unknown");
                    println!(
                        "{:<20} org={} storage={}",
                        p.name,
                        organization,
                        format_storage(p.s3_storage.as_ref())
                    );
                }
            }
        },
    );
    Ok(())
}

fn format_storage(storage: Option<&S3StorageMetadata>) -> &'static str {
    if storage.is_some() {
        "customer-owned S3"
    } else {
        "cluster default"
    }
}

pub fn create(
    client: &ApiClient,
    name: &str,
    organization: Option<&str>,
    cluster: Option<&str>,
    s3_storage: S3StorageArgs,
    json_mode: bool,
) -> Result<()> {
    let storage_configured = s3_storage.to_json()?.is_some();
    let resp = create_and_persist(client, name, organization, cluster, &s3_storage)?;

    output::print_result(
        &json!({
            "name": resp.database.name,
            "organization": resp
                .resolved_organization_name()
                .map(|name| json!({"name": name})),
            "storage_configured": storage_configured,
        }),
        json_mode,
        |_| {
            let organization_name = resp.resolved_organization_name().unwrap_or("unknown");
            println!(
                "Database '{}' created in organization '{}'.",
                resp.database.name, organization_name
            );
            if storage_configured {
                println!("Using customer-owned S3 storage.");
            }
        },
    );
    Ok(())
}

pub fn use_database(name: &str, json_mode: bool) -> Result<()> {
    let mut cfg = config::load()?;
    cfg.default_database = Some(name.to_string());
    config::save(&cfg)?;

    output::print_result(&json!({"default_database": name}), json_mode, |_| {
        println!("Default database set to '{}'.", name)
    });
    Ok(())
}

#[derive(Deserialize)]
struct DeleteDatabaseResponse {
    deleted: bool,
}

pub fn delete(
    client: &ApiClient,
    name: &str,
    organization: Option<&str>,
    cluster: Option<&str>,
    json_mode: bool,
) -> Result<()> {
    let path = org::scoped_path(
        &format!("/v1/databases/{}", urlencoding::encode(name)),
        organization,
        cluster,
    );
    let resp: DeleteDatabaseResponse = client.delete(&path)?;
    output::print_result(
        &json!({"deleted": resp.deleted, "name": name}),
        json_mode,
        |_| {
            if resp.deleted {
                println!("Database '{}' deleted.", name);
            }
        },
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{
        apply_database_create_config, create_database_request_body,
        database_create_collection_path, CreateDatabaseResponse, DatabaseItem, OrganizationRef,
    };
    use crate::cli::S3StorageArgs;
    use crate::config::Config;
    use serde_json::json;

    #[test]
    fn apply_database_create_config_preserves_jwt_for_standard_databases() {
        let mut cfg = Config {
            token: Some("jwt.token.value".to_string()),
            email: Some("user@example.com".to_string()),
            default_organization: Some("team_alpha".to_string()),
            ..Config::default()
        };
        let resp = CreateDatabaseResponse {
            organization: OrganizationRef {
                name: "new_team".to_string(),
            },
            database: DatabaseItem {
                name: "analytics".to_string(),
                organization: None,
                s3_storage: None,
            },
        };

        apply_database_create_config(&mut cfg, &resp, Some("production"));

        assert_eq!(cfg.token.as_deref(), Some("jwt.token.value"));
        assert_eq!(cfg.email.as_deref(), Some("user@example.com"));
        assert_eq!(cfg.default_organization.as_deref(), Some("new_team"));
        assert_eq!(cfg.default_cluster.as_deref(), Some("production"));
        assert_eq!(cfg.default_database.as_deref(), Some("analytics"));
    }

    #[test]
    fn database_item_deserializes_nested_organization_field() {
        let item: DatabaseItem = serde_json::from_value(json!({
            "name": "analytics",
            "organization": {"name": "team_alpha"}
        }))
        .expect("database item should deserialize");

        assert_eq!(item.name, "analytics");
        assert_eq!(item.organization.expect("organization").name, "team_alpha");
    }

    #[test]
    fn database_item_deserializes_s3_storage_metadata() {
        let item: DatabaseItem = serde_json::from_value(json!({
            "name": "analytics",
            "s3_storage": {
                "data": {"bucket": "customer-data", "path": "rawtree/data"},
                "backups": {"bucket": "customer-backups", "path": "rawtree/backups"}
            }
        }))
        .expect("database storage metadata should deserialize");

        assert_eq!(
            item.s3_storage.expect("storage metadata").data.bucket,
            "customer-data"
        );
    }

    #[test]
    fn database_create_response_deserializes_nested_platform_shape() {
        let response: CreateDatabaseResponse = serde_json::from_value(json!({
            "organization": {"name": "team_alpha"},
            "database": {"name": "analytics"}
        }))
        .expect("nested database create response should deserialize");

        assert_eq!(response.database.name, "analytics");
        assert_eq!(response.resolved_organization_name(), Some("team_alpha"));
    }

    #[test]
    fn database_create_body_uses_s3_storage() {
        let storage = S3StorageArgs {
            s3_data_bucket: Some("customer-data".to_string()),
            s3_data_path: None,
            s3_backups_bucket: Some("customer-backups".to_string()),
            s3_backups_path: None,
            s3_role_arn: Some("arn:aws:iam::123456789012:role/RawTreeS3Access".to_string()),
            s3_external_id: Some("rawtree-example".to_string()),
        };

        assert_eq!(
            create_database_request_body("analytics", &storage).expect("valid storage"),
            json!({
                "name": "analytics",
                "s3_storage": {
                    "data": {"bucket": "customer-data", "path": ""},
                    "backups": {"bucket": "customer-backups", "path": ""},
                    "role_arn": "arn:aws:iam::123456789012:role/RawTreeS3Access",
                    "external_id": "rawtree-example"
                }
            })
        );
    }

    #[test]
    fn database_create_collection_path_uses_databases_endpoint() {
        assert_eq!(database_create_collection_path(None, None), "/v1/databases");
        assert_eq!(
            database_create_collection_path(Some("team alpha"), Some("prod/eu")),
            "/v1/databases?organization=team%20alpha&cluster=prod%2Feu"
        );
    }
}

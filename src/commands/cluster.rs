use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
use comfy_table::{Cell, CellAlignment};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use super::table_output::new_cli_table;
use crate::cli::{ClusterSizeArg, S3StorageArgs, S3StorageMetadata};
use crate::client::ApiClient;
use crate::config;
use crate::output;

#[derive(Deserialize)]
struct ListClustersResponse {
    organization: OrganizationRef,
    clusters: Vec<ClusterItem>,
}

#[derive(Deserialize)]
struct OrganizationRef {
    name: String,
}

#[derive(Clone, Deserialize)]
struct ClusterSizeItem {
    cpu_cores: u32,
    memory_gib: u32,
}

#[derive(Deserialize)]
struct ClusterSizesResponse {
    sizes: Vec<ClusterSizeItem>,
}

#[derive(Clone, Deserialize, Serialize)]
struct ClusterItem {
    id: String,
    name: String,
    created_at: String,
    status: ClusterStatus,
    resources: Option<ClusterResources>,
    can_pause: bool,
    can_resume: bool,
    #[serde(default, alias = "custom_s3")]
    s3_storage: Option<S3StorageMetadata>,
    idle_timeout_minutes: u64,
}

#[derive(Clone, Deserialize, Serialize)]
struct ClusterStatus {
    phase: String,
    ready: bool,
    message: Option<String>,
}

#[derive(Clone, Deserialize, Serialize)]
struct ClusterResources {
    shards: u32,
    replicas: u32,
    cpu_cores_per_replica: Option<f64>,
    memory_bytes_per_replica: Option<u64>,
}

#[derive(Clone, Copy)]
enum LifecycleAction {
    Stop,
    Resume,
}

impl LifecycleAction {
    fn path_segment(self) -> &'static str {
        match self {
            Self::Stop => "stop",
            Self::Resume => "resume",
        }
    }

    fn label(self) -> &'static str {
        match self {
            Self::Stop => "Stop",
            Self::Resume => "Resume",
        }
    }
}

#[derive(Deserialize)]
struct DeleteClusterResponse {
    deleted: bool,
}

fn renamed_default_cluster(
    current_default_cluster: Option<&str>,
    old_name: &str,
    cluster_id: &str,
    new_name: &str,
) -> Option<String> {
    match current_default_cluster {
        Some(current) if current == old_name || current == cluster_id => Some(new_name.to_string()),
        Some(current) => Some(current.to_string()),
        None => None,
    }
}

fn default_cluster_after_delete(
    current_default_cluster: Option<&str>,
    deleted_name: &str,
    cluster_id: &str,
) -> Option<String> {
    match current_default_cluster {
        Some(current) if current == deleted_name || current == cluster_id => None,
        Some(current) => Some(current.to_string()),
        None => None,
    }
}

pub fn create(
    client: &ApiClient,
    options: ClusterCreateOptions<'_>,
    json_mode: bool,
) -> Result<()> {
    let s3_storage = options.s3_storage.to_json()?;
    let (_, sizes) = load_cluster_sizes(client)?;
    let (min_index, min_size) = resolve_cluster_size(&sizes.sizes, options.min_size)?;
    let (max_index, max_size) = options
        .max_size
        .map(|size| resolve_cluster_size(&sizes.sizes, size))
        .transpose()?
        .unwrap_or((min_index, min_size));
    if max_index < min_index {
        anyhow::bail!("Maximum size must be at least as large as the minimum size");
    }

    let body = create_request_body(
        options.name,
        options.replicas,
        min_size,
        max_size,
        options.idle_timeout_minutes,
        s3_storage,
    );

    let value: Value = client.post(&clusters_collection_path(options.organization), &body)?;
    let created: ClusterItem =
        serde_json::from_value(value.clone()).context("invalid cluster response from server")?;

    output::print_result(&value, json_mode, |_| {
        println!(
            "Cluster '{}' creation accepted (status: {}).",
            created.name,
            format_phase(&created.status.phase),
        );
        println!(
            "Run `rtree cluster status {}` to check provisioning progress.",
            created.name
        );
    });
    Ok(())
}

pub struct ClusterCreateOptions<'a> {
    pub organization: Option<&'a str>,
    pub name: &'a str,
    pub replicas: u32,
    pub min_size: ClusterSizeArg,
    pub max_size: Option<ClusterSizeArg>,
    pub idle_timeout_minutes: Option<u64>,
    pub s3_storage: S3StorageArgs,
}

pub fn update(
    client: &ApiClient,
    name_or_id: &str,
    organization: Option<&str>,
    name: Option<&str>,
    idle_timeout_minutes: Option<u64>,
    json_mode: bool,
) -> Result<()> {
    if name.is_none() && idle_timeout_minutes.is_none() {
        anyhow::bail!(
            "At least one setting is required. Pass --name and/or --idle-timeout-minutes."
        );
    }

    let (_, response) = load_dedicated_clusters(client, organization)?;
    let cluster = resolve_cluster(&response.clusters, name_or_id)?;
    let mut body = serde_json::Map::new();
    if let Some(name) = name {
        body.insert("name".to_string(), json!(name));
    }
    if let Some(idle_timeout_minutes) = idle_timeout_minutes {
        body.insert(
            "idle_timeout_minutes".to_string(),
            json!(idle_timeout_minutes),
        );
    }

    let value: Value = client.patch(
        &cluster_path(&cluster.id, None, organization),
        &Value::Object(body),
    )?;
    let updated: ClusterItem =
        serde_json::from_value(value.clone()).context("invalid cluster response from server")?;
    if name.is_some() {
        let mut cfg = config::load()?;
        cfg.default_cluster = renamed_default_cluster(
            cfg.default_cluster.as_deref(),
            &cluster.name,
            &cluster.id,
            &updated.name,
        );
        config::save(&cfg)?;
    }

    output::print_result(&value, json_mode, |_| {
        println!("Cluster '{}' settings updated.", updated.name);
        println!(
            "Idle timeout: {}",
            format_idle_timeout(updated.idle_timeout_minutes)
        );
    });
    Ok(())
}

pub fn list(client: &ApiClient, organization: Option<&str>, json_mode: bool) -> Result<()> {
    let (value, resp) = load_dedicated_clusters(client, organization)?;

    output::print_result(&value, json_mode, |_| {
        if resp.clusters.is_empty() {
            println!(
                "No dedicated clusters found for organization '{}'.",
                resp.organization.name
            );
            return;
        }

        let mut table = new_cli_table();
        table.set_header(vec![
            "cluster",
            "status",
            "replicas",
            "size / replica",
            "idle timeout",
            "storage",
            "created",
            "id",
        ]);
        for cluster in &resp.clusters {
            let replicas = cluster
                .resources
                .as_ref()
                .map(|resources| resources.replicas.to_string())
                .unwrap_or_else(|| "—".to_string());
            table.add_row(vec![
                Cell::new(&cluster.name),
                Cell::new(format_phase(&cluster.status.phase)),
                Cell::new(replicas).set_alignment(CellAlignment::Right),
                Cell::new(format_size_per_replica(cluster.resources.as_ref())),
                Cell::new(format_idle_timeout(cluster.idle_timeout_minutes)),
                Cell::new(format_storage(cluster.s3_storage.as_ref())),
                Cell::new(format_created_at(&cluster.created_at)),
                Cell::new(&cluster.id),
            ]);
        }
        println!("{table}");
    });

    Ok(())
}

pub fn sizes(client: &ApiClient, json_mode: bool) -> Result<()> {
    let (value, resp) = load_cluster_sizes(client)?;

    output::print_result(&value, json_mode, |_| {
        if resp.sizes.is_empty() {
            println!("No dedicated cluster sizes are currently available.");
            return;
        }

        let mut table = new_cli_table();
        table.set_header(vec!["Size", "CPU cores", "Memory (GiB)"]);
        for size in &resp.sizes {
            let size_arg = ClusterSizeArg {
                cpu_cores: size.cpu_cores,
                memory_gib: size.memory_gib,
            };
            table.add_row(vec![
                Cell::new(size_arg.to_string()),
                Cell::new(size.cpu_cores.to_string()).set_alignment(CellAlignment::Right),
                Cell::new(size.memory_gib.to_string()).set_alignment(CellAlignment::Right),
            ]);
        }
        println!("Available dedicated cluster sizes:");
        println!("{table}");
        println!();
        println!(
            "Use `rtree cluster create --min-size <CPU_CORES:MEMORY_GIB>` with one of these sizes."
        );
    });
    Ok(())
}

pub fn use_cluster(name: &str, json_mode: bool) -> Result<()> {
    let mut cfg = config::load()?;
    cfg.default_cluster = Some(name.to_string());
    config::save(&cfg)?;

    output::print_result(&json!({"default_cluster": name}), json_mode, |_| {
        println!("Default cluster set to '{}'.", name)
    });
    Ok(())
}

pub fn status(
    client: &ApiClient,
    name_or_id: &str,
    organization: Option<&str>,
    json_mode: bool,
) -> Result<()> {
    let (_, resp) = load_dedicated_clusters(client, organization)?;
    let cluster = resolve_cluster(&resp.clusters, name_or_id)?;

    output::print_result(cluster, json_mode, |cluster| {
        println!("Cluster: {}", cluster.name);
        println!("ID: {}", cluster.id);
        println!("Status: {}", format_phase(&cluster.status.phase));
        println!("Ready: {}", if cluster.status.ready { "yes" } else { "no" });
        println!(
            "Idle timeout: {}",
            format_idle_timeout(cluster.idle_timeout_minutes)
        );
        println!("Storage: {}", format_storage(cluster.s3_storage.as_ref()));
        if let Some(message) = cluster.status.message.as_deref() {
            println!("Message: {message}");
        }
    });
    Ok(())
}

pub fn stop(
    client: &ApiClient,
    name_or_id: &str,
    organization: Option<&str>,
    json_mode: bool,
) -> Result<()> {
    request_lifecycle(
        client,
        name_or_id,
        organization,
        LifecycleAction::Stop,
        json_mode,
    )
}

pub fn resume(
    client: &ApiClient,
    name_or_id: &str,
    organization: Option<&str>,
    json_mode: bool,
) -> Result<()> {
    request_lifecycle(
        client,
        name_or_id,
        organization,
        LifecycleAction::Resume,
        json_mode,
    )
}

pub fn delete(
    client: &ApiClient,
    name_or_id: &str,
    organization: Option<&str>,
    json_mode: bool,
) -> Result<()> {
    let (_, resp) = load_dedicated_clusters(client, organization)?;
    let cluster = resolve_cluster(&resp.clusters, name_or_id)?.clone();
    let path = cluster_path(&cluster.id, None, organization);
    let result: DeleteClusterResponse = client.delete(&path)?;
    let value = delete_output(&cluster, result.deleted);
    if result.deleted {
        let mut cfg = config::load()?;
        cfg.default_cluster = default_cluster_after_delete(
            cfg.default_cluster.as_deref(),
            &cluster.name,
            &cluster.id,
        );
        config::save(&cfg)?;
    }

    output::print_result(&value, json_mode, |_| {
        if result.deleted {
            println!("Delete request accepted for cluster '{}'.", cluster.name);
            println!(
                "The cluster is removed from status listings while infrastructure cleanup continues asynchronously."
            );
        }
    });
    Ok(())
}

fn delete_output(cluster: &ClusterItem, deleted: bool) -> Value {
    json!({
        "deleted": deleted,
        "id": cluster.id,
        "name": cluster.name,
    })
}

fn load_dedicated_clusters(
    client: &ApiClient,
    organization: Option<&str>,
) -> Result<(Value, ListClustersResponse)> {
    let path = clusters_collection_path(organization);
    let value: Value = client.get(&path)?;
    let resp =
        serde_json::from_value(value.clone()).context("invalid clusters response from server")?;
    Ok((value, resp))
}

fn load_cluster_sizes(client: &ApiClient) -> Result<(Value, ClusterSizesResponse)> {
    let value: Value = client
        .get("/v1/clusters/sizes")
        .context("invalid cluster sizes response from server")?;
    let response: ClusterSizesResponse = serde_json::from_value(value.clone())
        .context("invalid cluster sizes response from server")?;
    Ok((value, response))
}

fn resolve_cluster_size(
    sizes: &[ClusterSizeItem],
    requested: ClusterSizeArg,
) -> Result<(usize, ClusterSizeArg)> {
    let Some((index, size)) = sizes.iter().enumerate().find(|(_, size)| {
        size.cpu_cores == requested.cpu_cores && size.memory_gib == requested.memory_gib
    }) else {
        let available = sizes
            .iter()
            .map(|size| format!("{}:{}", size.cpu_cores, size.memory_gib))
            .collect::<Vec<_>>()
            .join(", ");
        anyhow::bail!(
            "Unsupported cluster size {}. Available sizes: {}",
            requested,
            if available.is_empty() {
                "none".to_string()
            } else {
                available
            }
        );
    };
    Ok((
        index,
        ClusterSizeArg {
            cpu_cores: size.cpu_cores,
            memory_gib: size.memory_gib,
        },
    ))
}

fn cluster_size_json(size: ClusterSizeArg) -> Value {
    json!({
        "cpu_cores": size.cpu_cores,
        "memory_gib": size.memory_gib,
    })
}

fn create_request_body(
    name: &str,
    replicas: u32,
    min_size: ClusterSizeArg,
    max_size: ClusterSizeArg,
    idle_timeout_minutes: Option<u64>,
    s3_storage: Option<Value>,
) -> Value {
    let mut body = json!({
        "name": name,
        "replicas": replicas,
        "size": cluster_size_json(min_size),
        "autoscaling": {
            "min_size": cluster_size_json(min_size),
            "max_size": cluster_size_json(max_size),
        },
    });
    if let Some(idle_timeout_minutes) = idle_timeout_minutes {
        body["idle_timeout_minutes"] = json!(idle_timeout_minutes);
    }
    if let Some(s3_storage) = s3_storage {
        body["s3_storage"] = s3_storage;
    }
    body
}

fn resolve_cluster<'a>(clusters: &'a [ClusterItem], name_or_id: &str) -> Result<&'a ClusterItem> {
    clusters
        .iter()
        .find(|cluster| cluster.name == name_or_id || cluster.id == name_or_id)
        .ok_or_else(|| {
            output::coded_error(
                "cluster_not_found",
                format!("Dedicated cluster '{name_or_id}' not found."),
                4,
            )
        })
}

fn request_lifecycle(
    client: &ApiClient,
    name_or_id: &str,
    organization: Option<&str>,
    action: LifecycleAction,
    json_mode: bool,
) -> Result<()> {
    let (_, resp) = load_dedicated_clusters(client, organization)?;
    let cluster = resolve_cluster(&resp.clusters, name_or_id)?;
    let path = cluster_path(&cluster.id, Some(action.path_segment()), organization);
    let value: Value = client.post_empty(&path)?;
    let updated: ClusterItem =
        serde_json::from_value(value.clone()).context("invalid cluster response from server")?;

    output::print_result(&value, json_mode, |_| {
        println!(
            "{} request accepted for cluster '{}' (status: {}).",
            action.label(),
            updated.name,
            format_phase(&updated.status.phase),
        );
        println!(
            "Run `rtree cluster status {}` to check progress.",
            updated.name
        );
    });
    Ok(())
}

fn clusters_collection_path(organization: Option<&str>) -> String {
    match organization {
        Some(name) => format!("/v1/clusters?organization={}", urlencoding::encode(name)),
        None => "/v1/clusters".to_string(),
    }
}

fn cluster_path(cluster_id: &str, action: Option<&str>, organization: Option<&str>) -> String {
    let mut path = format!("/v1/clusters/{}", urlencoding::encode(cluster_id));
    if let Some(action) = action {
        path.push('/');
        path.push_str(action);
    }
    if let Some(name) = organization {
        path.push_str("?organization=");
        path.push_str(&urlencoding::encode(name));
    }
    path
}

fn format_phase(phase: &str) -> String {
    phase.replace('_', " ")
}

fn format_idle_timeout(minutes: u64) -> String {
    if minutes == 0 {
        "disabled".to_string()
    } else {
        format!("{minutes} minutes")
    }
}

fn format_storage(storage: Option<&S3StorageMetadata>) -> &'static str {
    if storage.is_some() {
        "customer-owned S3"
    } else {
        "cluster default"
    }
}

fn format_created_at(created_at: &str) -> String {
    DateTime::parse_from_str(created_at, "%Y-%m-%d %H:%M:%S%.f%#z")
        .map(|timestamp| {
            timestamp
                .with_timezone(&Utc)
                .format("%Y-%m-%d %H:%M:%S UTC")
                .to_string()
        })
        .unwrap_or_else(|_| created_at.to_string())
}

fn format_size_per_replica(resources: Option<&ClusterResources>) -> String {
    let Some((cpu, memory_bytes)) = resources.and_then(|resources| {
        Some((
            resources.cpu_cores_per_replica?,
            resources.memory_bytes_per_replica?,
        ))
    }) else {
        return "—".to_string();
    };

    let memory_gib = memory_bytes as f64 / 1024_f64.powi(3);
    format!(
        "{} CPU / {} GiB",
        compact_number(cpu),
        compact_number(memory_gib)
    )
}

fn compact_number(value: f64) -> String {
    if value.fract() == 0.0 {
        format!("{value:.0}")
    } else {
        format!("{value:.1}")
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::{
        cluster_path, cluster_size_json, clusters_collection_path, create_request_body,
        default_cluster_after_delete, delete_output, format_created_at, format_idle_timeout,
        format_phase, format_size_per_replica, format_storage, renamed_default_cluster,
        resolve_cluster, resolve_cluster_size, ClusterItem, ClusterResources, ClusterSizeItem,
        ClusterStatus,
    };
    use crate::cli::{ClusterSizeArg, S3StorageArgs};

    #[test]
    fn collection_path_encodes_organization() {
        assert_eq!(
            clusters_collection_path(Some("team alpha")),
            "/v1/clusters?organization=team%20alpha"
        );
        assert_eq!(clusters_collection_path(None), "/v1/clusters");
    }

    #[test]
    fn lifecycle_paths_encode_cluster_and_organization() {
        assert_eq!(
            cluster_path("cluster/id", Some("stop"), Some("team alpha")),
            "/v1/clusters/cluster%2Fid/stop?organization=team%20alpha"
        );
        assert_eq!(
            cluster_path("cluster-id", Some("resume"), None),
            "/v1/clusters/cluster-id/resume"
        );
        assert_eq!(
            cluster_path("cluster-id", None, Some("team alpha")),
            "/v1/clusters/cluster-id?organization=team%20alpha"
        );
    }

    #[test]
    fn cluster_size_catalog_resolves_cpu_and_memory_pairs() {
        let sizes = vec![
            ClusterSizeItem {
                cpu_cores: 2,
                memory_gib: 8,
            },
            ClusterSizeItem {
                cpu_cores: 4,
                memory_gib: 16,
            },
        ];
        assert_eq!(
            resolve_cluster_size(
                &sizes,
                ClusterSizeArg {
                    cpu_cores: 2,
                    memory_gib: 8,
                },
            )
            .expect("valid cluster size"),
            (
                0,
                ClusterSizeArg {
                    cpu_cores: 2,
                    memory_gib: 8,
                }
            )
        );
        let error = resolve_cluster_size(
            &sizes,
            ClusterSizeArg {
                cpu_cores: 1,
                memory_gib: 4,
            },
        )
        .expect_err("unknown size");
        assert!(error.to_string().contains("Available sizes: 2:8, 4:16"));
    }

    #[test]
    fn cluster_size_json_uses_platform_field_names() {
        assert_eq!(
            cluster_size_json(ClusterSizeArg {
                cpu_cores: 2,
                memory_gib: 8,
            }),
            json!({"cpu_cores": 2, "memory_gib": 8})
        );
    }

    #[test]
    fn create_request_uses_minimum_as_initial_size_and_sends_bounds() {
        assert_eq!(
            create_request_body(
                "production",
                2,
                ClusterSizeArg {
                    cpu_cores: 2,
                    memory_gib: 8,
                },
                ClusterSizeArg {
                    cpu_cores: 64,
                    memory_gib: 256,
                },
                Some(30),
                None,
            ),
            json!({
                "name": "production",
                "replicas": 2,
                "size": {"cpu_cores": 2, "memory_gib": 8},
                "autoscaling": {
                    "min_size": {"cpu_cores": 2, "memory_gib": 8},
                    "max_size": {"cpu_cores": 64, "memory_gib": 256}
                },
                "idle_timeout_minutes": 30
            })
        );
    }

    fn cluster() -> ClusterItem {
        ClusterItem {
            id: "11111111-1111-1111-1111-111111111111".to_string(),
            name: "production".to_string(),
            created_at: "2026-07-14 20:38:33.004347+00".to_string(),
            status: ClusterStatus {
                phase: "ready".to_string(),
                ready: true,
                message: None,
            },
            resources: None,
            can_pause: true,
            can_resume: false,
            s3_storage: None,
            idle_timeout_minutes: 15,
        }
    }

    #[test]
    fn resolves_dedicated_cluster_by_name_or_id() {
        let clusters = vec![cluster()];
        assert_eq!(
            resolve_cluster(&clusters, "production")
                .expect("cluster by name")
                .id,
            "11111111-1111-1111-1111-111111111111"
        );
        assert_eq!(
            resolve_cluster(&clusters, "11111111-1111-1111-1111-111111111111")
                .expect("cluster by id")
                .name,
            "production"
        );
        let error = match resolve_cluster(&clusters, "missing") {
            Ok(_) => panic!("missing cluster should fail"),
            Err(error) => error,
        };
        let cli_error = error
            .downcast_ref::<crate::output::CliError>()
            .expect("coded CLI error");
        assert_eq!(cli_error.code(), "cluster_not_found");
        assert_eq!(cli_error.exit_code(), 4);
    }

    #[test]
    fn delete_json_output_identifies_the_cluster() {
        assert_eq!(
            delete_output(&cluster(), true),
            json!({
                "deleted": true,
                "id": "11111111-1111-1111-1111-111111111111",
                "name": "production"
            })
        );
    }

    #[test]
    fn renamed_cluster_updates_matching_default_name_or_id() {
        assert_eq!(
            renamed_default_cluster(Some("production"), "production", "cluster-id", "analytics")
                .as_deref(),
            Some("analytics")
        );
        assert_eq!(
            renamed_default_cluster(Some("cluster-id"), "production", "cluster-id", "analytics")
                .as_deref(),
            Some("analytics")
        );
        assert_eq!(
            renamed_default_cluster(Some("other"), "production", "cluster-id", "analytics")
                .as_deref(),
            Some("other")
        );
    }

    #[test]
    fn deleted_cluster_clears_matching_default_name_or_id() {
        assert_eq!(
            default_cluster_after_delete(Some("production"), "production", "cluster-id"),
            None
        );
        assert_eq!(
            default_cluster_after_delete(Some("cluster-id"), "production", "cluster-id"),
            None
        );
        assert_eq!(
            default_cluster_after_delete(Some("other"), "production", "cluster-id").as_deref(),
            Some("other")
        );
    }

    #[test]
    fn lifecycle_response_fields_deserialize_for_status_output() {
        let cluster: ClusterItem = serde_json::from_value(json!({
            "id": "11111111-1111-1111-1111-111111111111",
            "name": "production",
            "created_at": "2026-07-14 20:38:33.004347+00",
            "status": {
                "phase": "pausing",
                "ready": false,
                "message": "Waiting for active queries to finish."
            },
            "resources": {
                "shards": 1,
                "replicas": 3,
                "cpu_cores_per_replica": 2.0,
                "memory_bytes_per_replica": 8589934592_u64
            },
            "can_pause": false,
            "can_resume": false,
            "idle_timeout_minutes": 15
        }))
        .expect("valid cluster lifecycle response");

        assert_eq!(cluster.status.phase, "pausing");
        assert!(!cluster.status.ready);
        assert_eq!(
            cluster.status.message.as_deref(),
            Some("Waiting for active queries to finish.")
        );
        assert!(!cluster.can_pause);
        assert!(!cluster.can_resume);
        assert_eq!(cluster.idle_timeout_minutes, 15);
    }

    #[test]
    fn cluster_storage_response_deserializes_with_the_new_field_name() {
        let cluster: ClusterItem = serde_json::from_value(json!({
            "id": "cluster-id",
            "name": "production",
            "created_at": "2026-07-14 20:38:33.004347+00",
            "status": {"phase": "ready", "ready": true, "message": null},
            "resources": null,
            "can_pause": true,
            "can_resume": false,
            "s3_storage": {
                "data": {"bucket": "customer-data", "path": "rawtree/data"},
                "backups": {"bucket": "customer-backups", "path": "rawtree/backups"}
            },
            "idle_timeout_minutes": 15
        }))
        .expect("cluster storage response should deserialize");

        assert_eq!(
            format_storage(cluster.s3_storage.as_ref()),
            "customer-owned S3"
        );
        assert_eq!(
            cluster.s3_storage.expect("storage metadata").data.bucket,
            "customer-data"
        );
    }

    #[test]
    fn cluster_create_body_uses_s3_storage() {
        let storage = S3StorageArgs {
            s3_data_bucket: Some("customer-data".to_string()),
            s3_data_path: Some("rawtree/data".to_string()),
            s3_backups_bucket: Some("customer-backups".to_string()),
            s3_backups_path: Some("rawtree/backups".to_string()),
            s3_role_arn: Some("arn:aws:iam::123456789012:role/RawTreeS3Access".to_string()),
            s3_external_id: Some("rawtree-example".to_string()),
        };
        let body = create_request_body(
            "production",
            1,
            ClusterSizeArg {
                cpu_cores: 2,
                memory_gib: 8,
            },
            ClusterSizeArg {
                cpu_cores: 2,
                memory_gib: 8,
            },
            None,
            storage.to_json().expect("valid storage"),
        );

        assert_eq!(
            body["s3_storage"],
            json!({
                "data": {"bucket": "customer-data", "path": "rawtree/data"},
                "backups": {"bucket": "customer-backups", "path": "rawtree/backups"},
                "role_arn": "arn:aws:iam::123456789012:role/RawTreeS3Access",
                "external_id": "rawtree-example"
            })
        );
    }

    #[test]
    fn timestamp_is_rendered_to_seconds_in_utc() {
        assert_eq!(
            format_created_at("2026-07-14 20:38:33.004347+00"),
            "2026-07-14 20:38:33 UTC"
        );
        assert_eq!(
            format_created_at("2026-07-14 22:38:33+02"),
            "2026-07-14 20:38:33 UTC"
        );
    }

    #[test]
    fn resources_are_formatted_per_replica() {
        let resources = ClusterResources {
            shards: 1,
            replicas: 3,
            cpu_cores_per_replica: Some(2.0),
            memory_bytes_per_replica: Some(8 * 1024 * 1024 * 1024),
        };
        assert_eq!(format_size_per_replica(Some(&resources)), "2 CPU / 8 GiB");
        assert_eq!(format_size_per_replica(None), "—");
    }

    #[test]
    fn idle_timeout_is_human_readable() {
        assert_eq!(format_idle_timeout(0), "disabled");
        assert_eq!(format_idle_timeout(30), "30 minutes");
    }

    #[test]
    fn lifecycle_phase_is_human_readable() {
        assert_eq!(format_phase("rolling_rawtree"), "rolling rawtree");
    }
}

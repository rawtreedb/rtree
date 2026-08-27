use anyhow::Result;
use serde::{Deserialize, Serialize};

use crate::client::ApiClient;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OrganizationItem {
    pub name: String,
    pub role: String,
}

#[derive(Debug, Deserialize)]
struct ListOrganizationsResponse {
    organizations: Vec<OrganizationItem>,
}

pub fn list_organizations(client: &ApiClient) -> Result<Vec<OrganizationItem>> {
    let resp: ListOrganizationsResponse = client.get("/v1/organizations")?;
    Ok(resp.organizations)
}

pub fn first_organization_name(client: &ApiClient) -> Option<String> {
    list_organizations(client)
        .ok()?
        .into_iter()
        .next()
        .map(|org| org.name)
}

pub fn databases_collection_path(organization: Option<&str>, cluster: Option<&str>) -> String {
    scoped_path("/v1/databases", organization, cluster)
}

fn append_scope_params(path: &mut String, organization: Option<&str>, cluster: Option<&str>) {
    if let Some(org) = organization {
        path.push_str("&organization=");
        path.push_str(&urlencoding::encode(org));
    }
    if let Some(cluster) = cluster {
        path.push_str("&cluster=");
        path.push_str(&urlencoding::encode(cluster));
    }
}

pub fn scoped_path(path: &str, organization: Option<&str>, cluster: Option<&str>) -> String {
    let mut scoped = path.to_string();
    let mut params = Vec::new();
    if let Some(org) = organization {
        params.push(format!("organization={}", urlencoding::encode(org)));
    }
    if let Some(cluster) = cluster {
        params.push(format!("cluster={}", urlencoding::encode(cluster)));
    }
    if !params.is_empty() {
        scoped.push(if path.contains('?') { '&' } else { '?' });
        scoped.push_str(&params.join("&"));
    }
    scoped
}

pub fn database_scoped_path(
    database: &str,
    suffix: &str,
    organization: Option<&str>,
    cluster: Option<&str>,
) -> String {
    let normalized_suffix = if suffix.is_empty() {
        String::new()
    } else if suffix.starts_with('/') {
        suffix.to_string()
    } else {
        format!("/{suffix}")
    };

    let sep = if normalized_suffix.contains('?') {
        '&'
    } else {
        '?'
    };
    let mut path = format!(
        "/v1{normalized_suffix}{sep}database={}",
        urlencoding::encode(database)
    );
    append_scope_params(&mut path, organization, cluster);
    path
}

#[cfg(test)]
mod tests {
    use super::{database_scoped_path, databases_collection_path, scoped_path};

    #[test]
    fn databases_collection_path_uses_organization_filter() {
        assert_eq!(
            databases_collection_path(Some("team alpha"), None),
            "/v1/databases?organization=team%20alpha"
        );
        assert_eq!(databases_collection_path(None, None), "/v1/databases");
        assert_eq!(
            databases_collection_path(Some("team alpha"), Some("prod/eu")),
            "/v1/databases?organization=team%20alpha&cluster=prod%2Feu"
        );
    }

    #[test]
    fn database_scoped_path_adds_database_query_param() {
        let path = database_scoped_path("analytics", "/query", None, None);
        assert_eq!(path, "/v1/query?database=analytics");
    }

    #[test]
    fn database_scoped_path_adds_organization_query_param() {
        let path = database_scoped_path(
            "analytics",
            "/query",
            Some("team_alpha"),
            Some("production"),
        );
        assert_eq!(
            path,
            "/v1/query?database=analytics&organization=team_alpha&cluster=production"
        );
    }

    #[test]
    fn database_scoped_path_accepts_suffix_without_leading_slash() {
        let path = database_scoped_path("analytics", "tables", Some("team_alpha"), None);
        assert_eq!(
            path,
            "/v1/tables?database=analytics&organization=team_alpha"
        );
    }

    #[test]
    fn database_scoped_path_appends_to_existing_query() {
        let path =
            database_scoped_path("analytics db", "/tables/events?url=x", Some("team a"), None);
        assert_eq!(
            path,
            "/v1/tables/events?url=x&database=analytics%20db&organization=team%20a"
        );
    }

    #[test]
    fn scoped_path_adds_cluster_without_organization() {
        assert_eq!(
            scoped_path("/v1/databases", None, Some("production")),
            "/v1/databases?cluster=production"
        );
    }
}

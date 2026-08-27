use anyhow::Result;
use serde_json::json;

use crate::commands::open;
use crate::config;
use crate::output;

fn build_login_url(base_url: &str) -> String {
    format!("{}/login", base_url.trim_end_matches('/'))
}

fn resolve_dashboard_url(
    base_url: &str,
    authenticated: bool,
    organization: Option<&str>,
    cluster: Option<&str>,
    database: Option<&str>,
) -> Option<String> {
    if authenticated {
        return Some(open::build_open_url(
            base_url,
            organization,
            cluster,
            database,
        ));
    }
    Some(build_login_url(base_url))
}

pub fn status(resolved_url: &str, json_mode: bool) -> Result<()> {
    let cfg = config::load()?;
    let authenticated = cfg.token.is_some();
    let user = cfg.email.clone();
    let database = cfg.default_database.clone();
    let organization = cfg.default_organization.clone();
    let cluster = cfg.default_cluster.clone();
    let ui_base_url = open::resolve_ui_base_url();
    let dashboard_url = resolve_dashboard_url(
        &ui_base_url,
        authenticated,
        organization.as_deref(),
        cluster.as_deref(),
        database.as_deref(),
    );

    output::print_result(
        &json!({
            "authenticated": authenticated,
            "user": user.as_deref(),
            "database": database.as_deref(),
            "organization": organization.as_deref(),
            "cluster": cluster.as_deref(),
            "api_url": resolved_url,
            "dashboard_url": dashboard_url.as_deref(),
        }),
        json_mode,
        |_| {
            println!("API URL: {}", resolved_url);
            println!("Authenticated: {}", authenticated);
            println!("User: {}", user.as_deref().unwrap_or("-"));
            println!("Database: {}", database.as_deref().unwrap_or("-"));
            println!("Organization: {}", organization.as_deref().unwrap_or("-"));
            println!("Cluster: {}", cluster.as_deref().unwrap_or("-"));
            println!("Dashboard URL: {}", dashboard_url.as_deref().unwrap_or("-"));
        },
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{build_login_url, resolve_dashboard_url};

    #[test]
    fn resolve_dashboard_url_prefers_normal_dashboard_when_authenticated() {
        let url = resolve_dashboard_url(
            "https://rawtree.com",
            true,
            Some("team"),
            Some("production"),
            Some("analytics"),
        );
        assert_eq!(
            url.as_deref(),
            Some("https://rawtree.com/team/production?database=analytics")
        );
    }

    #[test]
    fn resolve_dashboard_url_uses_login_when_not_authenticated() {
        let url = resolve_dashboard_url(
            "https://rawtree.com",
            false,
            Some("team"),
            Some("production"),
            Some("analytics"),
        );
        assert_eq!(url.as_deref(), Some("https://rawtree.com/login"));
    }

    #[test]
    fn resolve_dashboard_url_uses_login_when_not_authenticated_without_database_context() {
        let url = resolve_dashboard_url("https://rawtree.com", false, None, None, None);
        assert_eq!(url.as_deref(), Some("https://rawtree.com/login"));
    }

    #[test]
    fn build_login_url_appends_login_path() {
        let url = build_login_url("https://rawtree.com/");
        assert_eq!(url, "https://rawtree.com/login");
    }
}

mod cli;
mod client;
mod commands;
mod config;
mod constants;
mod org;
mod output;

use std::io::{self, IsTerminal, Read};

use anyhow::Result;
use clap::{CommandFactory, Parser};
use clap_complete::generate;

use cli::{
    Cli, ClusterCommand, Command, DatabaseCommand, KeyCommand, OrganizationCommand, ShellType,
    TableCommand,
};
use client::ApiClient;
use constants::DEFAULT_API_URL;

fn resolve_url_from_sources(
    cli_url: Option<&str>,
    env_api_url: Option<String>,
    cfg_url: Option<String>,
) -> String {
    cli_url
        .map(str::to_string)
        .or(env_api_url)
        .or(cfg_url)
        .unwrap_or_else(|| DEFAULT_API_URL.to_string())
}

fn resolve_url(cli_url: Option<&str>) -> String {
    let env_api_url = std::env::var("RAWTREE_API_URL").ok();
    let cfg_url = config::load().ok().and_then(|c| c.url);
    resolve_url_from_sources(cli_url, env_api_url, cfg_url)
}

fn resolve_token_from_sources(
    cli_api_key: Option<String>,
    env_api_key: Option<String>,
    cfg_token: Option<String>,
) -> Option<String> {
    cli_api_key.or(env_api_key).or(cfg_token)
}

fn resolve_token(cli_api_key: Option<String>) -> Option<String> {
    let env_api_key = std::env::var("RAWTREE_API_KEY").ok();
    let cfg_token = config::load().ok().and_then(|c| c.token);
    resolve_token_from_sources(cli_api_key, env_api_key, cfg_token)
}

fn resolve_database_from_sources(
    cli_database: Option<String>,
    env_database: Option<String>,
    cfg_database: Option<String>,
) -> Option<String> {
    cli_database.or(env_database).or(cfg_database)
}

fn resolve_optional_database(cli_database: Option<String>) -> Option<String> {
    let env_database = std::env::var("RAWTREE_DATABASE").ok();
    let cfg_database = config::load().ok().and_then(|c| c.default_database);
    resolve_database_from_sources(cli_database, env_database, cfg_database)
}

fn resolve_database(cli_database: Option<String>) -> Result<String> {
    resolve_optional_database(cli_database).ok_or_else(|| {
        anyhow::anyhow!(
            "No database specified. Use --database, RAWTREE_DATABASE env, or `rtree database use <name>`"
        )
    })
}

fn resolve_org_from_sources(
    cli_org: Option<String>,
    env_org: Option<String>,
    cfg_org: Option<String>,
) -> Option<String> {
    cli_org.or(env_org).or(cfg_org)
}

fn resolve_explicit_org(cli_org: Option<String>) -> Option<String> {
    let env_org = std::env::var("RAWTREE_ORG").ok();
    let cfg_org = config::load().ok().and_then(|c| c.default_organization);
    resolve_org_from_sources(cli_org, env_org, cfg_org)
}

fn resolve_effective_org_with<F>(
    explicit_org: Option<String>,
    fetch_default_org: F,
) -> Option<String>
where
    F: FnOnce() -> Option<String>,
{
    explicit_org.or_else(fetch_default_org)
}

fn resolve_effective_org(client: &ApiClient, cli_org: Option<String>) -> Option<String> {
    let explicit_org = resolve_explicit_org(cli_org);
    resolve_effective_org_with(explicit_org, || org::first_organization_name(client))
}

fn read_stdin() -> Result<String> {
    let mut buf = String::new();
    io::stdin().read_to_string(&mut buf)?;
    let trimmed = buf.trim().to_string();
    if trimmed.is_empty() {
        anyhow::bail!("no SQL provided on stdin");
    }
    Ok(trimmed)
}

fn resolve_sql(positional: Option<String>, flag: Option<String>) -> Result<String> {
    let raw = positional.or(flag);
    match raw {
        Some(s) if s == "-" => read_stdin(),
        Some(s) => Ok(s),
        None => {
            if !io::stdin().is_terminal() {
                read_stdin()
            } else {
                anyhow::bail!("SQL query is required. Provide it as a positional argument, with --sql, or pipe via stdin")
            }
        }
    }
}

fn main() {
    let cli = Cli::parse();
    let json_mode = cli.json;
    if let Err(e) = run(cli) {
        let code = output::print_error(&e, json_mode);
        std::process::exit(code);
    }
}

fn prompt_password_if_missing(password: Option<String>) -> Result<String> {
    match password {
        Some(p) => Ok(p),
        None => {
            let p = rpassword::prompt_password("Password: ")?;
            if p.is_empty() {
                anyhow::bail!("password cannot be empty");
            }
            Ok(p)
        }
    }
}

fn should_prompt_for_login_method(
    email: Option<&str>,
    json_mode: bool,
    stdin_is_terminal: bool,
    stdout_is_terminal: bool,
) -> bool {
    email.is_none() && !json_mode && stdin_is_terminal && stdout_is_terminal
}

fn run(cli: Cli) -> Result<()> {
    let Cli {
        api_key: cli_api_key,
        api_url: cli_url,
        json,
        org: cli_org,
        command,
    } = cli;

    let url = resolve_url(cli_url.as_deref());
    let token = resolve_token(cli_api_key.clone());
    let client = ApiClient::new(url.clone(), token);

    match command {
        Command::Login {
            email,
            password,
            no_browser,
            timeout_seconds,
            database,
        } => {
            if let Some(api_key) = cli_api_key {
                if email.is_some() || password.is_some() {
                    anyhow::bail!(
                        "--api-key cannot be used with --email or --password during login"
                    );
                }
                commands::auth::login_with_api_key(
                    &client,
                    &api_key,
                    cli_org.clone(),
                    database,
                    json,
                )
            } else if should_prompt_for_login_method(
                email.as_deref(),
                json,
                io::stdin().is_terminal(),
                io::stdout().is_terminal(),
            ) {
                match commands::auth::prompt_for_login_method()? {
                    commands::auth::LoginMethod::Rawtree => commands::auth::login_with_browser(
                        &client,
                        no_browser,
                        timeout_seconds,
                        cli_org.clone(),
                        database,
                        json,
                    ),
                    commands::auth::LoginMethod::ManualApiKey => {
                        let api_key = commands::auth::prompt_for_api_key()?;
                        commands::auth::login_with_api_key(
                            &client,
                            &api_key,
                            cli_org.clone(),
                            database,
                            json,
                        )
                    }
                }
            } else if let Some(email) = email {
                let password = prompt_password_if_missing(password)?;
                commands::auth::login(&client, &email, &password, cli_org.clone(), database, json)
            } else {
                commands::auth::login_with_browser(
                    &client,
                    no_browser,
                    timeout_seconds,
                    cli_org.clone(),
                    database,
                    json,
                )
            }
        }
        Command::Logout => commands::auth::logout(json),
        Command::Database { action } => match action {
            DatabaseCommand::List => {
                let effective_org = resolve_effective_org(&client, cli_org.clone());
                commands::database::list(&client, effective_org.as_deref(), json)
            }
            DatabaseCommand::Create { name } => {
                let effective_org = resolve_effective_org(&client, cli_org.clone());
                commands::database::create(&client, &name, effective_org.as_deref(), json)
            }
            DatabaseCommand::Use { name } => commands::database::use_database(&name, json),
            DatabaseCommand::Delete { name } => {
                let effective_org = resolve_effective_org(&client, cli_org.clone());
                commands::database::delete(&client, &name, effective_org.as_deref(), json)
            }
        },
        Command::Organization { action } => match action {
            OrganizationCommand::List => commands::organization::list(&client, json),
            OrganizationCommand::Create { name } => {
                commands::organization::create(&client, &name, json)
            }
            OrganizationCommand::Use { name } => {
                commands::organization::use_organization(&name, json)
            }
            OrganizationCommand::Rename { old, new_name } => {
                commands::organization::rename(&client, &old, &new_name, json)
            }
            OrganizationCommand::Delete { name } => {
                commands::organization::delete(&client, &name, json)
            }
        },
        Command::Cluster { action } => {
            let effective_org = resolve_effective_org(&client, cli_org.clone());
            match action {
                ClusterCommand::List => {
                    commands::cluster::list(&client, effective_org.as_deref(), json)
                }
                ClusterCommand::Status { name_or_id } => {
                    commands::cluster::status(&client, &name_or_id, effective_org.as_deref(), json)
                }
                ClusterCommand::Stop { name_or_id } => {
                    commands::cluster::stop(&client, &name_or_id, effective_org.as_deref(), json)
                }
                ClusterCommand::Resume { name_or_id } => {
                    commands::cluster::resume(&client, &name_or_id, effective_org.as_deref(), json)
                }
                ClusterCommand::Delete { name_or_id } => {
                    commands::cluster::delete(&client, &name_or_id, effective_org.as_deref(), json)
                }
            }
        }
        Command::Key { action } => {
            let effective_org = resolve_effective_org(&client, cli_org.clone());
            match action {
                KeyCommand::List { database } => {
                    let database = resolve_database(database)?;
                    commands::keys::list(&client, &database, effective_org.as_deref(), json)
                }
                KeyCommand::Create {
                    database,
                    name,
                    permission,
                } => {
                    let database = resolve_database(database)?;
                    commands::keys::create(
                        &client,
                        &database,
                        effective_org.as_deref(),
                        &name,
                        &permission,
                        json,
                    )
                }
                KeyCommand::Delete {
                    database,
                    id_or_token,
                } => {
                    let database = resolve_database(database)?;
                    commands::keys::delete(
                        &client,
                        &database,
                        effective_org.as_deref(),
                        &id_or_token,
                        json,
                    )
                }
            }
        }
        Command::Table { action } => {
            let effective_org = resolve_effective_org(&client, cli_org.clone());
            match action {
                TableCommand::List { database } => {
                    let database = resolve_database(database)?;
                    commands::table::list(&client, &database, effective_org.as_deref(), json)
                }
                TableCommand::Describe { database, table } => {
                    let database = resolve_database(database)?;
                    commands::table::describe(
                        &client,
                        &database,
                        effective_org.as_deref(),
                        &table,
                        json,
                    )
                }
            }
        }
        Command::Logs {
            database,
            r#type,
            table,
            status,
            limit,
            offset,
            since,
            until,
            start_time,
            end_time,
        } => {
            let effective_org = resolve_effective_org(&client, cli_org.clone());
            let database = resolve_database(database)?;
            commands::logs::logs(
                &client,
                &database,
                effective_org.as_deref(),
                r#type.as_deref(),
                &table,
                status.as_deref(),
                limit,
                offset,
                since.as_deref(),
                until.as_deref(),
                start_time.as_deref(),
                end_time.as_deref(),
                json,
            )
        }
        Command::Query {
            database,
            sql_positional,
            sql,
            limit,
        } => {
            let effective_org = resolve_effective_org(&client, cli_org.clone());
            let database = resolve_database(database)?;
            let sql = resolve_sql(sql_positional, sql)?;
            commands::query::query(
                &client,
                &database,
                effective_org.as_deref(),
                &sql,
                limit,
                json,
            )
        }
        Command::Insert {
            database,
            table,
            data,
            file,
            url,
            transform,
        } => {
            let effective_org = resolve_effective_org(&client, cli_org.clone());
            let database = resolve_database(database)?;

            commands::insert::insert(
                &client,
                &database,
                effective_org.as_deref(),
                &table,
                data.as_deref(),
                file.as_deref(),
                url.as_deref(),
                transform.as_deref(),
                json,
            )
        }
        Command::Ping => commands::ping::ping(&client, json),
        Command::Docs => commands::docs::docs(&client),
        Command::Status => commands::status::status(&url, json),
        Command::Open { database } => {
            let ui_base_url = commands::open::resolve_ui_base_url();
            let effective_org = resolve_effective_org(&client, cli_org);
            let database = resolve_optional_database(database);
            commands::open::open(
                &ui_base_url,
                effective_org.as_deref(),
                database.as_deref(),
                json,
            )
        }
        Command::Completions { shell } => {
            let shell = match shell {
                ShellType::Bash => clap_complete::Shell::Bash,
                ShellType::Zsh => clap_complete::Shell::Zsh,
                ShellType::Fish => clap_complete::Shell::Fish,
            };
            generate(shell, &mut Cli::command(), "rtree", &mut io::stdout());
            Ok(())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::constants::DEFAULT_API_URL;
    use super::{
        resolve_database_from_sources, resolve_effective_org_with, resolve_org_from_sources,
        resolve_token_from_sources, resolve_url_from_sources, should_prompt_for_login_method,
    };

    #[test]
    fn resolve_org_uses_cli_first() {
        let resolved = resolve_org_from_sources(
            Some("cli-org".to_string()),
            Some("env-org".to_string()),
            Some("cfg-org".to_string()),
        );
        assert_eq!(resolved.as_deref(), Some("cli-org"));
    }

    #[test]
    fn resolve_org_uses_env_when_cli_missing() {
        let resolved = resolve_org_from_sources(
            None,
            Some("env-org".to_string()),
            Some("cfg-org".to_string()),
        );
        assert_eq!(resolved.as_deref(), Some("env-org"));
    }

    #[test]
    fn resolve_org_uses_config_when_cli_and_env_missing() {
        let resolved = resolve_org_from_sources(None, None, Some("cfg-org".to_string()));
        assert_eq!(resolved.as_deref(), Some("cfg-org"));
    }

    #[test]
    fn resolve_org_returns_none_when_no_sources() {
        let resolved = resolve_org_from_sources(None, None, None);
        assert_eq!(resolved, None);
    }

    #[test]
    fn explicit_org_wins_over_auto_default_fetch() {
        let resolved = resolve_effective_org_with(Some("explicit-org".to_string()), || {
            Some("fetched-org".to_string())
        });
        assert_eq!(resolved.as_deref(), Some("explicit-org"));
    }

    #[test]
    fn auto_default_org_is_used_when_explicit_is_missing() {
        let resolved = resolve_effective_org_with(None, || Some("fetched-org".to_string()));
        assert_eq!(resolved.as_deref(), Some("fetched-org"));
    }

    #[test]
    fn auto_default_org_can_fall_back_to_none() {
        let resolved = resolve_effective_org_with(None, || None);
        assert_eq!(resolved, None);
    }

    #[test]
    fn resolve_url_uses_cli_first() {
        let resolved = resolve_url_from_sources(
            Some("https://cli.example.com"),
            Some("https://api.example.com".to_string()),
            Some("https://cfg.example.com".to_string()),
        );
        assert_eq!(resolved, "https://cli.example.com");
    }

    #[test]
    fn resolve_url_uses_env_when_cli_missing() {
        let resolved = resolve_url_from_sources(
            None,
            Some("https://api.example.com".to_string()),
            Some("https://cfg.example.com".to_string()),
        );
        assert_eq!(resolved, "https://api.example.com");
    }

    #[test]
    fn resolve_url_uses_config_when_cli_and_env_missing() {
        let resolved =
            resolve_url_from_sources(None, None, Some("https://cfg.example.com".to_string()));
        assert_eq!(resolved, "https://cfg.example.com");
    }

    #[test]
    fn resolve_url_defaults_when_no_sources() {
        let resolved = resolve_url_from_sources(None, None, None);
        assert_eq!(resolved, DEFAULT_API_URL);
    }

    #[test]
    fn resolve_token_uses_cli_first() {
        let resolved = resolve_token_from_sources(
            Some("cli-key".to_string()),
            Some("env-key".to_string()),
            Some("cfg-token".to_string()),
        );
        assert_eq!(resolved.as_deref(), Some("cli-key"));
    }

    #[test]
    fn resolve_token_uses_env_when_cli_missing() {
        let resolved = resolve_token_from_sources(
            None,
            Some("env-key".to_string()),
            Some("cfg-token".to_string()),
        );
        assert_eq!(resolved.as_deref(), Some("env-key"));
    }

    #[test]
    fn resolve_token_uses_config_when_cli_and_env_missing() {
        let resolved = resolve_token_from_sources(None, None, Some("cfg-token".to_string()));
        assert_eq!(resolved.as_deref(), Some("cfg-token"));
    }

    #[test]
    fn plain_terminal_login_prompts_for_login_method() {
        assert!(should_prompt_for_login_method(None, false, true, true));
    }

    #[test]
    fn explicit_or_non_interactive_login_skips_login_method_prompt() {
        assert!(!should_prompt_for_login_method(
            Some("user@example.com"),
            false,
            true,
            true
        ));
        assert!(!should_prompt_for_login_method(None, true, true, true));
        assert!(!should_prompt_for_login_method(None, false, false, true));
    }

    #[test]
    fn redirected_stdout_skips_login_method_prompt() {
        assert!(!should_prompt_for_login_method(None, false, true, false));
    }

    #[test]
    fn resolve_database_uses_cli_first() {
        let resolved = resolve_database_from_sources(
            Some("cli-database".to_string()),
            Some("env-database".to_string()),
            Some("cfg-database".to_string()),
        );
        assert_eq!(resolved.as_deref(), Some("cli-database"));
    }

    #[test]
    fn resolve_database_uses_env_when_cli_missing() {
        let resolved = resolve_database_from_sources(
            None,
            Some("env-database".to_string()),
            Some("cfg-database".to_string()),
        );
        assert_eq!(resolved.as_deref(), Some("env-database"));
    }

    #[test]
    fn resolve_database_uses_config_when_cli_and_env_missing() {
        let resolved = resolve_database_from_sources(None, None, Some("cfg-database".to_string()));
        assert_eq!(resolved.as_deref(), Some("cfg-database"));
    }
}

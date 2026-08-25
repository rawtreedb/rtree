use clap::{Parser, Subcommand, ValueEnum};

#[derive(Parser)]
#[command(
    name = "rtree",
    version,
    disable_version_flag = true,
    arg(
        clap::Arg::new("version")
            .short('v')
            .long("version")
            .help("Output the current version")
            .action(clap::ArgAction::Version)
    ),
    about = "CLI for the RawTree analytics platform"
)]
pub struct Cli {
    /// API key (overrides RAWTREE_API_KEY env and config file token)
    #[arg(long, global = true)]
    pub api_key: Option<String>,

    /// API URL (overrides RAWTREE_API_URL env and config file)
    #[arg(long, global = true)]
    pub api_url: Option<String>,

    /// Output results as JSON (for scripting and agents)
    #[arg(long, global = true)]
    pub json: bool,

    /// Organization name (overrides RAWTREE_ORG env and config file)
    #[arg(long, global = true)]
    pub org: Option<String>,

    #[command(subcommand)]
    pub command: Command,
}

#[derive(Subcommand)]
pub enum Command {
    /// Log in and save credentials
    #[command(
        after_help = "API key mode:\n  --api-key saves an API key directly without browser authentication.\n\nAPI key output (--json):\n  {\"success\":true,\"config_path\":\"<path>\",\"database\":\"<name>\",\"organization\":\"<name>\"}"
    )]
    Login {
        #[arg(long, hide = true)]
        email: Option<String>,
        /// Password (prompted interactively if omitted)
        #[arg(long, requires = "email", hide = true)]
        password: Option<String>,
        /// Do not try to open the browser automatically
        #[arg(long, default_value_t = false)]
        no_browser: bool,
        /// Max seconds to wait for browser login approval
        #[arg(long, default_value_t = 300)]
        timeout_seconds: u64,
        /// Database name to set as default after authentication
        #[arg(long)]
        database: Option<String>,
    },
    /// Log out and remove saved local credentials
    Logout,
    /// Manage databases
    Database {
        #[command(subcommand)]
        action: DatabaseCommand,
    },
    /// Manage API keys
    #[command(name = "key")]
    Key {
        #[command(subcommand)]
        action: KeyCommand,
    },
    /// Manage organizations
    Organization {
        #[command(subcommand)]
        action: OrganizationCommand,
    },
    /// Inspect dedicated clusters
    Cluster {
        #[command(subcommand)]
        action: ClusterCommand,
    },
    /// Inspect tables
    Table {
        #[command(subcommand)]
        action: TableCommand,
    },
    /// View API request logs for a database
    Logs {
        #[arg(long)]
        database: Option<String>,
        #[arg(long)]
        search: Option<String>,
        /// Comma-separated HTTP methods (for example GET,POST)
        #[arg(long, value_delimiter = ',')]
        methods: Vec<String>,
        /// Comma-separated HTTP status codes (for example 200,404,500)
        #[arg(long, value_delimiter = ',', value_parser = clap::value_parser!(u16).range(100..=599))]
        status_codes: Vec<u16>,
        /// Comma-separated request levels: success, warning, or error
        #[arg(long, value_delimiter = ',')]
        levels: Vec<String>,
        /// Comma-separated request sources: ui, cli, or api
        #[arg(long, value_delimiter = ',')]
        sources: Vec<String>,
        /// Exact user-agent value
        #[arg(long)]
        user_agent: Option<String>,
        /// Case-insensitive substring of the request host
        #[arg(long)]
        host: Option<String>,
        /// Comma-separated exact request paths
        #[arg(long, value_delimiter = ',')]
        paths: Vec<String>,
        /// Minimum request duration in milliseconds
        #[arg(long)]
        min_duration_ms: Option<u64>,
        /// Maximum request duration in milliseconds
        #[arg(long)]
        max_duration_ms: Option<u64>,
        /// Comma-separated database names to include on a dedicated cluster
        #[arg(long, value_delimiter = ',')]
        log_databases: Vec<String>,
        /// Maximum number of log entries to return (default: 50, max: 200)
        #[arg(long, default_value = "50", value_parser = clap::value_parser!(u64).range(1..=200))]
        limit: u64,
        /// Offset for pagination
        #[arg(long, default_value = "0")]
        offset: u64,
        /// Show logs from the last duration (e.g., 1h, 30m, 7d, 2w)
        #[arg(long, conflicts_with_all = ["start_time", "end_time"])]
        since: Option<String>,
        /// Show logs until this duration ago (e.g., 30m)
        #[arg(long, conflicts_with_all = ["start_time", "end_time"])]
        until: Option<String>,
        /// Start time in UTC (e.g., "2026-03-28T18:00:00Z")
        #[arg(long, conflicts_with_all = ["since", "until"])]
        start_time: Option<String>,
        /// End time in UTC (e.g., "2026-03-28T19:00:00Z")
        #[arg(long, conflicts_with_all = ["since", "until"])]
        end_time: Option<String>,
    },
    /// Execute a SQL query against a database
    Query {
        #[arg(long)]
        database: Option<String>,
        /// SQL query to execute (positional or --sql). Use "-" to read from stdin.
        #[arg(value_name = "SQL", conflicts_with = "sql")]
        sql_positional: Option<String>,
        /// SQL query to execute
        #[arg(long)]
        sql: Option<String>,
        /// Append LIMIT N to the query
        #[arg(long)]
        limit: Option<u64>,
    },
    /// Insert data into a table
    Insert {
        #[arg(long)]
        database: Option<String>,
        #[arg(long)]
        table: String,
        /// Inline JSON data
        #[arg(long, conflicts_with = "file")]
        data: Option<String>,
        /// Path to a JSON or JSONL file
        #[arg(long, conflicts_with = "data")]
        file: Option<String>,
        /// Public URL to JSON or JSONL content
        #[arg(long, conflicts_with_all = ["data", "file"])]
        url: Option<String>,
        /// Apply a predefined transform (e.g., otlp-traces, otlp-logs, otlp-metrics)
        #[arg(long)]
        transform: Option<String>,
    },
    /// Check server connectivity
    Ping,
    /// Fetch and display API documentation from the server
    Docs,
    /// Show current auth state and API URL
    Status,
    /// Open Rawtree UI in your browser
    Open {
        /// Database name (defaults to --database/RAWTREE_DATABASE/config default)
        #[arg(long)]
        database: Option<String>,
    },
    /// Generate shell completions
    Completions {
        /// Shell to generate completions for
        #[arg(value_enum)]
        shell: ShellType,
    },
}

#[derive(Clone, ValueEnum)]
pub enum ShellType {
    Bash,
    Zsh,
    Fish,
}

#[derive(Subcommand)]
pub enum DatabaseCommand {
    /// List all databases
    List,
    /// Create a new database
    Create {
        /// Database name
        name: String,
    },
    /// Set the default database
    Use {
        /// Database name
        name: String,
    },
    /// Delete a database and all its data
    Delete {
        /// Database name
        name: String,
    },
}

#[derive(Subcommand)]
pub enum OrganizationCommand {
    /// List all organizations
    List,
    /// Create a new organization
    Create {
        /// Organization name
        name: String,
    },
    /// Set the default organization
    Use {
        /// Organization name
        name: String,
    },
    /// Rename an organization
    Rename {
        /// Current organization name
        old: String,
        /// New organization name
        new_name: String,
    },
    /// Delete an organization
    Delete {
        /// Organization name
        name: String,
    },
}

#[derive(Subcommand)]
pub enum ClusterCommand {
    /// List dedicated clusters
    List,
    /// Create a dedicated cluster
    Create {
        /// Cluster name
        #[arg(long)]
        name: String,
        /// Number of cluster replicas
        #[arg(long)]
        replicas: u32,
        /// Minimum CPU cores per replica
        #[arg(long, value_name = "CPU_CORES")]
        min_cpu_cores: u32,
        /// Minimum memory GB per replica
        #[arg(long, value_name = "MEMORY_GB")]
        min_memory_gb: u32,
        /// Maximum CPU cores per replica; defaults to the minimum
        #[arg(long, requires = "max_memory_gb", value_name = "CPU_CORES")]
        max_cpu_cores: Option<u32>,
        /// Maximum memory GB per replica; defaults to the minimum
        #[arg(long, requires = "max_cpu_cores", value_name = "MEMORY_GB")]
        max_memory_gb: Option<u32>,
        /// Minutes without activity before automatically pausing; 0 disables idling
        #[arg(long)]
        idle_timeout_minutes: Option<u64>,
    },
    /// Show the current state of a dedicated cluster
    Status {
        /// Cluster name or ID
        name_or_id: String,
    },
    /// Update dedicated cluster settings
    Update {
        /// Cluster name or ID
        name_or_id: String,
        /// New cluster name
        #[arg(long)]
        name: Option<String>,
        /// Minutes without activity before automatically pausing; 0 disables idling
        #[arg(long)]
        idle_timeout_minutes: Option<u64>,
    },
    /// Request that a dedicated cluster stop
    Stop {
        /// Cluster name or ID
        name_or_id: String,
    },
    /// Request that a stopped dedicated cluster resume
    Resume {
        /// Cluster name or ID
        name_or_id: String,
    },
    /// Request deletion of a dedicated cluster and its data
    Delete {
        /// Cluster name or ID
        name_or_id: String,
    },
}

#[derive(Subcommand)]
pub enum KeyCommand {
    /// List API keys for a database
    List {
        #[arg(long)]
        database: Option<String>,
    },
    /// Create a new API key
    Create {
        #[arg(long)]
        database: Option<String>,
        /// Name for the key
        #[arg(long)]
        name: String,
        /// Permission level: admin, read_write, write_only, read_only
        #[arg(long)]
        permission: String,
    },
    /// Delete an API key
    Delete {
        #[arg(long)]
        database: Option<String>,
        /// Key ID or full API key token to delete
        id_or_token: String,
    },
}

#[derive(Subcommand)]
pub enum TableCommand {
    /// List tables in a database
    List {
        #[arg(long)]
        database: Option<String>,
    },
    /// Describe a table
    Describe {
        #[arg(long)]
        database: Option<String>,
        /// Table name
        table: String,
    },
}

#[cfg(test)]
mod tests {
    use super::{Cli, ClusterCommand, Command, KeyCommand};
    use clap::{error::ErrorKind, CommandFactory, Parser};

    #[test]
    fn root_command_exposes_version_flag() {
        let command = Cli::command();
        assert_eq!(command.get_version(), Some(env!("CARGO_PKG_VERSION")));
    }

    #[test]
    fn lowercase_v_triggers_version_output() {
        let err = match Cli::try_parse_from(["rtree", "-v"]) {
            Ok(_) => panic!("-v should print version"),
            Err(err) => err,
        };
        assert_eq!(err.kind(), ErrorKind::DisplayVersion);
        assert!(err.to_string().contains(env!("CARGO_PKG_VERSION")));
    }

    #[test]
    fn login_without_api_key_is_allowed_for_interactive_flow() {
        let cli = Cli::try_parse_from(["rtree", "login"]).expect("login should parse");
        assert!(cli.api_key.is_none());
    }

    #[test]
    fn login_with_api_key_parses() {
        let cli = Cli::try_parse_from(["rtree", "login", "--api-key", "rt_abc123"])
            .expect("login with --api-key should parse");
        assert_eq!(cli.api_key.as_deref(), Some("rt_abc123"));
    }

    #[test]
    fn login_help_hides_internal_email_and_password_flags() {
        let err = match Cli::try_parse_from(["rtree", "login", "--help"]) {
            Ok(_) => panic!("--help should return display output"),
            Err(err) => err,
        };
        assert_eq!(err.kind(), ErrorKind::DisplayHelp);
        let help = err.to_string();
        assert!(!help.contains("--email"));
        assert!(!help.contains("--password"));
        assert!(!help.to_ascii_lowercase().contains("email"));
    }

    #[test]
    fn global_api_key_parses_before_subcommand() {
        let cli = Cli::try_parse_from(["rtree", "--api-key", "rt_abc123", "database", "list"])
            .expect("global --api-key should parse before subcommand");
        assert_eq!(cli.api_key.as_deref(), Some("rt_abc123"));
    }

    #[test]
    fn cluster_list_parses() {
        let cli =
            Cli::try_parse_from(["rtree", "cluster", "list"]).expect("cluster list should parse");
        assert!(matches!(
            cli.command,
            Command::Cluster {
                action: ClusterCommand::List
            }
        ));
    }

    #[test]
    fn cluster_create_parses_explicit_name_and_idle_timeout() {
        let cli = Cli::try_parse_from([
            "rtree",
            "cluster",
            "create",
            "--name",
            "production",
            "--replicas",
            "2",
            "--min-cpu-cores",
            "2",
            "--min-memory-gb",
            "8",
            "--max-cpu-cores",
            "64",
            "--max-memory-gb",
            "256",
            "--idle-timeout-minutes",
            "30",
        ])
        .expect("cluster create should parse");
        assert!(matches!(
            cli.command,
            Command::Cluster {
                action: ClusterCommand::Create {
                    name,
                    replicas,
                    idle_timeout_minutes: Some(30),
                    min_cpu_cores,
                    min_memory_gb,
                    max_cpu_cores: Some(max_cpu_cores),
                    max_memory_gb: Some(max_memory_gb),
                }
            } if name == "production"
                && replicas == 2
                && min_cpu_cores == 2
                && min_memory_gb == 8
                && max_cpu_cores == 64
                && max_memory_gb == 256
        ));
    }

    #[test]
    fn cluster_update_parses_partial_settings() {
        let cli = Cli::try_parse_from([
            "rtree",
            "cluster",
            "update",
            "production",
            "--idle-timeout-minutes",
            "0",
        ])
        .expect("cluster update should parse");
        assert!(matches!(
            cli.command,
            Command::Cluster {
                action: ClusterCommand::Update {
                    name_or_id,
                    name: None,
                    idle_timeout_minutes: Some(0),
                }
            } if name_or_id == "production"
        ));
    }

    #[test]
    fn cluster_lifecycle_commands_parse() {
        let status = Cli::try_parse_from(["rtree", "cluster", "status", "production"])
            .expect("cluster status should parse");
        assert!(matches!(
            status.command,
            Command::Cluster {
                action: ClusterCommand::Status { name_or_id }
            } if name_or_id == "production"
        ));

        let stop = Cli::try_parse_from(["rtree", "cluster", "stop", "production"])
            .expect("cluster stop should parse");
        assert!(matches!(
            stop.command,
            Command::Cluster {
                action: ClusterCommand::Stop { name_or_id }
            } if name_or_id == "production"
        ));

        let resume = Cli::try_parse_from(["rtree", "cluster", "resume", "production"])
            .expect("cluster resume should parse");
        assert!(matches!(
            resume.command,
            Command::Cluster {
                action: ClusterCommand::Resume { name_or_id }
            } if name_or_id == "production"
        ));

        let delete = Cli::try_parse_from(["rtree", "cluster", "delete", "production"])
            .expect("cluster delete should parse");
        assert!(matches!(
            delete.command,
            Command::Cluster {
                action: ClusterCommand::Delete { name_or_id }
            } if name_or_id == "production"
        ));
    }

    #[test]
    fn project_command_is_rejected() {
        let result = Cli::try_parse_from(["rtree", "project", "list"]);
        assert!(result.is_err(), "project command should not be accepted");
    }

    #[test]
    fn login_with_token_flag_is_rejected() {
        let result = Cli::try_parse_from(["rtree", "login", "--token", "rt_abc123"]);
        assert!(result.is_err(), "--token should not be accepted");
    }

    #[test]
    fn login_with_password_requires_email() {
        let result = Cli::try_parse_from(["rtree", "login", "--password", "secret123"]);
        assert!(result.is_err(), "password without email should fail");
    }

    #[test]
    fn login_with_api_key_conflicts_with_email() {
        let cli = Cli::try_parse_from([
            "rtree",
            "login",
            "--api-key",
            "rt_abc123",
            "--email",
            "user@example.com",
        ])
        .expect("global --api-key is parsed before runtime login validation");
        assert_eq!(cli.api_key.as_deref(), Some("rt_abc123"));
    }

    #[test]
    fn login_with_database_without_email_is_allowed_for_interactive_flow() {
        let cli = Cli::try_parse_from(["rtree", "login", "--database", "analytics"])
            .expect("login with --database should parse");
        match cli.command {
            Command::Login {
                email, database, ..
            } => {
                assert!(email.is_none());
                assert_eq!(database.as_deref(), Some("analytics"));
            }
            _ => panic!("expected login command"),
        }
    }

    #[test]
    fn register_command_is_rejected() {
        let result = Cli::try_parse_from([
            "rtree",
            "register",
            "--email",
            "user@example.com",
            "--password",
            "secret123",
        ]);
        assert!(result.is_err(), "register command should not be accepted");
    }

    #[test]
    fn insert_with_url_is_allowed() {
        let cli = Cli::try_parse_from([
            "rtree",
            "insert",
            "--database",
            "analytics",
            "--table",
            "events",
            "--url",
            "https://example.com/events.jsonl",
        ])
        .expect("insert --url should parse");

        match cli.command {
            Command::Insert { url, .. } => {
                assert_eq!(url.as_deref(), Some("https://example.com/events.jsonl"))
            }
            _ => panic!("expected insert command"),
        }
    }

    #[test]
    fn insert_url_conflicts_with_data() {
        let result = Cli::try_parse_from([
            "rtree",
            "insert",
            "--database",
            "analytics",
            "--table",
            "events",
            "--url",
            "https://example.com/events.jsonl",
            "--data",
            r#"{"id":1}"#,
        ]);
        assert!(result.is_err(), "insert --url should conflict with --data");
    }

    #[test]
    fn api_url_and_insert_url_can_both_be_provided() {
        let cli = Cli::try_parse_from([
            "rtree",
            "--api-url",
            "https://api.rawtree.com",
            "insert",
            "--database",
            "analytics",
            "--table",
            "events",
            "--url",
            "https://example.com/events.jsonl",
        ])
        .expect("--api-url and insert --url should parse");

        assert_eq!(cli.api_url.as_deref(), Some("https://api.rawtree.com"));
        match cli.command {
            Command::Insert { url, .. } => {
                assert_eq!(url.as_deref(), Some("https://example.com/events.jsonl"))
            }
            _ => panic!("expected insert command"),
        }
    }

    #[test]
    fn api_url_can_be_passed_before_subcommand() {
        let cli = Cli::try_parse_from([
            "rtree",
            "--api-url",
            "https://api.rawtree.com",
            "query",
            "--database",
            "analytics",
            "--sql",
            "SELECT 1",
        ])
        .expect("--api-url should parse before subcommand");

        assert_eq!(cli.api_url.as_deref(), Some("https://api.rawtree.com"));
    }

    #[test]
    fn query_format_flag_is_rejected() {
        let result = Cli::try_parse_from([
            "rtree",
            "query",
            "--database",
            "analytics",
            "--sql",
            "SELECT 1",
            "--format",
            "csv",
        ]);
        assert!(result.is_err(), "query --format should not be supported");
    }

    #[test]
    fn query_project_flag_is_rejected() {
        let result = Cli::try_parse_from([
            "rtree",
            "query",
            "--project",
            "analytics",
            "--sql",
            "SELECT 1",
        ]);
        assert!(result.is_err(), "query should use --database");
    }

    #[test]
    fn query_named_query_flag_is_rejected() {
        let result = Cli::try_parse_from([
            "rtree",
            "query",
            "--database",
            "analytics",
            "--query",
            "SELECT 1",
        ]);
        assert!(result.is_err(), "query --query should not be supported");
    }

    #[test]
    fn key_command_is_singular() {
        let cli = Cli::try_parse_from(["rtree", "key", "list", "--database", "analytics"]).unwrap();

        match cli.command {
            Command::Key { action } => match action {
                KeyCommand::List { database } => {
                    assert_eq!(database.as_deref(), Some("analytics"));
                }
                _ => panic!("expected key list command"),
            },
            _ => panic!("expected key command"),
        }
    }

    #[test]
    fn keys_command_is_rejected() {
        let result = Cli::try_parse_from(["rtree", "keys", "list", "--database", "analytics"]);
        assert!(result.is_err(), "keys should not be accepted as a command");
    }

    #[test]
    fn key_create_uses_name_flag() {
        let cli = Cli::try_parse_from([
            "rtree",
            "key",
            "create",
            "--database",
            "analytics",
            "--name",
            "ci",
            "--permission",
            "read_write",
        ])
        .expect("key create with --name should parse");

        match cli.command {
            Command::Key { action } => match action {
                KeyCommand::Create { name, .. } => {
                    assert_eq!(name, "ci");
                }
                _ => panic!("expected key create command"),
            },
            _ => panic!("expected key command"),
        }
    }

    #[test]
    fn key_create_label_flag_is_rejected() {
        let result = Cli::try_parse_from([
            "rtree",
            "key",
            "create",
            "--database",
            "analytics",
            "--label",
            "ci",
            "--permission",
            "read_write",
        ]);
        assert!(result.is_err(), "key create should use --name, not --label");
    }

    #[test]
    fn logs_request_filters_parse() {
        let cli = Cli::try_parse_from([
            "rtree",
            "logs",
            "--database",
            "analytics",
            "--search",
            "request-123",
            "--methods",
            "GET,POST",
            "--status-codes",
            "200,500",
            "--levels",
            "success,error",
            "--sources",
            "cli",
            "--paths",
            "/v1/query,/v1/logs",
            "--log-databases",
            "events,audit",
        ])
        .expect("request log filters should parse");

        match cli.command {
            Command::Logs {
                search,
                methods,
                status_codes,
                levels,
                sources,
                paths,
                log_databases,
                ..
            } => {
                assert_eq!(search.as_deref(), Some("request-123"));
                assert_eq!(methods, vec!["GET", "POST"]);
                assert_eq!(status_codes, vec![200, 500]);
                assert_eq!(levels, vec!["success", "error"]);
                assert_eq!(sources, vec!["cli"]);
                assert_eq!(paths, vec!["/v1/query", "/v1/logs"]);
                assert_eq!(log_databases, vec!["events", "audit"]);
            }
            _ => panic!("expected logs command"),
        }
    }

    #[test]
    fn logs_status_codes_must_be_http_statuses() {
        let result = Cli::try_parse_from(["rtree", "logs", "--status-codes", "99"]);
        assert!(result.is_err(), "status codes below 100 should be rejected");
    }
}

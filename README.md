# RawTree CLI

The official CLI for the RawTree analytics platform.

Built for humans, AI agents, and CI/CD pipelines.

The package/repo name is `rawtree-cli`, and the command you run is `rtree`.

## Install

### GitHub Releases (recommended)

```sh
curl --proto '=https' --tlsv1.2 -LsSf \
  https://github.com/rawtreedb/rawtree-cli/releases/latest/download/rawtree-cli-installer.sh | sh
```

### Cargo (from source)

```sh
git clone https://github.com/rawtreedb/rawtree-cli.git
cd rawtree-cli
cargo install --path .
```

### Build locally

```sh
git clone https://github.com/rawtreedb/rawtree-cli.git
cd rawtree-cli
cargo build --release
./target/release/rtree --help
```

## Quick Start

```sh
# Authenticate (browser flow by default)
rtree login

# Create and select a database
rtree database create analytics
rtree database use analytics

# Insert a JSON row
rtree insert --table events --data '{"event":"signup","user_id":1}'

# Run a query
rtree query --sql "SELECT count(*) FROM events"

# Open the UI for the current database
rtree open
```

## Authentication

### Login modes

- Interactive login selection: `rtree login`
- Direct API key save: `rtree login --api-key rt_123`
- Select a database during auth in a specific cluster: `rtree login --org team-alpha --cluster production --database analytics`

Interactive login offers browser-based Rawtree authentication or securely prompts
for an existing API key. Non-interactive and `--json` login continue to use
browser-based authentication unless `--api-key` is provided.

When using `--api-key`, the CLI stores the API key directly and resolves organization/database defaults from that key.
With `--json`, API key login returns:

```json
{"success":true,"config_path":"<path>","database":"<name>","organization":"<name>"}
```

### Token resolution

1. `--api-key` flag
2. `RAWTREE_API_KEY` environment variable
3. Local config file

### Logout

```sh
rtree logout
```

Logout clears the local config, including any saved API URL, so the next run uses
the default `https://api.rawtree.com` endpoint unless an override is provided.

## Configuration

Config file location:

- Unix: `~/.config/rtree/config.json`

Resolution priority by setting:

- API KEY: `--api-key` -> `RAWTREE_API_KEY` -> config file token
- API URL: `--api-url` -> `RAWTREE_API_URL` -> config file -> `https://api.rawtree.com`
- Database: `--database` -> `RAWTREE_DATABASE` -> config file default database
- Organization: `--org` -> `RAWTREE_ORG` -> config file default organization
- Cluster routing: `--cluster` for the current invocation; API keys remain restricted to their bound cluster

## Commands

Top-level commands:

- `login`, `logout`
- `database`, `organization`, `cluster`, `key`, `table`
- `query`, `insert`
- `ping`, `docs`, `status`, `open`, `completions`

Global flags:

- `--api-url <URL>`
- `--org <ORG>`
- `--cluster <CLUSTER>`
- `--json`

## Common Workflows

### Databases and organizations

```sh
rtree organization list
rtree organization create team-alpha
rtree organization use team-alpha

rtree database list
rtree database create analytics
rtree database use analytics
```

### Querying

```sh
# Positional SQL
rtree query "SELECT * FROM events LIMIT 10"

# SQL from stdin
cat query.sql | rtree query -

# JSON output
rtree query --json --sql "SELECT * FROM events LIMIT 10"
```

### Data ingestion

```sh
# Inline JSON
rtree insert --org team-alpha --cluster production --database analytics --table events --data '{"event":"page_view"}'

# JSON/JSONL file
rtree insert --org team-alpha --cluster production --database analytics --table events --file ./events.jsonl

# Public URL to JSON/JSONL
rtree insert --org team-alpha --cluster production --database analytics --table events --url https://example.com/events.jsonl
```

### Keys and tables

```sh
rtree key list --database analytics
rtree key create --database analytics --name ci --permission read_write

rtree table list --database analytics
rtree table describe --database analytics events
```

### Clusters

```sh
rtree cluster list
rtree cluster list --json
rtree cluster status production
rtree cluster stop production
rtree cluster resume production
rtree cluster delete production
```

Cluster lifecycle changes are asynchronous. The `stop`, `resume`, and `delete`
commands return as soon as the API accepts the request; they do not wait for
the infrastructure operation to finish. After stopping or resuming a cluster,
run `rtree cluster status <name-or-id>` to follow its current state.

## Shell Completions

```sh
# Bash
rtree completions bash > ~/.rtree-completion.bash

# Zsh
rtree completions zsh > ~/.rtree-completion.zsh

# Fish
rtree completions fish > ~/.config/fish/completions/rtree.fish
```

## Local Development

Prerequisites:

- Rust (stable)

Setup:

```sh
git clone https://github.com/rawtreedb/rawtree-cli.git
cd rawtree-cli
cargo check
cargo test
```

Run locally:

```sh
cargo run -- --help
```

## Release Notes

- Repository/package name: `rawtree-cli`
- Executable name: `rtree`

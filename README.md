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

When using `--api-key`, the CLI stores the API key directly, resolves organization/database
defaults from that key, and validates any selected cluster against the key's bound cluster.
With `--json`, API key login returns:

```json
{"success":true,"config_path":"<path>","database":"<name>","organization":"<name>","cluster":"<name>"}
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
- Cluster: `--cluster` -> `RAWTREE_CLUSTER` -> config file default cluster

API keys remain restricted to their bound cluster regardless of which selector source is used.

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

rtree cluster list
rtree cluster use production

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
rtree insert --table events --data '{"event":"page_view"}'

# JSON/JSONL file
rtree insert --table events --file ./events.jsonl

# Public URL to JSON/JSONL
rtree insert --table events --url https://example.com/events.jsonl
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
rtree cluster sizes
rtree cluster create \
  --name production \
  --replicas 2 \
  --min-size 2:8 \
  --max-size 64:256 \
  --idle-timeout-minutes 30
# Or create a cluster with customer-owned S3 storage
rtree cluster create \
  --name customer-data \
  --replicas 1 \
  --min-size 2:8 \
  --s3-data-bucket acme-rawtree-data \
  --s3-data-path rawtree/data \
  --s3-backups-bucket acme-rawtree-backups \
  --s3-backups-path rawtree/backups \
  --s3-role-arn arn:aws:iam::123456789012:role/RawTreeS3Access \
  --s3-external-id rawtree-example
rtree cluster use production
rtree cluster status production
rtree cluster update production --idle-timeout-minutes 60
rtree cluster update production --idle-timeout-minutes 0
rtree cluster stop production
rtree cluster resume production
rtree cluster delete production
```

`--min-size` is required and uses `CPU_CORES:MEMORY_GIB` format. `--max-size`
is optional; omitting it uses the minimum size for both bounds and disables
vertical autoscaling. Sizes are validated against the server's cluster size
catalog. Run `rtree cluster sizes` to see the currently available sizes; add
`--json` for a machine-readable response.
`--idle-timeout-minutes` is optional on create and update; omit it to use the
server default on create, and pass `0` to disable automatic idling.

Cluster and database creation accept the same customer-owned S3 options. Pass
all four required values (`--s3-data-bucket`, `--s3-backups-bucket`,
`--s3-role-arn`, and `--s3-external-id`) to send the platform's `s3_storage`
configuration; the data and backup paths are optional. For a database:

```sh
rtree database create analytics \
  --s3-data-bucket acme-rawtree-data \
  --s3-backups-bucket acme-rawtree-backups \
  --s3-role-arn arn:aws:iam::123456789012:role/RawTreeS3Access \
  --s3-external-id rawtree-example
```

If no S3 options are passed, the database inherits the cluster's storage. Use
`rtree cluster list --json` or `rtree database list --json` to inspect the
safe bucket/path metadata returned as `s3_storage`; credentials are never
returned.

Cluster lifecycle and provisioning changes are asynchronous. The `create`,
`stop`, `resume`, and `delete` commands return as soon as the API accepts the
request; they do not wait for the infrastructure operation to finish. After
creating, stopping, or resuming a cluster, run `rtree cluster status
<name-or-id>` to follow its current state.

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

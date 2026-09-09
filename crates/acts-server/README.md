# acts-server

The acts workflow server: an embedded acts engine exposed over multiple
transports. It registers the core packages plus the transport plugins
selected by the server config, and ships together with `acts-cli`.

## Install

### Option 1 — GitHub release binaries (macOS / Linux / Windows)

Every `v*` tag builds `acts-server` and `acts-cli` and uploads them to the
[GitHub releases](https://github.com/yaojianpin/acts/releases) page. Each
archive (`acts-<version>-<os>-<arch>.tar.gz` / `.zip`) contains **both**
binaries — install them with the install scripts:

macOS / Linux (bash):

```bash
curl -fsSL https://raw.githubusercontent.com/yaojianpin/acts/main/install.sh | bash
```

Windows (PowerShell):

```powershell
iwr -useb https://raw.githubusercontent.com/yaojianpin/acts/main/install.ps1 | iex
```

The binaries are installed to `~/.acts/bin` (override with
`ACTS_INSTALL_DIR`); add that directory to your `PATH`. Or download the
archive for your platform from the releases page and unpack it anywhere.

### Option 2 — cargo install

```bash
cargo install acts-server
```

The first start auto-creates the config file (see below), so no setup is
needed after the install.

## Run & first start

```bash
acts-server
```

On the first run acts-server creates its config directory `~/.acts`
(`$HOME/.acts`, `%USERPROFILE%\.acts` on Windows) and writes a default
`acts.toml` there with a working sled database under `~/.acts/data` and logs
under `~/.acts/log`. The server then blocks until interrupted.

To point the config directory elsewhere, set `ACTS_CONFIG_DIR`.

## Configuration

The effective config is layered, each file deep-merged per key over the
previous one (nested `[tables]` merge field by field, other values replace):

1. `~/.acts/acts.toml` — server defaults, auto-created when missing
2. `./acts.toml` in the working directory (if present)

so a project can run its own server with a small local file:

```toml
# acts.toml — overrides only what it sets
[web]
port = 18082
```

Relative paths in a file are resolved from the working directory; the
auto-created `~/.acts/acts.toml` uses absolute paths into the config dir.

### Storage (`[db]`)

The default store is **sled** (a directory under the config dir). Other
backends are configured in the `[db]` table with a `type` and a
`database_url`; the `ACTS_DATABASE_URL` env var overrides `database_url`:

```toml
[db]
type = "postgres"
database_url = "postgres://user:pass@host:5432/acts"
```

| type     | database_url                          |
| -------- | ------------------------------------- |
| `sled`   | directory path (default)              |
| `sqlite` | sqlite file path                      |
| `postgres` | `postgres://user:pass@host:5432/db` |
| `redis`  | `redis://host:6379`                   |
| `nats`   | `nats://host:4222` (JetStream KV)     |

```toml
[db]
type = "sqlite"
database_url = "./data/acts.db"   # or set ACTS_DATABASE_URL
```

### Transports

```toml
[grpc]                          # acts-plugin-grpc
port = 10080                    # default 10080

[web]                           # acts-plugin-web (only when this section exists)
port = 10082                    # default 10082

[nats]                          # acts-plugin-nats — only connected when
url = "nats://127.0.0.1:4222"   # this section is present
subject = "acts"

[[nats.channels]]               # engine events forwarded to NATS
id = "ops"
type = "*"
state = "*"
uses = "*"
```

Plugin registration mirrors the config:
- the gRPC and web plugins always start (default ports 10080 / 10082);
- the NATS plugin is registered only when a `[nats]` section exists, so a
  server without NATS never tries to reach a broker.

### Snapshot targets

`[[snapshot]]` entries (see the default config) pre-register
snapshot-backed sealed-data targets: data fed through the snapshot APIs is
sealed into tasks at their prepare under each target's
`policy`/`scope`/`on_missing`/`ttl` settings.

## Build & run (development)

```bash
cargo run -p acts-server
```

## Clients

- `acts-cli` — interactive client over gRPC
- `acts-channel` — gRPC client library (`ActsChannel`)
- any HTTP client against the web API (`/api/*`, `/hooks/{event-id}`)
- any NATS client against the actions/event subjects (see
  `plugins/acts-plugin-nats/README.md`)

## Transports

The three plugins share one action set (dispatch lives in
`acts::actions`): model/package/proc/task/message/act/event commands
plus snapshot operations (`snap:upsert`, `snap:remove`, `snap:get`,
`snap:ls`). Over HTTP the snapshot endpoints are `/api/snap/upsert`,
`/api/snap/get`, `/api/snap/ls` and `/api/snap/remove`.

## Tests

`cargo test -p acts-server --test nats` boots the same engine the binary
runs (NATS plugin only) and exercises snapshot actions over NATS; it skips
when no broker is reachable (`ACTS_NATS_URL`, default
`nats://127.0.0.1:4222`). CI provides a NATS service container.

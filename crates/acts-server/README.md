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
under `~/.acts/log`. The server then blocks until it is asked to stop:
Ctrl-C (SIGINT), or SIGTERM on macOS/Linux. It logs the signal, closes the
engine — flushing the store writer and stopping the transport plugins and
their background tasks — and exits.

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

Every database has **one writer**, and the document locks that keep a row and
its index entries consistent are process-local: two servers on one database
have no mutual exclusion, so their concurrent writes to the same row can leave
a query matching a row that no longer holds the value, or missing one that
does. Run one `acts-server` per database; a deployment that needs several gives
each its own, or supplies coordination the store does not — a backend
conditional write, or a lock held across the read. A single `batch` is not
that: it is atomic on its own, while a read followed by another process's batch
is not.

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

### Access control

Without an `[acl]` section the engine is **anonymous and read-only**: every
request is attributed to the built-in `anonymous` subject, which may list and
get models, processes, tasks, messages, events and packages — no writes, no
control actions (`proc:start`, `act:*`, `evt:start`, `msg:ack`), no admin
actions, no snapshot scope, no subscriptions. Add `[acl]` to name your callers
(a single `token` is the smallest useful section), or write `enabled = false`
inside it to lift the limits on purpose.

| Transport | Credential |
|-----------|------------|
| gRPC | `authorization: Bearer <token>` metadata, on actions and the `on_message` subscription |
| HTTP | `authorization: Bearer <token>` header; `/health` stays open |
| NATS | the `token` field of the action JSON body |

The token selects a role; the role's `allow`/`deny` action-name globs decide,
with `deny` winning. Tokens are compared as SHA-256 digests
(`sha256:<hex>` keeps the clear text out of the config). Opening a message
stream is itself an action (`msg:sub`), and a subscription's channel is
namespaced by the caller's subject, so one caller cannot take over another's
channel. A `snapshot` table per role narrows which scopes of which targets the
subject owns (`$subject` is the role name), and that ownership is re-checked
when a task seals a snapshot value — a workflow cannot read another subject's
sealed data even when started with their `uid`.

`workdir` (on `[acl]`, or per role) is a **root**: each run gets its own
directory `<workdir>/<pid>` and its filesystem access is confined to that one.
The process id becomes a path segment (so one that is not a single safe
component is refused), acts read the directory through `Context::workdir()`,
and a script reads the same directory as `$env.WORK_DIR` (engine-owned: a write
to that name is dropped). The directory is removed together with the run's rows
— once the run finished and every delivery of its messages settled — so it does
not accumulate one directory per historical process; a run whose row is kept
(an errored delivery awaiting a manual retry) keeps its directory too, and
anything a run needs to outlive itself must be exported, not left in the
workdir.

See the commented template in the generated default config
(`~/.acts/acts.toml`) or the access-control chapter of the book.

## Tests

`cargo test -p acts-server --test nats` boots the same engine the binary
runs (NATS plugin only) and exercises snapshot actions over NATS; it skips
when no broker is reachable (`ACTS_NATS_URL`, default
`nats://127.0.0.1:4222`). CI provides a NATS service container.

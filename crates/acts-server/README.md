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

```toml
[db]
type = "sqlite"
database_url = "./data/acts.db"   # or set ACTS_DATABASE_URL
```

Every database has **one writer**, and the server takes an exclusive lease on
it before starting an engine:

```toml
[db]
lease = true          # default; the database's exclusive lease
lease_ttl_secs = 30   # a dead holder blocks a restart for at most this long
lease_renew_secs = 10 # at most half the TTL
# lease_owner = "acts-1"   # default "<host>:<pid>"
```

The lease is a row in the database itself (`__acts_lease__`), claimed with an
atomic compare-and-swap and renewed on an interval; every write the engine
makes commits only while this server still holds it. Two servers on one
database therefore cannot both run: the second fails its startup with
`LeaseHeld` naming the holder (and succeeds once the first stops — a graceful
stop hands the lease back, a crash leaves it to expire). An instance whose
renewals fail for longer than the TTL is taken over, and from that moment every
write of it is refused with `LeaseLost` and its engine is stopped, so it can
never overwrite the new holder's rows. The holder's fence rises with every
acquisition — across crashes and restarts — and a write carrying a stale fence
is refused inside the same atomic write that would have committed it.

That matters because the document locks that keep a row and its index entries
consistent are process-local: two servers on one database have no mutual
exclusion of their own, so their concurrent writes to the same row could leave
a query matching a row that no longer holds the value, or missing one that
does. The lease is what makes one server the writer; it is on by default and
needs no configuration.

`lease = false` turns it off and leaves one writer per database to you: give
each server its own database, or coordinate outside the engine. An embedded
engine (`EngineBuilder::set_store`) never takes a lease either — it is the raw
store you passed, so the single-writer rule is yours there too.

### Logging (`[log]`)

The server logs to stdout and to hourly rolling files under `[log].dir`
(`acts.log.<YYYY-MM-DD-HH>`, UTC). `[log].max_files` bounds how many of those
files are kept: the oldest goes at every rotation, and stale files are pruned
once at startup, so a restart also reclaims the space. It defaults to 168
(one week of hourly files); `0` keeps every file, and at least 2 files are
always kept (the current one and the previous).

```toml
[log]
dir = "/var/log/acts"
level = "INFO"     # ACTS_LOG overrides the level at runtime
max_files = 168
```

### Transports

```toml
[grpc]                          # acts-plugin-grpc
port = 10080                    # default 10080
queue_size = 128                # messages that may wait for one subscriber
                                # (default 128); a full queue ends the stream

[web]                           # acts-plugin-web (only when this section exists)
port = 10082                    # default 10082
queue_size = 100                # same for one SSE subscriber (default 100)

[nats]                          # acts-plugin-nats — only connected when
url = "nats://127.0.0.1:4222"   # this section is present
subject = "acts"
max_in_flight = 256             # actions running at once (default 256); past it
                                # a caller is answered `err` instead of started

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

`[nats].max_in_flight` bounds the actions running at once; a burst past it is
refused to its callers (`err`) rather than started, so a busy actions subject
cannot turn into unbounded work.

A subscription's queue is its only backlog: a message that does not fit is never
awaited on, and a subscriber that fills its queue has its stream ended instead
of leaving behind a task that waits for room with the message in hand. Its
unacked deliveries of processes that have not settled stay in the store, so the
retry timer re-sends them when the client subscribes again.

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

Without an `[acl]` section the engine is **anonymous and catalogue-only**: every
request is attributed to the built-in `anonymous` subject, which may list and
get **models and packages** — no other read (a run, a delivery or a trigger is
someone's work), no write, no control actions (`proc:start`, `act:*`,
`evt:start`, `msg:ack`), no admin actions, no snapshot scope, no subscriptions.
Add `[acl]` to name your callers (a single `token` is the smallest useful
section), or write `enabled = false` inside it to lift the limits on purpose.

Every operation is checked — including the ones an embedder performs through
`Engine::executor(&principal)` — and the executor seals the principal's
snapshot scopes and workdir root into every run it starts.

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

`acts.app.shell` mounts that directory as the root of the script's filesystem:
the script runs in [bashkit](https://github.com/everruns/bashkit), a virtual
bash with no process behind it, where `/` is the run's own directory and the
rest of the host filesystem is not part of the filesystem it was given. A file
the host puts there is readable at the same relative path, and what the script
writes there is the file the host sees. A run with no workdir still runs its
script, on an in-memory filesystem with no host behind it. The package also
adds a script policy of its own: `[shell] allow`/`deny` are globs over the whole
script text (`*` spans `/` and newlines, `deny` wins), and a script the policy
refuses fails the act before anything runs. `shell: bash` is the only accepted
shell.

See the commented template in the generated default config
(`~/.acts/acts.toml`) or the access-control chapter of the book.

## Tests

`cargo test -p acts-server --test nats` boots the same engine the binary
runs (NATS plugin only) and exercises snapshot actions over NATS; it skips
when no broker is reachable (`ACTS_NATS_URL`, default
`nats://127.0.0.1:4222`). CI provides a NATS service container.

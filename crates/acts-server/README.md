# acts-server

The acts workflow server: an embedded acts engine exposed over multiple
transports. It registers the core packages plus the transport plugins
selected by the server config.

## Build & run

```bash
cargo run -p acts-server
```

The server reads `config/acts.toml` from the working directory and keeps
log files under the configured log dir. It blocks until interrupted.

## Configuration (`config/acts.toml`)

```toml
[db]
database_url = "./data"          # sled store path

[log]
dir = "data"
level = "INFO"

# transport plugins (all optional):

[grpc]                           # acts-plugin-grpc
port = 10080                     # default 10080

[http]                           # acts-plugin-web
port = 10082                     # default 10082

[nats]                           # acts-plugin-nats — only connected when
url = "nats://127.0.0.1:4222"    # this section is present
subject = "acts"

[[nats.channels]]                # engine events forwarded to NATS
id = "ops"
type = "*"
state = "*"
uses = "*"
```

Plugin registration mirrors the config:
- the gRPC and web plugins always start (default ports 10080 / 10082);
- the NATS plugin is registered only when a `[nats]` section exists, so a
  server without NATS never tries to reach a broker.

## Clients

- `acts-cli` — interactive client over gRPC
- `acts-channel` — gRPC client library (`ActsChannel`)
- any HTTP client against the web API (`/api/*`, `/hooks/{event-id}`)
- any NATS client against the actions/event subjects (see
  `plugins/acts-plugin-nats/README.md`)

## Transports

The three plugins share one action set (dispatch lives in
`acts-plugin-common`): model/package/proc/task/message/act/event commands
plus snapshot operations (`snap:upsert`, `snap:remove`, `snap:get`,
`snap:ls`). Over HTTP the snapshot endpoints are `/api/snap/upsert`,
`/api/snap/get`, `/api/snap/ls` and `/api/snap/remove`.

## Tests

`cargo test -p acts-server --test nats` boots the same engine the binary
runs (NATS plugin only) and exercises snapshot actions over NATS; it skips
when no broker is reachable (`ACTS_NATS_URL`, default
`nats://127.0.0.1:4222`). CI provides a NATS service container.

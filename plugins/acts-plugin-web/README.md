# acts-plugin-web

HTTP REST API plugin for the acts workflow engine. Spawns an axum server with endpoints for model management, process execution, package discovery, and Server-Sent Events (SSE) message streaming.

## Installation

```toml
[dependencies]
acts-plugin-web = { path = "./plugins/acts-plugin-web" }
```

## Usage

```rust,no_run
use acts::Engine;
use acts_plugin_web::WebPlugin;

#[tokio::main]
async fn main() {
    let engine = Engine::builder()
        .add_plugin(&WebPlugin::new())
        .start()
        .unwrap();

    // HTTP server is now listening on port 10082 (configurable via config/acts.toml)
}
```

## Configuration

In `config/acts.toml`:

```toml
[web]
host = "127.0.0.1"
port = 10082
# how many messages may wait for one SSE subscriber (default 100)
queue_size = 100
```

`host` is the address the server binds. It defaults to `127.0.0.1`, so the
management surface answers local callers only; set `host = "0.0.0.0"` to serve
remote callers. The listener is bound while the engine starts: a port another
process already holds fails the start with the bind error instead of leaving a
"running" engine without its HTTP transport.

`queue_size` is the whole backlog of one subscription: a message that does not
fit is never awaited on, and a subscriber whose queue is full is disconnected
rather than leaving behind a task that waits for room with the message in hand.
Its unacked deliveries of processes that have not settled stay in the store, so
the engine's retry timer re-sends them to the channel the client registers again
(the client sees the stream end and resubscribes).

## Authentication

Every endpoint except `/health` runs its operation through the shared action
table (`acts::actions::apply_as`) as the request's principal, so the same role
rules cover HTTP, gRPC and NATS. The principal is the bearer token's role; with
no token — or no `[acl]` section at all — it is the `anonymous` subject, which
may read only the catalogue (`model:ls`/`model:get`/`pack:ls`/`pack:get`).
A refusal answers `401` (no/unknown token) or `403` (authenticated but not
allowed); an endpoint outside the catalogue therefore answers `401` until an
`[acl]` section names its caller (`/api/msg/sse` additionally needs the
`msg:sub` grant).

See the access-control chapter of the book for the config format.

## Endpoints

| Method | Path | Description |
|--------|------|-------------|
| `GET` | `/health` | Health check |
| `POST` | `/api/model/deploy` | Deploy a workflow model |
| `POST` | `/api/model/list` | List deployed models |
| `POST` | `/api/model/get` | Get a model by id |
| `POST` | `/api/model/rm` | Remove a model |
| `POST` | `/api/proc/start` | Start a process (by model id or inline) |
| `POST` | `/api/pack/list` | List available packages |
| `GET` | `/api/pack/catalogs` | List package catalogs |
| `POST` | `/api/pack` | Get package details |
| `POST` | `/api/msg/sse` | Subscribe to workflow events via SSE |
| `POST` | `/api/msg/ack` | Acknowledge a message |

### SSE Streaming

```bash
curl -N "http://127.0.0.1:10082/api/msg/sse/my-client?type=step&state=created"
```

## Model

```yml
name: web example
id: web-example
ver: 0.1.0
steps:
  - name: simple step
    uses: acts.core.set
    params:
      message: "Hello from Web!"
```

See `examples/plugins/web/` for a complete runnable example.

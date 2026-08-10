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
        .build()
        .start()
        .unwrap();

    // HTTP server is now listening on port 10082 (configurable via config/acts.toml)
}
```

## Configuration

In `config/acts.toml`:

```toml
[http]
port = 10082
```

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
steps:
  - name: simple step
    acts:
      - uses: acts.core.set
        params:
          message: "Hello from Web!"
```

See `examples/plugins/web/` for a complete runnable example.

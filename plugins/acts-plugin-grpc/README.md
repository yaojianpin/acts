# acts-plugin-grpc

gRPC server plugin for the acts workflow engine. Spawns a tonic gRPC server that exposes the `ActsService` API — model management, process control, task operations, message streaming, and more.

## Installation

```toml
[dependencies]
acts-plugin-grpc = { path = "./plugins/acts-plugin-grpc" }
```

## Usage

```rust,no_run
use acts::Engine;
use acts_plugin_grpc::GrpcPlugin;

#[tokio::main]
async fn main() {
    let engine = Engine::builder()
        .add_plugin(&GrpcPlugin::new())
        .build()
        .start()
        .unwrap();

    // gRPC server is now listening on port 10080 (configurable via config/acts.toml)
    // engine stays alive until closed
}
```

## Configuration

In `config/acts.toml`:

```toml
[grpc]
port = 10080
```

## Endpoints

The gRPC server implements the full `ActsService`:

| RPC | Description |
|-----|-------------|
| `Send(Message)` | Execute an action (`model:*`, `proc:*`, `pack:*`, `task:*`, `msg:*`, `evt:*`, `act:*`) |
| `OnMessage(MessageOptions)` | Streaming subscription to workflow events |

## Model

```yml
name: grpc example
id: grpc-example
ver: 0.1.0
steps:
  - name: simple step
    uses: acts.core.set
    params:
      message: "Hello from gRPC!"
```

See `examples/plugins/grpc/` for a complete runnable example.

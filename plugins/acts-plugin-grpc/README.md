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
# how many messages may wait for one `on_message` subscriber (default 128)
queue_size = 128
```

`queue_size` is the whole backlog of one subscription: a message that does not
fit is never awaited on, and a subscriber whose queue is full has its stream
ended rather than leaving behind a task that waits for room with the message in
hand. Its unacked deliveries of processes that have not settled stay in the
store, so the engine's retry timer re-sends them to the channel the client
registers again (the client sees its stream end and resubscribes).

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

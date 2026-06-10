# Channel Client

The client channel is used by application services to connect to the workflow service, subscribe to messages, and execute workflow tasks.

## Installation

Install the client library via `cargo`:

```bash
cargo add acts-channel
```

## Supported Languages

| Language | Library |
| ---- | ---- |
| Rust | [acts-channel](https://github.com/yaojianpin/acts-channel) |
| Python | [acts-channel-py](https://github.com/yaojianpin/acts-channel-py) |
| Go | [acts-channel-go](https://github.com/yaojianpin/acts-channel-go) |

## Basic Usage

```rust
use acts_channel::{Client, ChannelOptions};

let mut client = Client::new("http://localhost:8080", &ChannelOptions::default());

// Connect to the service
client.connect().await?;

// Subscribe to messages
client.subscribe("client1", "act*", None, None).await?;

// Deploy a model
client.deploy(&model_str).await?;

// Start a workflow
let mut vars = Vars::new();
vars.set("input", 100);
client.start("model_id", vars).await?;
```

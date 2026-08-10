# acts-channel

gRPC client/server channel for the acts workflow engine. Provides the protobuf service definition (`ActsService`), generated client/server stubs, and a high-level `ActsChannel` client.

## Installation

```toml
[dependencies]
acts-channel = { path = "./crates/acts-channel" }
```

## Client Usage

Connect to an acts gRPC server and subscribe to workflow messages:

```rust,ignore
use acts_channel::{ActsChannel, ActsOptions};

let mut client = ActsChannel::connect("http://127.0.0.1:10080").await?;

client
    .subscribe("my-client", move |msg| println!("{msg:?}"), &ActsOptions::default())
    .await;
```

### Actions

```rust,ignore
// deploy a model
client.deploy(yml_str, Some("model-id")).await?;

// start a process
client.submit("pid", "tid", vars).await?;

// ack a message
client.ack("msg-id").await?;

// send an arbitrary action
client.send::<()>("complete", options).await?;
```

## Server Usage

Implement `ActsService` to build a gRPC server. See `acts-plugin-grpc` for a ready-to-use plugin.

## Proto

```protobuf
service ActsService {
  rpc Send(Message) returns (Message) {}
  rpc OnMessage(MessageOptions) returns (stream Message) {}
}
```

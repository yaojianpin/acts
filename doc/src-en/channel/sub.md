# Subscribe

Subscribe to workflow messages via the client channel.

## Subscribe to Messages

```rust
use acts_channel::{ActsChannel, ActsOptions};

let mut client = ActsChannel::connect("http://localhost:8080");
client.connect().await?;

client.subscribe("my_client", "act*", None, None).await?;

// Subscribe to messages with a specific value
// options supports glob patterns, e.g. "act*" matches all message starting with "act"
let options = ActsOptions {
    tag: "your tag",
    state: "{created,completed}"
    r#type: "act*"
    ..ChannelOptions::default()
};
client
    .subscribe(
        "client-1",
        move |message| {
            println!("{message:?}");
        },
        &options,
    )
    .await;
```


## Message Types

| Type | Description |
| ---- | ---- |
| `workflow` | Workflow-level message |
| `step` | Step-level message |
| `act` | action message |

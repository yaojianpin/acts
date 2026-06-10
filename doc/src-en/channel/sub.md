# Subscribe

Subscribe to workflow messages via the client channel.

## Subscribe to Messages

```rust
use acts_channel::{Client, ChannelOptions};

let mut client = Client::new("http://localhost:8080", &ChannelOptions::default());
client.connect().await?;

// Subscribe to messages with a specific key
// key supports glob patterns, e.g. "act*" matches all keys starting with "act"
client.subscribe("my_client", "act*", None, None).await?;
```

## Message Callback

```rust
use acts_channel::{Client, ChannelOptions, Message};

fn on_message(msg: &Message) {
    match msg.r#type.as_str() {
        "req" => {
            // Handle interrupt request
            println!("Received request action: {:?}", msg);
        }
        "msg" => {
            // Handle message notification
            println!("Received message notification: {:?}", msg);
        }
        _ => {}
    }
}

let options = ChannelOptions {
    on_message: Some(on_message),
    ..ChannelOptions::default()
};
```

## Message Types

| Type | Description |
| ---- | ---- |
| `workflow` | Workflow-level message |
| `step` | Step-level message |
| `req` | Interrupt request action message |
| `msg` | One-way message notification |

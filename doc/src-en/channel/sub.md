# Subscribe

Subscribe to workflow messages via the client channel.

## Subscribe to Messages

```rust
use acts_channel::{ActsChannel, ActsOptions};

let mut client = ActsChannel::connect("http://127.0.0.1:10080").await?;

// ActsOptions fields support glob patterns, e.g. "act*" matches every message
// starting with "act"
let options = ActsOptions {
    state: Some("{created,completed}".to_string()),
    r#type: Some("act*".to_string()),
    ..ActsOptions::default()
};

let sub = client
    .subscribe(
        "client-1",
        move |message| {
            println!("{message:?}");
        },
        // faults that do not end the feed: decode failures, failed auto-acks
        move |err| eprintln!("subscription fault: {err}"),
        &options,
    )
    .await?;

// the end of the feed: Ok(()) on a clean close, Err(status) on a failure
if let Err(err) = sub.wait().await {
    eprintln!("subscription closed: {err}");
}
```


## Message Types

| Type | Description |
| ---- | ---- |
| `workflow` | Workflow-level message |
| `step` | Step-level message |
| `act` | action message |

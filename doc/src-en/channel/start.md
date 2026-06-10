# Start

Start a workflow via the client channel.

## Start Workflow

```rust
use acts_channel::{Client, ChannelOptions};

let mut client = Client::new("http://localhost:8080", &ChannelOptions::default());
client.connect().await?;

// Start workflow
let mut vars = Vars::new();
vars.set("a", 100);
client.start("model_id", vars).await?;
```

## Start Parameters

You can pass variables when starting to override the workflow's default `vars`:

```rust
let mut vars = Vars::new();
vars.set("input_value", 42);
vars.set("user_name", "admin");

client.start("my_workflow", vars).await?;
```

## Event-Triggered Start

If the workflow defines an `on` event, you can also trigger startup via events:

```rust
use acts::executor::Executor;

let mut vars = Vars::new();
vars.set("data", "event_data");
executor.evt().start("model-id:event-id", &vars)?;
```

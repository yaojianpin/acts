# Start

Start a workflow via the client channel.

## Start Workflow

```rust
use acts_channel::{ActsChannel, ChannelOptions};

let mut client = ActsChannel::connect("http://localhost:8080");

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


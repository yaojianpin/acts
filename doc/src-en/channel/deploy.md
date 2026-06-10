# Deploy

Deploy workflow models via the client channel.

## Deploy Model

```rust
use acts_channel::{Client, ChannelOptions};

let mut client = Client::new("http://localhost:8080", &ChannelOptions::default());
client.connect().await?;

// Load model from file and deploy
let model = std::fs::read_to_string("workflow.yml").unwrap();
client.deploy(&model).await?;
```

## Build Model Dynamically

You can also build models dynamically via Rust code before deploying:

```rust
use acts::Workflow;

let workflow = Workflow::new("my_model", "my workflow")
    .add_step(|step| {
        step.set_uses("acts.core.irq")
    });

let model_str = serde_yaml::to_string(&workflow).unwrap();
client.deploy(&model_str).await?;
```

# Deploy

Deploy workflow models via the client channel.

## Deploy Model

```rust
use acts_channel::ActsChannel;

let mut client = ActsChannel::connect("http://localhost:8080");

// Load model from file and deploy
let model = std::fs::read_to_string("workflow.yml").unwrap();
let resp = client
    .deploy(yml, Some("custom_model_id")).await?;
```

## Build Model Dynamically

You can also build models dynamically via Rust code before deploying:

```rust
use acts::{Workflow, Vars};

let workflow = Workflow::new("my_model", "my workflow")
    .with_step(|step| {
        step.with_uses("acts.core.irq", Vars::new().with("key", "my_key"))
    });

let model_str = serde_yaml::to_string(&workflow).unwrap();
client.deploy(&model_str, Some("custom_model_id")).await?;
```

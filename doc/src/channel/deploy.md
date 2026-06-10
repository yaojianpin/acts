# 部署

通过客户端 Channel 部署工作流模型。

## 部署模型

```rust
use acts_channel::ActsChannel;

let mut client = ActsChannel::connect("http://localhost:8080");

// 从文件加载模型并部署
let model = std::fs::read_to_string("workflow.yml").unwrap();
let resp = client
    .deploy(yml, Some("custom_model_id")).await?;
```

## 动态构建模型

也可以通过 Rust 代码动态构建模型后部署：

```rust
use acts::{Workflow, Vars};

let workflow = Workflow::new("my_model", "my workflow")
    .with_step(|step| {
        step.with_uses("acts.core.irq", Vars::new().with("key", "my_key"))
    });

let model_str = serde_yaml::to_string(&workflow).unwrap();
client.deploy(&model_str, Some("custom_model_id")).await?;
```

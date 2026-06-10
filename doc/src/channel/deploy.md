# 部署

通过客户端 Channel 部署工作流模型。

## 部署模型

```rust
use acts_channel::{Client, ChannelOptions};

let mut client = Client::new("http://localhost:8080", &ChannelOptions::default());
client.connect().await?;

// 从文件加载模型并部署
let model = std::fs::read_to_string("workflow.yml").unwrap();
client.deploy(&model).await?;
```

## 动态构建模型

也可以通过 Rust 代码动态构建模型后部署：

```rust
use acts::Workflow;

let workflow = Workflow::new("my_model", "my workflow")
    .add_step(|step| {
        step.set_uses("acts.core.irq")
    });

let model_str = serde_yaml::to_string(&workflow).unwrap();
client.deploy(&model_str).await?;
```
